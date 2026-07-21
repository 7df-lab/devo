//! Glob deny entries: detection, the macOS Seatbelt-regex translation, and the
//! Linux launch-time expansion. A deny entry is a GLOB iff it contains a glob
//! metacharacter; macOS emits an anchored runtime regex (covers files created
//! after launch), Linux expands to concrete existing matches at bwrap launch
//! (best-effort).
//!
//! Parity invariant: `validate_deny_glob` accepts/rejects identically on both
//! platforms, and the accepted subset translates the SAME on both — asserted by
//! the `macos_regex_matches_globset_property` cross-product test.

#[cfg(all(feature = "enforce", unix))]
use nono::CapabilitySet;
#[cfg(all(feature = "enforce", unix))]
use std::path::{Path, PathBuf};
// macOS regex translation reuses the parent module's alias + write-deny helpers.
#[cfg(all(feature = "enforce", target_os = "macos"))]
use super::{emit_seatbelt_deny, macos_deny_aliases};

/// Whether a raw deny entry is a glob pattern rather than an exact path. True iff
/// it contains a gitignore-style metacharacter (`*`, `?`, `[`).
#[cfg(all(feature = "enforce", unix))]
pub(crate) fn is_glob(entry: &str) -> bool {
    entry.contains(['*', '?', '['])
}

/// Split a profile's raw deny entries into exact paths (handled by the literal /
/// subpath kernel-deny flow) and glob patterns. Non-glob entries are returned
/// unchanged so their exact-path enforcement is preserved with no regression.
#[cfg(all(feature = "enforce", unix))]
pub(crate) fn partition_deny_entries(deny: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let mut exact = Vec::new();
    let mut globs = Vec::new();
    for entry in deny {
        match entry.to_str() {
            Some(s) if is_glob(s) => globs.push(s.to_string()),
            _ => exact.push(entry.clone()),
        }
    }
    (exact, globs)
}

/// Split a glob into its literal root directory and the glob tail (from the first
/// component containing a metacharacter onward). Relative globs root at
/// `workspace` (recursive `**` allowed); absolute globs root at their leading
/// non-glob components (e.g. `/home/**/.ssh` -> root `/home`, tail `**/.ssh`).
#[cfg(all(feature = "enforce", unix))]
fn split_glob_root(workspace: &Path, glob: &str) -> (PathBuf, String) {
    let Some(abs) = glob.strip_prefix('/') else {
        return (workspace.to_path_buf(), glob.to_string());
    };
    let mut root = PathBuf::from("/");
    let mut tail: Vec<&str> = Vec::new();
    let mut in_tail = false;
    for comp in abs.split('/') {
        if in_tail {
            tail.push(comp);
        } else if is_glob(comp) {
            in_tail = true;
            tail.push(comp);
        } else if !comp.is_empty() {
            root.push(comp);
        }
    }
    (root, tail.join("/"))
}

/// Validate a deny glob on BOTH platforms so a given pattern is interpreted
/// IDENTICALLY everywhere or rejected everywhere (never silently under-enforced
/// on macOS). Two checks, run before the macOS regex translation and the Linux
/// globset expansion alike:
///
/// 1. Reject `{`/`}`/`\`: globset honors brace alternation and backslash-escapes,
///    but Seatbelt's runtime regex (sourced from globset's own `.regex()` mis-
///    enforces `**/` for root-level paths, so we hand-roll the regex instead and
///    cannot faithfully reproduce those forms — rejecting them on both platforms
///    keeps the two backends in agreement. A user wanting alternation writes
///    separate deny entries.
/// 2. Compile through `globset` (the Linux matcher) so a malformed glob (`a**b`,
///    unterminated `[`) fails closed identically on both platforms.
#[cfg(all(feature = "enforce", unix))]
pub(crate) fn validate_deny_glob(glob: &str) -> anyhow::Result<()> {
    if let Some(c) = glob.chars().find(|&c| matches!(c, '{' | '}' | '\\')) {
        anyhow::bail!(
            "deny glob {glob:?} uses unsupported metacharacter '{c}' \
             (brace alternation and backslash-escapes are not supported; \
             use separate deny entries)"
        );
    }
    // `**` must be a whole path component (gitignore semantics). A non-component
    // `**` (e.g. `a**b`) would translate to `.*` on macOS but collapse to `*` in
    // globset — reject it on both platforms so they never diverge.
    for comp in glob.split('/') {
        if comp.contains("**") && comp != "**" {
            anyhow::bail!(
                "deny glob {glob:?}: `**` must be its own path component (got segment {comp:?})"
            );
        }
    }
    // Char classes: support only the simple subset that translates identically to
    // globset. Reject a literal `]`-first member (`[]a]`) and any nested `[` —
    // which covers POSIX `[[:…:]]` — since globset and the hand-rolled regex parse
    // those differently. (A leading `!`/`^` negation IS supported.)
    let cc: Vec<char> = glob.chars().collect();
    let mut i = 0;
    while i < cc.len() {
        if cc[i] != '[' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if matches!(cc.get(j), Some('!') | Some('^')) {
            j += 1;
        }
        if cc.get(j) == Some(&']') {
            anyhow::bail!("deny glob {glob:?}: a literal ']' as first class member is unsupported");
        }
        while j < cc.len() && cc[j] != ']' {
            if cc[j] == '[' {
                anyhow::bail!(
                    "deny glob {glob:?}: nested '[' / POSIX '[[:…:]]' classes are unsupported"
                );
            }
            j += 1;
        }
        // Unterminated class: let the globset build below report it uniformly.
        i = if j < cc.len() { j + 1 } else { cc.len() };
    }
    globset::GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .map_err(|e| anyhow::anyhow!("invalid deny glob {glob:?}: {e}"))?;
    Ok(())
}

/// Push `c` as a regex literal, escaping it when it is a regex metacharacter.
#[cfg(all(feature = "enforce", target_os = "macos"))]
fn push_escaped_regex_literal(out: &mut String, c: char) {
    if matches!(
        c,
        '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
    ) {
        out.push('\\');
    }
    out.push(c);
}

/// Regex-escape every character of a literal path segment.
#[cfg(all(feature = "enforce", target_os = "macos"))]
fn escape_regex_literal_str(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        push_escaped_regex_literal(&mut out, c);
    }
    out
}

/// Translate a gitignore-style glob tail into an (unanchored) Seatbelt regex
/// body. Dialect: `**/`->`(.*/)?`, `**`->`.*`, `*`->`[^/]*`, `?`->`[^/]`,
/// `[...]` classes copied with a leading `!`/`^` -> regex negation `[^…]`, all
/// other literal text regex-escaped. Only the class subset `validate_deny_glob`
/// accepts reaches here, so it always matches globset.
#[cfg(all(feature = "enforce", target_os = "macos"))]
fn glob_tail_to_regex(tail: &str) -> String {
    let mut out = String::new();
    let mut chars = tail.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        out.push_str("(.*/)?"); // `**/` spans zero or more dirs
                    } else {
                        out.push_str(".*"); // `**` spans anything, incl. `/`
                    }
                } else {
                    out.push_str("[^/]*"); // `*` stops at a path separator
                }
            }
            '?' => out.push_str("[^/]"),
            '[' => {
                out.push('[');
                // globset treats a leading `!` OR `^` as negation -> regex `[^…]`
                // (validate_deny_glob has rejected the class forms that would drift).
                if matches!(chars.peek(), Some('!') | Some('^')) {
                    chars.next();
                    out.push('^');
                }
                while let Some(cc) = chars.next() {
                    if cc == ']' {
                        break;
                    }
                    // Backslash-escapes are rejected by validate_deny_glob; this
                    // passthrough stays defensive.
                    if cc == '\\' {
                        out.push('\\');
                        if let Some(n) = chars.next() {
                            out.push(n);
                        }
                    } else {
                        out.push(cc);
                    }
                }
                out.push(']');
            }
            c => push_escaped_regex_literal(&mut out, c),
        }
    }
    out
}

/// Anchored Seatbelt regex bodies for one glob — one per macOS alias form of the
/// glob's root (workspace for relative, literal prefix for absolute) so the broad
/// read-allow cannot be bypassed via the `/private` firmlink alias.
#[cfg(all(feature = "enforce", target_os = "macos"))]
fn glob_to_seatbelt_regexes(workspace: &Path, glob: &str) -> Vec<String> {
    let (root, tail) = split_glob_root(workspace, glob);
    let tail_regex = glob_tail_to_regex(&tail);
    let canonical_root = dunce::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let mut regexes = Vec::new();
    for form in macos_deny_aliases(&root, &canonical_root) {
        let Some(form_str) = form.to_str() else {
            continue;
        };
        let escaped_root = escape_regex_literal_str(form_str);
        // Avoid a double slash when the root is `/`.
        let sep = if escaped_root.ends_with('/') { "" } else { "/" };
        regexes.push(format!("^{escaped_root}{sep}{tail_regex}$"));
    }
    regexes
}

/// Wrap a finished regex body in a Seatbelt `(regex #"…")` filter, escaping the
/// SBPL string delimiter and rejecting control chars. Fail-closed: returns
/// `None` for an inexpressible pattern so the caller errors rather than emitting
/// a rule that silently targets the wrong path.
#[cfg(all(feature = "enforce", target_os = "macos"))]
fn seatbelt_regex_filter(regex: &str) -> Option<String> {
    if regex.chars().any(|c| c.is_control()) {
        return None;
    }
    let escaped = regex.replace('"', "\\\"");
    Some(format!("(regex #\"{escaped}\")"))
}

/// Apply kernel-level deny rules for glob patterns.
///
/// On macOS, translate each glob to an anchored Seatbelt regex and emit the same
/// read + per-write-sub-action denies as the exact-path flow (so `mv x y && cat y`
/// stays closed), covering files created after launch. On Linux this is a no-op:
/// a mount namespace can't match a regex at runtime, so globs are expanded to
/// concrete paths and bound over at bwrap re-exec (see [`expand_deny_globs`]).
///
/// Unlike the exact-path flow, this does NOT call `remove_exact_file_caps_for_paths`
/// (a glob can't enumerate the file caps it collides with); glob denies rely on
/// Seatbelt last-match ordering — the deny platform rules are emitted after the
/// read/write allows, so the regex deny wins. The e2e is the contract.
#[cfg(all(feature = "enforce", unix))]
pub(crate) fn apply_deny_globs_to_capability_set(
    caps: &mut CapabilitySet,
    workspace: &Path,
    globs: &[String],
) -> anyhow::Result<()> {
    if globs.is_empty() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        for glob in globs {
            // Fail CLOSED on any glob that isn't expressible identically on both
            // platforms (braces/backslash) or is malformed — same check Linux runs.
            validate_deny_glob(glob)?;
            let regexes = glob_to_seatbelt_regexes(workspace, glob);
            if regexes.is_empty() {
                // Fail CLOSED: a glob we can't anchor would be silently
                // unprotected while the sandbox still reports active.
                anyhow::bail!("cannot translate deny glob {glob:?} to a Seatbelt regex");
            }
            for regex in regexes {
                let Some(filter) = seatbelt_regex_filter(&regex) else {
                    anyhow::bail!("cannot express deny glob {glob:?} as a Seatbelt regex filter");
                };
                emit_seatbelt_deny(caps, &filter)?;
            }
        }
        tracing::info!(
            count = globs.len(),
            "Applied Seatbelt deny regexes for sandbox deny globs"
        );
    }

    #[cfg(target_os = "linux")]
    {
        let _ = (caps, workspace);
        tracing::debug!(
            count = globs.len(),
            "Linux deny globs are expanded and bound over at bwrap re-exec (launch-time)"
        );
    }

    Ok(())
}

/// Caps for launch-time deny-glob expansion on Linux. A mount namespace can't
/// glob at runtime, so globs are expanded to existing matches once at launch;
/// these bounds stop a broad glob (e.g. `**/*`) from exploding the bind list or
/// taking an unbounded walk. Exceeding either fails closed (see [`expand_deny_globs`]).
#[cfg(all(feature = "enforce", target_os = "linux"))]
pub(crate) const DENY_GLOB_MAX_DEPTH: usize = 64;
#[cfg(all(feature = "enforce", target_os = "linux"))]
pub(crate) const DENY_GLOB_MAX_MATCHES: usize = 4096;
/// Total tree entries the walk may visit before failing closed. Bounds launch
/// latency on large repos (`max_matches` caps matches, not entries visited) so a
/// broad glob that matches little still can't walk an unbounded tree each launch.
#[cfg(all(feature = "enforce", target_os = "linux"))]
pub(crate) const DENY_GLOB_MAX_ENTRIES: usize = 200_000;

/// Classify a walk error hit while expanding deny globs. A permission error means
/// the same-uid agent is equally denied by the kernel, so skipping that subtree
/// exposes nothing; any other error (transient IO, fd exhaustion, a race) could
/// hide a readable match, so it is fatal and we fail closed rather than under-enforce.
#[cfg(all(feature = "enforce", target_os = "linux"))]
fn deny_glob_walk_error_is_fatal(err: &ignore::Error) -> bool {
    match err.io_error() {
        Some(io) => io.kind() != std::io::ErrorKind::PermissionDenied,
        None => true,
    }
}

/// Expand deny GLOBS into the concrete EXISTING paths that match, for the Linux
/// bwrap bind-over. Relative globs anchor at `workspace`; absolute globs at their
/// literal (non-glob) prefix. The walk DISABLES gitignore/hidden filters (a
/// denied secret like `.env` or `*.pem` is usually both) and never follows
/// symlinks (a symlink must not smuggle its target into the deny set).
///
/// Returns `None` (fail closed) if a glob is invalid, the walk is truncated by
/// `max_depth`, more than `max_entries` are visited, matches exceed `max_matches`,
/// or the walk hits a non-permission error — so the caller refuses to start rather
/// than under-enforcing or exploding the bind list. A permission error is skipped
/// (the same-uid agent is equally OS-denied). Each fail-closed cause is logged so
/// the refusal names the glob (not the generic "install bubblewrap" path).
///
/// Best-effort: files created AFTER launch that match a glob are NOT covered on
/// Linux. macOS Seatbelt enforces the same globs as runtime regexes, so they are.
#[cfg(all(feature = "enforce", target_os = "linux"))]
pub(crate) fn expand_deny_globs(
    workspace: &Path,
    globs: &[String],
    max_depth: usize,
    max_matches: usize,
    max_entries: usize,
) -> Option<Vec<String>> {
    use globset::{GlobBuilder, GlobSetBuilder};
    use ignore::WalkBuilder;
    use std::collections::BTreeSet;

    // Log + surface the real reason before fail-closing so the shell's refusal is
    // not misattributed to a missing bubblewrap.
    let fail = |reason: String| -> Option<Vec<String>> {
        tracing::error!(%reason, "sandbox deny-glob expansion failed; refusing to start");
        eprintln!("error: sandbox deny glob could not be enforced on Linux: {reason}");
        None
    };

    let ws = workspace.to_string_lossy().into_owned();
    let mut builder = GlobSetBuilder::new();
    let mut roots: BTreeSet<PathBuf> = BTreeSet::new();
    for glob in globs {
        // Same validation macOS runs, so a malformed/unsupported glob fails closed
        // identically on both platforms.
        if let Err(e) = validate_deny_glob(glob) {
            return fail(e.to_string());
        }
        // Match against absolute paths: relative globs get the (escaped) workspace
        // prefix; absolute globs are used as-is. `literal_separator(true)` =>
        // `*`/`?` stop at `/` (gitignore-style), matching the macOS translation.
        let pattern = if glob.starts_with('/') {
            glob.clone()
        } else {
            format!("{}/{}", globset::escape(&ws), glob)
        };
        let Ok(compiled) = GlobBuilder::new(&pattern).literal_separator(true).build() else {
            return fail(format!("could not compile glob {glob:?}"));
        };
        builder.add(compiled);
        roots.insert(split_glob_root(workspace, glob).0);
    }
    let Ok(set) = builder.build() else {
        return fail("could not build glob set".to_string());
    };

    let mut matches: BTreeSet<String> = BTreeSet::new();
    let mut visited: usize = 0;
    for root in roots {
        if !root.exists() {
            continue;
        }
        let walker = WalkBuilder::new(&root)
            .max_depth(Some(max_depth))
            .standard_filters(false)
            .hidden(false)
            .follow_links(false)
            .build();
        for dent in walker {
            let dent = match dent {
                Ok(dent) => dent,
                Err(e) => {
                    // Hidden subtree: skip OS-enforced permission errors, fail closed on anything else.
                    if deny_glob_walk_error_is_fatal(&e) {
                        return fail(format!("walk error under a deny-glob root: {e}"));
                    }
                    tracing::warn!(error = %e, "skipping unreadable entry during deny-glob walk");
                    continue;
                }
            };
            visited += 1;
            if visited > max_entries {
                return fail(format!(
                    "walk visited over {max_entries} entries (glob too broad)"
                ));
            }
            // A directory at the depth cap may hide deeper matches we cannot see;
            // fail closed rather than silently under-enforce.
            if dent.depth() >= max_depth && dent.file_type().is_some_and(|ft| ft.is_dir()) {
                return fail(format!("tree deeper than the {max_depth}-level depth cap"));
            }
            let path = dent.path();
            if set.is_match(path) {
                // A non-UTF8 match can't be bound by a string path. Fail closed
                // (like the exact-path Seatbelt flow) rather than skip it and leave
                // a matching secret readable while the sandbox reports active.
                let Some(s) = path.to_str() else {
                    return fail(format!("deny-glob match has a non-UTF8 path: {path:?}"));
                };
                matches.insert(s.to_owned());
                if matches.len() > max_matches {
                    return fail(format!("matched over {max_matches} files (glob too broad)"));
                }
            }
        }
    }
    Some(matches.into_iter().collect())
}

#[cfg(test)]
mod tests {
    // All tests here exercise enforce+unix paths; without the gate `super::*`
    // is unused on `--no-default-features`.
    #[cfg(all(feature = "enforce", unix))]
    use super::*;
    #[cfg(all(feature = "enforce", unix))]
    use pretty_assertions::assert_eq;

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn is_glob_detects_metacharacters() {
        assert!(is_glob("**/.env"));
        assert!(is_glob("**/*.pem"));
        assert!(is_glob("secrets/**"));
        assert!(is_glob("a?b"));
        assert!(is_glob("[abc].txt"));
        // Exact paths must NOT be treated as globs (no regression in literal deny).
        assert!(!is_glob(".env"));
        assert!(!is_glob("src/server.pem"));
        assert!(!is_glob("/etc/shadow"));
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn partition_separates_globs_from_exact_paths() {
        let deny = vec![
            PathBuf::from(".env"),
            PathBuf::from("**/*.pem"),
            PathBuf::from("/etc/shadow"),
            PathBuf::from("secrets/**"),
        ];
        let (exact, globs) = partition_deny_entries(&deny);
        assert_eq!(
            exact,
            vec![PathBuf::from(".env"), PathBuf::from("/etc/shadow")]
        );
        assert_eq!(
            globs,
            vec!["**/*.pem".to_string(), "secrets/**".to_string()]
        );
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn split_glob_root_relative_vs_absolute() {
        let ws = Path::new("/ws");
        assert_eq!(
            split_glob_root(ws, "**/.env"),
            (PathBuf::from("/ws"), "**/.env".to_string())
        );
        assert_eq!(
            split_glob_root(ws, "/home/**/.ssh"),
            (PathBuf::from("/home"), "**/.ssh".to_string())
        );
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn glob_tail_translates_to_seatbelt_regex() {
        assert_eq!(glob_tail_to_regex("**/.env"), "(.*/)?\\.env");
        assert_eq!(glob_tail_to_regex("**/*.pem"), "(.*/)?[^/]*\\.pem");
        assert_eq!(glob_tail_to_regex("secrets/**"), "secrets/.*");
        assert_eq!(glob_tail_to_regex("*.key"), "[^/]*\\.key");
        assert_eq!(glob_tail_to_regex("a?b"), "a[^/]b");
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn glob_tail_translates_char_classes() {
        assert_eq!(glob_tail_to_regex("[abc].txt"), "[abc]\\.txt");
        assert_eq!(glob_tail_to_regex("[a-z].rs"), "[a-z]\\.rs");
        // A leading `!` OR `^` is NEGATION in globset -> regex `[^…]` (both must
        // produce the SAME negated class, else macOS under-matches).
        assert_eq!(glob_tail_to_regex("[!a]b"), "[^a]b");
        assert_eq!(glob_tail_to_regex("[^a]b"), "[^a]b");
    }

    #[test]
    #[cfg(all(feature = "enforce", unix))]
    fn validate_deny_glob_accepts_subset_rejects_rest() {
        // Supported subset (`*`, `?`, `**`, `[...]` incl. `[!a]`/`[^a]` negation).
        for g in [
            "**/*.pem",
            "**/.env",
            "secrets/**",
            "[abc].txt",
            "[a-z].rs",
            "[!a]b",
            "[^a]b",
            "a?b",
            "/home/**/.ssh",
        ] {
            assert!(validate_deny_glob(g).is_ok(), "{g} should be supported");
        }
        // Braces + backslash drift macOS vs globset -> rejected (fail closed) on BOTH.
        for g in ["**/*.{pem,key}", "a\\*b", "{a,b}"] {
            assert!(validate_deny_glob(g).is_err(), "{g} should be rejected");
        }
        // Char-class forms that parse differently in globset vs the regex engine.
        for g in ["[]a]", "[[:alpha:]]", "[a[b]"] {
            assert!(
                validate_deny_glob(g).is_err(),
                "{g} unsupported char class should be rejected"
            );
        }
        // Malformed globs fail closed identically to the Linux globset backend.
        for g in ["a**b", "**a", "[abc"] {
            assert!(
                validate_deny_glob(g).is_err(),
                "{g} should be rejected as malformed"
            );
        }
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn glob_to_regex_doubles_private_aliased_root() {
        // A workspace under /tmp (firmlinked to /private/tmp) must emit a deny
        // regex for BOTH alias roots, else the broad read-allow leaks via the alias.
        let regexes = glob_to_seatbelt_regexes(Path::new("/tmp/projalias"), "**/.env");
        assert!(
            regexes.contains(&"^/tmp/projalias/(.*/)?\\.env$".to_string()),
            "{regexes:?}"
        );
        assert!(
            regexes.contains(&"^/private/tmp/projalias/(.*/)?\\.env$".to_string()),
            "{regexes:?}"
        );
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn macos_regex_matches_globset_property() {
        // PARITY GUARD (cross-product): for EVERY pattern `validate_deny_glob`
        // accepts, the hand-rolled macOS regex must match a path IFF globset (the
        // Linux backend) matches it. Generated from building blocks (literals,
        // regex-metachar literal, `*`, `?`, `**`, char classes incl. both
        // negations, ranges, class-content edge cases, and a `]` outside a class)
        // crossed with sample paths, so any future dialect drift fails
        // mechanically. Rejected forms are asserted to fail closed.
        let segs = [
            "a", "x.y", "*", "?", "**", "[abc]", "[a-z]", "[!a]", "[^a]", "[.]", "[*]", "[a^]",
            "[a-]", "[-a]", "*]",
        ];
        let paths = [
            "a",
            "ab",
            "x",
            "x.y",
            "xay",
            "^",
            "!",
            ".",
            "-",
            "*",
            "b",
            "a/b",
            "ab/cd",
            "sub/a",
            "sub/dir/a",
            ".env",
            "sub/.env",
            "k.pem",
            "foo.env",
            ".envrc",
            "secrets/x",
            "]",
            "a]",
        ];
        // Build single- and two-segment patterns from the blocks.
        let mut patterns: Vec<String> = Vec::new();
        for a in segs {
            patterns.push(a.to_string());
            for b in segs {
                patterns.push(format!("{a}/{b}"));
            }
        }
        for p in &patterns {
            if validate_deny_glob(p).is_err() {
                continue; // rejected patterns aren't enforced on either platform
            }
            let regexes = glob_to_seatbelt_regexes(Path::new("/ws"), p);
            assert_eq!(regexes.len(), 1, "expected one regex for {p:?}");
            let re = regex::Regex::new(&regexes[0]).unwrap_or_else(|e| panic!("{p:?}: {e}"));
            let gs = globset::GlobBuilder::new(&format!("/ws/{p}"))
                .literal_separator(true)
                .build()
                .unwrap()
                .compile_matcher();
            for path in paths {
                let abs = format!("/ws/{path}");
                assert_eq!(
                    re.is_match(&abs),
                    gs.is_match(&abs),
                    "DRIFT for pattern {p:?} on {abs:?}: macos={}, globset={}",
                    re.is_match(&abs),
                    gs.is_match(&abs)
                );
            }
        }
        // Forms that MUST fail closed on both platforms (cannot translate identically).
        for bad in [
            "[]a]",
            "[[:alpha:]]",
            "{a,b}",
            "**/*.{pem,key}",
            "a**b",
            "**a",
            "[abc",
        ] {
            assert!(validate_deny_glob(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn glob_to_regex_anchors_relative_at_workspace() {
        // A non-existent workspace cannot canonicalize/alias, so exactly one
        // anchored regex is produced.
        let regexes = glob_to_seatbelt_regexes(Path::new("/ws-does-not-exist-xyz"), "**/*.pem");
        assert_eq!(
            regexes,
            vec!["^/ws-does-not-exist-xyz/(.*/)?[^/]*\\.pem$".to_string()]
        );
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn glob_to_regex_roots_absolute_at_literal_prefix() {
        let regexes =
            glob_to_seatbelt_regexes(Path::new("/ws-does-not-exist-xyz"), "/nope-xyz/**/.ssh");
        assert_eq!(regexes, vec!["^/nope-xyz/(.*/)?\\.ssh$".to_string()]);
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn seatbelt_regex_filter_wraps_and_rejects_control_chars() {
        assert_eq!(
            seatbelt_regex_filter("^/ws/(.*/)?\\.env$").unwrap(),
            "(regex #\"^/ws/(.*/)?\\.env$\")"
        );
        assert!(seatbelt_regex_filter("a\u{07}b").is_none());
    }

    #[test]
    #[cfg(all(feature = "enforce", target_os = "macos"))]
    fn apply_deny_globs_emits_nono_accepted_rules() {
        // Every emitted `(deny … (regex …))` must pass nono's validate_platform_rule.
        let mut caps = CapabilitySet::new();
        let globs = vec![
            "**/*.pem".to_string(),
            "**/.env".to_string(),
            "secrets/**".to_string(),
        ];
        apply_deny_globs_to_capability_set(&mut caps, Path::new("/ws-does-not-exist-xyz"), &globs)
            .expect("emitted Seatbelt deny regexes must be accepted by nono");
    }

    // Linux launch-time expansion + its fail-closed caps. Gated to enforce+linux,
    // so they run on the Linux CI lane (not on macOS).
    #[cfg(all(feature = "enforce", target_os = "linux"))]
    mod linux_expand {
        use super::*;

        struct TmpTree(PathBuf);
        impl Drop for TmpTree {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        fn tmp_tree(tag: &str) -> PathBuf {
            let p = std::env::temp_dir().join(format!(
                "deny-glob-ut-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            p
        }

        #[test]
        fn matches_nested_pem_and_dotenv_excludes_control() {
            let ws = tmp_tree("match");
            let _g = TmpTree(ws.clone());
            std::fs::create_dir_all(ws.join("sub/dir")).unwrap();
            std::fs::write(ws.join("sub/dir/key.pem"), "x").unwrap();
            std::fs::write(ws.join(".env"), "x").unwrap(); // hidden + usually gitignored
            std::fs::write(ws.join("readable.txt"), "x").unwrap();
            let globs = vec!["**/*.pem".to_string(), "**/.env".to_string()];
            let out = expand_deny_globs(&ws, &globs, 64, 4096, 200_000).expect("should expand");
            assert!(
                out.iter().any(|p| p.ends_with("sub/dir/key.pem")),
                "{out:?}"
            );
            assert!(out.iter().any(|p| p.ends_with("/.env")), "{out:?}");
            assert!(!out.iter().any(|p| p.ends_with("readable.txt")), "{out:?}");
        }

        #[test]
        fn empty_when_nothing_matches() {
            let ws = tmp_tree("empty");
            let _g = TmpTree(ws.clone());
            std::fs::write(ws.join("a.txt"), "x").unwrap();
            let out = expand_deny_globs(&ws, &["**/*.pem".to_string()], 64, 4096, 200_000).unwrap();
            assert!(out.is_empty(), "{out:?}");
        }

        #[test]
        fn fails_closed_on_match_cap() {
            let ws = tmp_tree("matchcap");
            let _g = TmpTree(ws.clone());
            for i in 0..5 {
                std::fs::write(ws.join(format!("k{i}.pem")), "x").unwrap();
            }
            assert!(expand_deny_globs(&ws, &["**/*.pem".to_string()], 64, 2, 200_000).is_none());
        }

        #[test]
        fn fails_closed_on_depth_cap() {
            let ws = tmp_tree("depthcap");
            let _g = TmpTree(ws.clone());
            std::fs::create_dir_all(ws.join("a/b/c")).unwrap();
            std::fs::write(ws.join("a/b/c/k.pem"), "x").unwrap();
            // A directory sits at the depth cap -> deeper matches could hide -> None.
            assert!(expand_deny_globs(&ws, &["**/*.pem".to_string()], 1, 4096, 200_000).is_none());
        }

        #[test]
        fn fails_closed_on_entries_cap() {
            let ws = tmp_tree("entriescap");
            let _g = TmpTree(ws.clone());
            for i in 0..10 {
                std::fs::write(ws.join(format!("f{i}.txt")), "x").unwrap();
            }
            // Broad walk, nothing matches, but the entries cap still fails closed.
            assert!(expand_deny_globs(&ws, &["**/*.pem".to_string()], 64, 4096, 3).is_none());
        }

        #[test]
        fn rejects_unsupported_glob() {
            let ws = tmp_tree("reject");
            let _g = TmpTree(ws.clone());
            // Braces are rejected identically to macOS (fail closed).
            assert!(
                expand_deny_globs(&ws, &["**/*.{pem,key}".to_string()], 64, 4096, 200_000)
                    .is_none()
            );
        }

        #[test]
        fn does_not_descend_symlinked_dir() {
            let ws = tmp_tree("symlink");
            let _g = TmpTree(ws.clone());
            let outside = tmp_tree("symlink-outside");
            let _g2 = TmpTree(outside.clone());
            std::fs::write(outside.join("secret.pem"), "x").unwrap();
            std::os::unix::fs::symlink(&outside, ws.join("link")).unwrap();
            // follow_links(false): the symlink must not smuggle its target in.
            let out = expand_deny_globs(&ws, &["**/*.pem".to_string()], 64, 4096, 200_000).unwrap();
            assert!(
                !out.iter().any(|p| p.contains("secret.pem")),
                "symlinked dir must not be descended: {out:?}"
            );
        }

        #[test]
        fn walk_error_permission_skipped_others_fatal() {
            use std::io;
            // EACCES on a dir: the same-uid agent is equally OS-denied -> skip (non-fatal).
            let perm = ignore::Error::from(io::Error::from(io::ErrorKind::PermissionDenied));
            assert!(!deny_glob_walk_error_is_fatal(&perm));
            // A non-permission IO error could hide a readable match -> fatal (fail closed).
            let other = ignore::Error::from(io::Error::other("boom"));
            assert!(deny_glob_walk_error_is_fatal(&other));
            // A non-IO walk error (e.g. a symlink loop) has no io_error -> fatal.
            let loop_err = ignore::Error::Loop {
                ancestor: PathBuf::from("/a"),
                child: PathBuf::from("/a/b"),
            };
            assert!(deny_glob_walk_error_is_fatal(&loop_err));
        }

        #[test]
        fn fails_closed_on_non_utf8_match() {
            use std::os::unix::ffi::OsStrExt;
            let ws = tmp_tree("nonutf8");
            let _g = TmpTree(ws.clone());
            // A filename with an invalid UTF-8 byte that still matches `*.pem`.
            let name = std::ffi::OsStr::from_bytes(b"secret\xFF.pem");
            std::fs::write(ws.join(name), "x").unwrap();
            // The match can't be expressed as a UTF-8 bind path -> fail closed (None).
            assert!(expand_deny_globs(&ws, &["**/*.pem".to_string()], 64, 4096, 200_000).is_none());
        }
    }
}
