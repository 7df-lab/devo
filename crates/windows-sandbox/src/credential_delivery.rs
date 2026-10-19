//! Per-session credential authority. Devo-owned: codex's per-exec model never
//! needs to deliver credentials to a long-lived sandboxed process, so this
//! layer cannot live upstream (see `DEVO_PATCHES.md`).
//!
//! A session-scoped capability SID is minted once per kernel session and, when
//! the kernel is fenced, travels in its restricted token. Granting a path =
//! adding an inheritable allow ACE for that SID (raw `open()` works
//! immediately, no kernel restart); revoking = removing the ACE (new opens are
//! refused; already-open handles keep working — a known residual).
//! Every grant/revoke is journaled to `<devo_home>/.sandbox/credential_journal.json`
//! so crash recovery can re-derive deliveries. The journal is the authority
//! for recovery — not just an audit trail.
//!
//! Startup orphan cleanup is not called automatically: this journal has no
//! cross-process liveness markers, and one server could otherwise revoke the
//! live credentials of an independently spawned kernel using the same home.
//! Elevated-account tokens also need access in the normal account SID pass,
//! not only this restricting SID pass; keep mediated approval fallback for
//! roots the sandbox account cannot otherwise access.

use crate::acl::add_allow_ace;
use crate::acl::ensure_allow_mask_aces_with_inheritance;
use crate::acl::revoke_ace_checked;
use crate::setup::sandbox_dir;
use crate::token::LocalSid;
use anyhow::Result;
use rand::RngCore;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;

/// Inheritance flags (mirroring acl.rs's private constants): delivered ACEs
/// propagate to the granted subtree.
const CONTAINER_INHERIT_ACE: u32 = 0x2;
const OBJECT_INHERIT_ACE: u32 = 0x1;

pub struct SessionCredentialAuthority {
    sid_string: String,
    journal_path: PathBuf,
    session_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CredentialJournal {
    /// session_id → credential record. The SID is journaled alongside the
    /// grants because revocation is impossible without it, and crash recovery
    /// (not just audit) reads this file.
    sessions: BTreeMap<String, SessionRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SessionRecord {
    sid: String,
    #[serde(default)]
    grants: Vec<GrantRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GrantRecord {
    root: PathBuf,
    /// "read" | "write" — which mask the delivered ACE carries.
    access: String,
}

/// Access class of a delivered credential ACE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialAccess {
    Read,
    Write,
}

impl CredentialAccess {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// Canonical read mask for delivered read credentials (mirrors
/// `FileSystemAccessMode::Read` in acl.rs).
const GENERIC_READ_MASK: u32 = 0x8000_0000;

fn make_random_session_sid() -> String {
    let mut rng = SmallRng::from_entropy();
    let a = rng.next_u32();
    let b = rng.next_u32();
    let c = rng.next_u32();
    let d = rng.next_u32();
    format!("S-1-5-21-{a}-{b}-{c}-{d}")
}

fn journal_path_for(devo_home: &Path) -> PathBuf {
    sandbox_dir(devo_home).join("credential_journal.json")
}

fn load_journal(path: &Path) -> Result<CredentialJournal> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CredentialJournal::default());
        }
        Err(err) => return Err(err.into()),
    };
    // A truncated or unreadable journal must never be mistaken for an empty
    // journal: that would discard the only record of previously delivered ACEs.
    Ok(serde_json::from_slice(&bytes)?)
}

fn persist_journal(path: &Path, journal: &CredentialJournal) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("journal has no parent"))?;
    fs::create_dir_all(dir)?;
    // Never truncate the authority journal in place. A crash or full disk
    // during serialization would otherwise erase all earlier ACE records.
    let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut temporary, journal)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

impl SessionCredentialAuthority {
    /// Mint a session credential authority. Does not touch any DACL yet; the
    /// journal gains its session row on the first grant.
    pub fn new(session_id: &str, devo_home: &Path) -> Result<Self> {
        let sid_string = make_random_session_sid();
        // Validate the SID shape eagerly; store only the string so this type
        // stays Send+Sync (LocalSid wraps a raw pointer and is neither).
        LocalSid::from_string(&sid_string)?;
        Ok(Self {
            sid_string,
            journal_path: journal_path_for(devo_home),
            session_id: session_id.to_string(),
        })
    }

    /// Transient SID handle for one ACE operation.
    fn local_sid(&self) -> Result<LocalSid> {
        LocalSid::from_string(&self.sid_string)
    }

    pub fn sid(&self) -> &str {
        &self.sid_string
    }

    /// Grant write access to `root` for this session: inheritable allow ACE on
    /// the root (Windows propagation covers descendants), journaled. Idempotent
    /// — an already-granted root is a no-op. Returns `true` when a new ACE was
    /// applied.
    pub fn grant_write_root(&self, root: &Path) -> Result<bool> {
        self.grant_root(root, CredentialAccess::Write)
    }

    /// Grant read access to `root` (read-mask inheritable allow ACE), journaled
    /// and idempotent, same contract as [`grant_write_root`].
    pub fn grant_read_root(&self, root: &Path) -> Result<bool> {
        self.grant_root(root, CredentialAccess::Read)
    }

    fn grant_root(&self, root: &Path, access: CredentialAccess) -> Result<bool> {
        let mut journal = load_journal(&self.journal_path)?;
        let record = journal
            .sessions
            .entry(self.session_id.clone())
            .or_insert_with(|| SessionRecord {
                sid: self.sid_string.clone(),
                grants: Vec::new(),
            });
        // Journal before touching the ACL. If delivery fails or the process
        // crashes between these steps, the row still allows a later retry or
        // cleanup. Retrying an already-journaled entry rechecks the DACL.
        if !record
            .grants
            .iter()
            .any(|g| g.root == root && g.access == access.as_str())
        {
            record.grants.push(GrantRecord {
                root: root.to_path_buf(),
                access: access.as_str().to_string(),
            });
            persist_journal(&self.journal_path, &journal)?;
        }
        let sid = self.local_sid()?;
        let applied = match access {
            CredentialAccess::Write => unsafe { add_allow_ace(root, sid.as_ptr()) }?,
            CredentialAccess::Read => unsafe {
                ensure_allow_mask_aces_with_inheritance(
                    root,
                    &[sid.as_ptr()],
                    FILE_GENERIC_READ | GENERIC_READ_MASK,
                    CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
                )?
            },
        };
        Ok(applied)
    }

    /// Revoke one granted root. Keep its journal entry if the ACL operation
    /// fails so the next cleanup can retry it.
    pub fn revoke_write_root(&self, root: &Path) -> Result<()> {
        let mut journal = load_journal(&self.journal_path)?;
        let Some(record) = journal.sessions.get_mut(&self.session_id) else {
            return Ok(());
        };
        let sid = self.local_sid()?;
        let (_, errors) = revoke_record_grants(
            record,
            |grant| grant.root == root,
            |path| unsafe { revoke_ace_checked(path, sid.as_ptr()) },
        );
        if record.grants.is_empty() {
            journal.sessions.remove(&self.session_id);
        }
        persist_journal(&self.journal_path, &journal)?;
        finish_revocations(errors)
    }

    /// Revoke all delivered roots. A failed root stays in the journal for
    /// retry; successfully revoked roots are removed even on partial failure.
    pub fn revoke_all(&self) -> Result<usize> {
        let mut journal = load_journal(&self.journal_path)?;
        let Some(record) = journal.sessions.get_mut(&self.session_id) else {
            return Ok(0);
        };
        let sid = self.local_sid()?;
        let (revoked, errors) = revoke_record_grants(
            record,
            |_| true,
            |path| unsafe { revoke_ace_checked(path, sid.as_ptr()) },
        );
        if record.grants.is_empty() {
            journal.sessions.remove(&self.session_id);
        }
        persist_journal(&self.journal_path, &journal)?;
        finish_revocations(errors)?;
        Ok(revoked)
    }
}

/// A failure keeps exactly the unresolved grant in the caller's journal. This
/// helper also permits deterministic failure-injection tests without changing
/// filesystem ACLs or requiring an elevated Windows test runner.
fn revoke_record_grants(
    record: &mut SessionRecord,
    should_revoke: impl Fn(&GrantRecord) -> bool,
    mut revoke: impl FnMut(&Path) -> Result<()>,
) -> (usize, Vec<anyhow::Error>) {
    let mut revoked = 0;
    let mut errors = Vec::new();
    record.grants.retain(|grant| {
        if !should_revoke(grant) {
            return true;
        }
        match revoke(&grant.root) {
            Ok(()) => {
                revoked += 1;
                false
            }
            Err(err) => {
                errors.push(err);
                true
            }
        }
    });
    (revoked, errors)
}

fn finish_revocations(mut errors: Vec<anyhow::Error>) -> Result<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        let count = errors.len();
        Err(anyhow::anyhow!(
            "{count} sandbox credential ACL revocation(s) failed; grants remain journaled: {}",
            errors.remove(0)
        ))
    }
}

/// Explicitly sweep sessions known to be dead, after the caller has verified
/// liveness across *all* daemon processes using this home. Do not call with an
/// empty live set at startup unless exclusive ownership of the home is proven:
/// another daemon may have a live fenced kernel with a journaled session.
pub fn sweep_orphaned_sessions(devo_home: &Path, live_session_ids: &[&str]) -> Result<usize> {
    let journal_path = journal_path_for(devo_home);
    let mut journal = load_journal(&journal_path)?;
    let dead: Vec<String> = journal
        .sessions
        .keys()
        .filter(|id| !live_session_ids.contains(&id.as_str()))
        .cloned()
        .collect();
    let mut swept = 0usize;
    let mut errors = Vec::new();
    for id in &dead {
        let Some(record) = journal.sessions.get_mut(id) else {
            continue;
        };
        let sid = match LocalSid::from_string(&record.sid) {
            Ok(sid) => sid,
            Err(err) => {
                errors.push(err);
                continue;
            }
        };
        let (revoked, failures) = revoke_record_grants(
            record,
            |_| true,
            |path| unsafe { revoke_ace_checked(path, sid.as_ptr()) },
        );
        swept += revoked;
        errors.extend(failures);
        if record.grants.is_empty() {
            journal.sessions.remove(id);
        }
    }
    persist_journal(&journal_path, &journal)?;
    finish_revocations(errors)?;
    Ok(swept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::dacl_has_write_allow_for_sid;
    use crate::acl::fetch_dacl_handle;
    use pretty_assertions::assert_eq;

    #[test]
    fn failed_acl_revoke_preserves_only_unresolved_journal_entries() {
        let mut record = SessionRecord {
            sid: "S-1-5-21-101-102-103-104".into(),
            grants: ["revoke", "fail", "unrelated"]
                .map(|name| GrantRecord {
                    root: PathBuf::from(name),
                    access: "write".into(),
                })
                .into(),
        };
        let (revoked, errors) = revoke_record_grants(
            &mut record,
            |grant| grant.root.as_path() != Path::new("unrelated"),
            |path| {
                if path == Path::new("fail") {
                    anyhow::bail!("injected ACL failure")
                }
                Ok(())
            },
        );
        assert_eq!(revoked, 1);
        assert_eq!(errors.len(), 1);
        assert_eq!(
            record
                .grants
                .iter()
                .map(|grant| grant.root.as_path())
                .collect::<Vec<_>>(),
            vec![Path::new("fail"), Path::new("unrelated")]
        );
        assert!(finish_revocations(errors).is_err());
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = journal_path_for(tmp.path());
        let journal = CredentialJournal {
            sessions: BTreeMap::from([("ses_failed".into(), record)]),
        };
        persist_journal(&path, &journal).expect("save retry state");
        let retried = load_journal(&path).expect("reload retry state");
        assert_eq!(retried.sessions["ses_failed"].grants.len(), 2);
        assert_eq!(
            retried.sessions["ses_failed"].grants[0].root,
            PathBuf::from("fail")
        );
    }

    #[test]
    fn corrupted_journal_must_not_be_treated_as_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = journal_path_for(tmp.path());
        fs::create_dir_all(path.parent().expect("journal parent")).expect("create parent");
        fs::write(&path, b"{truncated").expect("write journal");
        assert!(load_journal(&path).is_err());
    }

    #[test]
    fn grant_and_revoke_write_root_round_trip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("home");
        let root = tmp.path().join("granted");
        fs::create_dir_all(&root).expect("root");

        let authority =
            SessionCredentialAuthority::new("ses_test_1", home.path()).expect("authority");
        // Bind the LocalSid: as_ptr() on a temporary would dangle after the
        // statement ends.
        let sid = LocalSid::from_string(authority.sid()).expect("sid");
        let sid_ptr = sid.as_ptr();

        let (dacl, _sd) = unsafe { fetch_dacl_handle(&root).expect("dacl") };
        assert!(
            !unsafe { dacl_has_write_allow_for_sid(dacl, sid_ptr) },
            "fresh root must not carry the session SID"
        );

        assert!(authority.grant_write_root(&root).expect("grant"));
        // Idempotent: second grant is a journal no-op (Ok(false) or ACE-exists).
        let again = authority.grant_write_root(&root).expect("grant again");
        assert!(!again, "second grant must be a no-op");

        let (dacl, _sd) = unsafe { fetch_dacl_handle(&root).expect("dacl") };
        assert!(
            unsafe { dacl_has_write_allow_for_sid(dacl, sid_ptr) },
            "granted root must carry the session SID allow ACE"
        );

        authority.revoke_write_root(&root).expect("revoke");
        let (dacl, _sd) = unsafe { fetch_dacl_handle(&root).expect("dacl") };
        assert!(
            !unsafe { dacl_has_write_allow_for_sid(dacl, sid_ptr) },
            "revoked root must not carry the session SID ACE"
        );
    }

    #[test]
    fn revoke_all_clears_journal_row_and_aces() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("home");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        fs::create_dir_all(&a).expect("a");
        fs::create_dir_all(&b).expect("b");

        let authority =
            SessionCredentialAuthority::new("ses_test_2", home.path()).expect("authority");
        authority.grant_write_root(&a).expect("grant a");
        authority.grant_write_root(&b).expect("grant b");

        assert_eq!(authority.revoke_all().expect("revoke all"), 2);
        assert_eq!(authority.revoke_all().expect("revoke all again"), 0);

        let journal = load_journal(&journal_path_for(home.path())).expect("journal");
        assert!(journal.sessions.is_empty(), "journal row must be gone");
    }

    #[test]
    fn sweep_removes_only_dead_sessions() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("home");
        let root = tmp.path().join("dead-owned");
        fs::create_dir_all(&root).expect("root");

        let dead = SessionCredentialAuthority::new("ses_dead", home.path()).expect("dead");
        dead.grant_write_root(&root).expect("grant");
        let live = SessionCredentialAuthority::new("ses_live", home.path()).expect("live");

        let swept = super::sweep_orphaned_sessions(home.path(), &["ses_live"]).expect("sweep");
        assert_eq!(swept, 1, "dead session root must be swept");

        let (dacl, _sd) = unsafe { fetch_dacl_handle(&root).expect("dacl") };
        let dead_sid = LocalSid::from_string(dead.sid()).expect("sid");
        assert!(
            !unsafe { dacl_has_write_allow_for_sid(dacl, dead_sid.as_ptr()) },
            "dead session ACE must be removed"
        );
        let _ = live; // live authority untouched by the sweep
    }

    /// Env-driven grant runner (operational tool, not an assertion):
    /// `DEVO_GRANT_SID` + `DEVO_GRANT_ROOT` [+ `DEVO_GRANT_HOME`] delivers a
    /// write ACE for an already-running fenced kernel's session SID.
    /// Runs as a test so it shares the crate's token/ACL plumbing:
    /// `cargo test -p devo-windows-sandbox --lib grant_for_running_session -- --nocapture`
    #[test]
    fn grant_for_running_session() {
        let (Ok(sid), Ok(root), home) = (
            std::env::var("DEVO_GRANT_SID"),
            std::env::var("DEVO_GRANT_ROOT"),
            std::env::var("DEVO_GRANT_HOME").unwrap_or_else(|_| {
                std::env::var_os("USERPROFILE")
                    .map(|p| format!("{}\\.devo", p.to_string_lossy()))
                    .unwrap_or_else(|| ".devo".to_string())
            }),
        ) else {
            eprintln!("skip: DEVO_GRANT_SID/DEVO_GRANT_ROOT not set");
            return;
        };
        let _authority =
            SessionCredentialAuthority::new(&format!("kernel-{sid}"), std::path::Path::new(&home))
                .expect("authority");
        // The authority mints a fresh SID; for an external grant the SID is
        // fixed, so rebind via the journal path with the given SID.
        let applied = unsafe {
            crate::acl::add_allow_ace(
                std::path::Path::new(&root),
                LocalSid::from_string(&sid).expect("sid").as_ptr(),
            )
        }
        .expect("grant");
        eprintln!("granted write ACE root={root} applied={applied}");
    }

    #[test]
    fn grant_read_root_delivers_read_mask_ace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("home");
        let root = tmp.path().join("readable");
        fs::create_dir_all(&root).expect("root");

        let authority =
            SessionCredentialAuthority::new("ses_test_read", home.path()).expect("authority");
        assert!(authority.grant_read_root(&root).expect("grant read"));
        assert!(!authority.grant_read_root(&root).expect("grant read again"));

        let sid = LocalSid::from_string(authority.sid()).expect("sid");
        let read_mask = FILE_GENERIC_READ | GENERIC_READ_MASK;
        assert!(
            crate::acl::path_mask_allows(
                &root,
                &[sid.as_ptr()],
                read_mask,
                /*require_all_bits*/ false,
            )
            .expect("mask check"),
            "read grant must deliver a read-mask allow ACE"
        );
    }
}
