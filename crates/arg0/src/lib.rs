//! Single-binary dispatch via `argv[0]`.
//!
//! Allows `devo` and `devo-server` to share a single executable. The trick:
//!
//! - On Unix, symlinks (e.g. `devo-server -> devo`) are placed on PATH so the
//!   right sub‑function runs based on `argv[0]`.
//! - On Windows, `.bat` wrappers are generated that re‑invoke the binary with a
//!   sentinel argument so the same dispatch logic can work.
//!
//! Usage from `main()`:
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     devo_arg0::run_as(|_paths| async {
//!         // regular CLI logic here
//!     })
//! }
//! ```

use std::ffi::OsString;
use std::future::Future;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

/// Alias name for the server sub‑binary.
const SERVER_ALIAS: &str = "devo-server";
const ALIAS_SENTINEL_PREFIX: &str = "--devo-alias=";

/// Directory (under DEVO_HOME/tmp/arg0) where alias entries are created.
const ALIAS_TEMP_ROOT: &str = "arg0";
const LOCK_FILENAME: &str = ".lock";

/// Stack size for Tokio worker threads (16 MB).
const TOKIO_WORKER_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

// ── Public types ───────────────────────────────────────────────────────────

/// Paths to the current executable and its helper aliases.
///
/// Passed to the main closure so child processes can re‑invoke the binary
/// without relying on [`std::env::current_exe()`] (which can be unreliable
/// under test harnesses).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Arg0DispatchPaths {
    /// Stable path to the current devo executable for child re-execs.
    pub devo_self_exe: Option<PathBuf>,
}

/// Result of a pre-runtime dispatch hook.
pub enum EarlyDispatch {
    /// Continue into the normal async CLI runtime.
    Continue,
    /// Return this result without creating the normal async CLI runtime.
    Handled(Result<()>),
}

// ── Public API ────────────────────────────────────────────────────────────

/// Initialize the tokio-console tracing subscriber if `TOKIO_CONSOLE` is set
/// and the `tokio-console` feature is active. Must be called before any other
/// tracing subscriber is installed.
pub fn maybe_init_tokio_console() {
    #[cfg(feature = "tokio-console")]
    if std::env::var("TOKIO_CONSOLE").is_ok() {
        console_subscriber::init();
    }
    #[cfg(not(feature = "tokio-console"))]
    let _ = std::env::var("TOKIO_CONSOLE");
}

/// Entry‑point wrapper that performs `argv[0]` dispatch first.
///
/// If the current executable was invoked as `devo-server` (via symlink or batch
/// script), `run_as` runs the server entry‑point and exits the process.
/// Otherwise it calls `main_fn`, which should contain the normal CLI logic.
///
/// The function also:
/// - Loads `~/.devo/.env` (if present) before any threading starts, filtering
///   out `DEVO_`-prefixed vars for security.
/// - Injects a temporary directory on `PATH` so that alias names (symlinks /
///   `.bat` scripts) are found by child processes.
/// - Cleans up stale temp directories from previous sessions.
pub fn run_as<F, Fut>(main_fn: F) -> Result<()>
where
    F: FnOnce(Arg0DispatchPaths) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    run_as_with_early_dispatch(main_fn, |_paths| EarlyDispatch::Continue)
}

/// Entry-point wrapper with a pre-runtime dispatch hook for commands that must
/// own the process main thread before Tokio starts.
pub fn run_as_with_early_dispatch<F, Fut, D>(main_fn: F, early_dispatch: D) -> Result<()>
where
    F: FnOnce(Arg0DispatchPaths) -> Fut,
    Fut: Future<Output = Result<()>>,
    D: FnOnce(&Arg0DispatchPaths) -> EarlyDispatch,
{
    // ── argv[0] dispatch (must happen before any threading) ──
    if let Some(alias) = argv0_alias() {
        match alias {
            SERVER_ALIAS => {
                // Called as `devo-server` — run the server directly.
                run_server_alias_dispatch()?;
                // Never returns normally; the server stays up until signaled.
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argv[0] alias: {other}");
                std::process::exit(1);
            }
        }
    }

    // ── Load .env from home dir (single-threaded, before any threading) ──
    load_dotenv();

    // ── Inject aliases on PATH (best‑effort) ──
    let _guard = prepend_path_for_aliases().unwrap_or_else(|err| {
        eprintln!("WARNING: could not update PATH for devo aliases: {err}");
        None
    });

    let current_exe = std::env::current_exe().ok();
    let paths = Arg0DispatchPaths {
        devo_self_exe: current_exe,
    };

    match early_dispatch(&paths) {
        EarlyDispatch::Continue => {}
        EarlyDispatch::Handled(result) => return result,
    }

    let runtime = build_runtime()?;
    let result = runtime.block_on(async move { main_fn(paths).await });
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    result
}

// ── argv[0] detection ─────────────────────────────────────────────────────

/// Returns the alias name if the process was invoked through one of our
/// symlinks or batch scripts, or `None` for a normal `devo` invocation.
fn argv0_alias() -> Option<&'static str> {
    argv0_alias_from_args(std::env::args_os())
}

fn argv0_alias_from_args<I>(args: I) -> Option<&'static str>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let argv0 = args.next().unwrap_or_default();
    let file_name = Path::new(&argv0)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Direct alias hit: `devo-server` symlink or bare name.
    if file_name == SERVER_ALIAS {
        return Some(SERVER_ALIAS);
    }

    // Windows batch scripts pass the intended alias via a sentinel arg.
    if args.next().as_deref().is_some_and(is_server_alias_sentinel) {
        return Some(SERVER_ALIAS);
    }

    None
}

fn is_server_alias_sentinel(arg: &std::ffi::OsStr) -> bool {
    arg.to_str()
        .and_then(|s| s.strip_prefix(ALIAS_SENTINEL_PREFIX))
        .is_some_and(|rest| rest == SERVER_ALIAS)
}

fn server_dispatch_args() -> Vec<OsString> {
    server_dispatch_args_from(std::env::args_os())
}

fn server_dispatch_args_from<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let mut filtered = vec![args.next().unwrap_or_else(|| OsString::from(SERVER_ALIAS))];

    if let Some(first_arg) = args.next()
        && !is_server_alias_sentinel(&first_arg)
    {
        filtered.push(first_arg);
    }
    filtered.extend(args);
    filtered
}

/// Build a multi‑thread Tokio runtime.
fn build_runtime() -> Result<tokio::runtime::Runtime> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    builder.thread_stack_size(TOKIO_WORKER_STACK_SIZE_BYTES);
    Ok(builder.build()?)
}

fn run_server_alias_dispatch() -> Result<()> {
    maybe_init_tokio_console();
    let runtime = build_runtime()?;
    runtime.block_on(run_server_dispatch());
    Ok(())
}

fn parse_server_dispatch_args() -> devo_server::ServerProcessArgs {
    use clap::Parser;
    devo_server::ServerProcessArgs::parse_from(server_dispatch_args())
}

fn install_server_logging_for_dispatch() -> Option<devo_core::LoggingRuntime> {
    let home_dir = match devo_util_paths::find_devo_home() {
        Ok(d) => d,
        Err(err) => {
            eprintln!("error: could not locate DEVO_HOME: {err}");
            std::process::exit(1);
        }
    };
    let loader = devo_core::FileSystemAppConfigLoader::new(home_dir.clone());
    use devo_core::AppConfigLoader;
    let app_config = loader.load(/*workspace_root*/ None).unwrap_or_else(|err| {
        eprintln!("warning: failed to load app config for logging: {err}");
        devo_core::AppConfig::default()
    });
    let logging = devo_core::LoggingBootstrap {
        process_name: "server",
        config: app_config.logging,
        home_dir,
    }
    .install();
    match logging {
        Ok(logging) => Some(logging),
        Err(err) => {
            eprintln!("warning: failed to install server logging: {err}");
            None
        }
    }
}

/// Run the server dispatch: parse `ServerProcessArgs` and invoke the server.
///
/// Uses `clap::Parser` directly; the `--devo-alias` sentinel has already been
/// consumed by `argv0_alias()` so the remaining args are the server's own.
async fn run_server_dispatch() {
    let args = parse_server_dispatch_args();
    let _logging = install_server_logging_for_dispatch();
    if let Err(err) =
        devo_server::run_server_process(args, devo_server::ServerProcessRunOptions::default()).await
    {
        eprintln!("server error: {err}");
        std::process::exit(1);
    }
}

// ── .env loading ───────────────────────────────────────────────────────────

const ILLEGAL_ENV_VAR_PREFIX: &str = "DEVO_";

/// Load env vars from `~/.devo/.env`.
///
/// Security: Do not allow `.env` files to create or modify any variables with
/// names starting with `DEVO_`.
fn load_dotenv() {
    let devo_home = match devo_util_paths::find_devo_home() {
        Ok(d) => d,
        Err(_) => return,
    };
    let env_path = devo_home.join(".env");
    if !env_path.exists() {
        return;
    }
    let iter = match dotenvy::from_path_iter(&env_path) {
        Ok(iter) => iter,
        Err(_) => return,
    };
    set_filtered(iter);
}

/// Set env vars from a dotenvy iterator, filtering out `DEVO_`-prefixed keys.
fn set_filtered<I>(iter: I)
where
    I: IntoIterator<Item = std::result::Result<(String, String), dotenvy::Error>>,
{
    for (key, value) in iter.into_iter().flatten() {
        // Avoid allocating an uppercased copy for every dotenv key; the
        // reserved prefix is ASCII, so byte-wise case folding is equivalent.
        let has_illegal_prefix = key
            .as_bytes()
            .get(..ILLEGAL_ENV_VAR_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(ILLEGAL_ENV_VAR_PREFIX.as_bytes()));
        if !has_illegal_prefix {
            // SAFETY: called before any threads are spawned.
            unsafe { std::env::set_var(&key, &value) };
        }
    }
}

// ── PATH injection ────────────────────────────────────────────────────────

/// Creates a temporary directory with alias entries and prepends it to `PATH`.
///
/// Returns a guard whose lifetime keeps the directory alive.
fn prepend_path_for_aliases() -> std::io::Result<Option<PathGuard>> {
    let devo_home = devo_util_paths::find_devo_home()?;
    let temp_root = devo_home.join("tmp").join(ALIAS_TEMP_ROOT);
    std::fs::create_dir_all(&temp_root)?;

    // Best-effort cleanup of stale per-session dirs.
    if let Err(err) = janitor_cleanup(&temp_root) {
        eprintln!("WARNING: failed to clean up stale arg0 temp dirs: {err}");
    }

    let temp_dir = tempfile::Builder::new()
        .prefix("devo-arg0")
        .tempdir_in(&temp_root)?;
    let path = temp_dir.path();

    // Create a lock file so janitor can detect this session is still live.
    let lock_path = path.join(LOCK_FILENAME);
    let lock_file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    // Acquire an exclusive advisory lock so the janitor (which tries a
    // non-blocking lock) knows this session is alive.
    use fs2::FileExt;
    lock_file.try_lock_exclusive()?;

    let exe = std::env::current_exe()?;

    // ── Create alias entries ──
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = path.join(SERVER_ALIAS);
        symlink(&exe, &link)?;
    }
    #[cfg(windows)]
    {
        // Each alias gets a .bat script that passes the alias name via a
        // sentinel argument.
        let batch = path.join(format!("{SERVER_ALIAS}.bat"));
        std::fs::write(
            &batch,
            format!(
                "@echo off\r\n\"{}\" --devo-alias={SERVER_ALIAS} %*\r\n",
                exe.display()
            ),
        )?;
    }

    // ── Prepend to PATH ──
    let path_separator = if cfg!(windows) { ";" } else { ":" };
    let updated_path = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut new =
                std::ffi::OsString::with_capacity(path.as_os_str().len() + 1 + existing.len());
            new.push(path);
            new.push(path_separator);
            new.push(existing);
            new
        }
        None => path.as_os_str().to_owned(),
    };
    // SAFETY: called before any threads are spawned.
    unsafe {
        std::env::set_var("PATH", updated_path);
    }

    Ok(Some(PathGuard {
        _temp_dir: temp_dir,
        _lock_file: lock_file,
    }))
}

// ── Janitor (stale directory cleanup) ──────────────────────────────────────

/// Remove stale (unlocked) session directories under `temp_root`.
fn janitor_cleanup(temp_root: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(temp_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // Skip the directory if locking fails or the lock is currently held.
        let Some(_lock_file) = try_lock_dir(&path)? else {
            continue;
        };

        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            // Expected TOCTOU race: directory can disappear after read_dir/lock checks.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            // Windows can report active locks while removing nested files even
            // after the arg0 lock was acquired. Treat those as still-live dirs.
            Err(err) if is_lock_temporarily_unavailable(&err) => continue,
            Err(err) => return Err(err),
        }
    }

    Ok(())
}

fn is_lock_temporarily_unavailable(err: &std::io::Error) -> bool {
    if err.kind() == ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(windows)]
    {
        // ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION are both normal
        // outcomes when another Windows process still owns an arg0 temp dir.
        matches!(err.raw_os_error(), Some(32 | 33))
    }

    #[cfg(not(windows))]
    {
        false
    }
}

/// Attempt to acquire an exclusive lock on a directory's lock file.
///
/// Returns `Ok(Some(File))` if the lock was acquired, `Ok(None)` if the lock
/// file doesn't exist or is held by another process.
fn try_lock_dir(dir: &Path) -> std::io::Result<Option<std::fs::File>> {
    use fs2::FileExt;

    let lock_path = dir.join(LOCK_FILENAME);
    let lock_file = match std::fs::File::options()
        .read(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) if is_lock_temporarily_unavailable(&err) => return Ok(None),
        Err(err) => return Err(err),
    };

    match lock_file.try_lock_exclusive() {
        Ok(()) => Ok(Some(lock_file)),
        Err(err) if is_lock_temporarily_unavailable(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

// ── PathGuard ──────────────────────────────────────────────────────────────

/// Keeps the temporary alias directory alive for the process lifetime.
struct PathGuard {
    _temp_dir: tempfile::TempDir,
    _lock_file: std::fs::File,
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::Path;

    use fs2::FileExt;
    use pretty_assertions::assert_eq;

    use super::LOCK_FILENAME;
    use super::janitor_cleanup;

    fn create_lock(dir: &Path) -> std::io::Result<std::fs::File> {
        let lock_path = dir.join(LOCK_FILENAME);
        std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
    }

    #[test]
    fn argv0_alias_returns_none_for_normal_invocation() {
        assert_eq!(super::argv0_alias(), None);
    }

    #[test]
    fn argv0_alias_detects_batch_sentinel() {
        let args = vec![
            OsString::from("devo.exe"),
            OsString::from("--devo-alias=devo-server"),
            OsString::from("--transport"),
            OsString::from("stdio"),
        ];

        assert_eq!(
            super::argv0_alias_from_args(args),
            Some(super::SERVER_ALIAS)
        );
    }

    #[test]
    fn server_dispatch_args_remove_batch_alias_sentinel() {
        let args = vec![
            OsString::from("devo.exe"),
            OsString::from("--devo-alias=devo-server"),
            OsString::from("--transport"),
            OsString::from("stdio"),
        ];

        assert_eq!(
            super::server_dispatch_args_from(args),
            vec![
                OsString::from("devo.exe"),
                OsString::from("--transport"),
                OsString::from("stdio")
            ]
        );
    }

    #[test]
    fn server_dispatch_args_keep_direct_alias_args() {
        let args = vec![
            OsString::from("devo-server"),
            OsString::from("--transport"),
            OsString::from("stdio"),
        ];

        assert_eq!(
            super::server_dispatch_args_from(args),
            vec![
                OsString::from("devo-server"),
                OsString::from("--transport"),
                OsString::from("stdio")
            ]
        );
    }

    #[test]
    fn lock_unavailable_detection_handles_would_block() {
        assert_eq!(
            [super::is_lock_temporarily_unavailable(
                &std::io::Error::new(ErrorKind::WouldBlock, "lock held"),
            )],
            [true]
        );
    }

    #[cfg(windows)]
    #[test]
    fn lock_unavailable_detection_handles_windows_lock_errors() {
        assert_eq!(
            [32, 33]
                .into_iter()
                .map(|code| {
                    super::is_lock_temporarily_unavailable(&std::io::Error::from_raw_os_error(code))
                })
                .collect::<Vec<_>>(),
            vec![true, true]
        );
    }

    #[test]
    fn janitor_skips_dirs_without_lock_file() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("no-lock");
        fs::create_dir(&dir)?;

        janitor_cleanup(root.path())?;

        assert!(dir.exists());
        Ok(())
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "fs2 LockFileEx allows same-process locking differently on Windows"
    )]
    fn janitor_skips_dirs_with_held_lock() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("locked");
        fs::create_dir(&dir)?;
        // On Windows, opening the file with different share modes can cause
        // LockFileEx to fail.  Open the lock file the same way
        // prepend_path_for_aliases does (shared read/write sharing).
        let lock_path = dir.join(LOCK_FILENAME);
        let lock_file = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        lock_file.try_lock_exclusive()?;

        janitor_cleanup(root.path())?;

        assert!(dir.exists());
        Ok(())
    }

    #[test]
    fn janitor_removes_dirs_with_unlocked_lock() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("stale");
        fs::create_dir(&dir)?;
        create_lock(&dir)?;

        janitor_cleanup(root.path())?;

        assert!(!dir.exists());
        Ok(())
    }
}
