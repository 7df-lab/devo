//! POSIX credential delivery channel (design doc §9, P2). Devo-owned: codex's
//! per-exec model never delivers credentials to a long-lived process.
//!
//! A Unix stream socketpair is created at spawn; one end stays with the
//! host, the other is inherited by the kernel process. Grants travel as a
//! one-line JSON payload plus one passed descriptor:
//!
//! 1. host opens the granted root with `open_tree(OPEN_TREE_CLONE)` — a
//!    detached mount clone whose root IS the granted directory, so a dirfd
//!    handed out this way clamps `..` at the grant boundary (the naive
//!    "open the dir and pass the fd" alternative lets `openat(fd, "../..")`
//!    walk the host mount tree with the kernel's own uid);
//! 2. host fstats the fd and sends `{"type":"grant",...}` with the fd
//!    attached via `SCM_RIGHTS` plus the claimed `(dev, ino)`;
//! 3. the kernel-side facade verifies `fstat(fd)` matches the claim before
//!    accepting (anti-swap), stores it per granted root, and serves
//!    `rlm.read`/`rlm.write` with `os.open(path, dir_fd=fd)`.
//!
//! Revocation is cooperative: a `{"type":"revoke"}` message makes the
//! facade drop the descriptor (already-open handles keep working — a
//! documented residual, §9). Plain libc: no high-level socket crate needed.

use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;
use std::os::fd::RawFd;
use std::path::Path;

#[derive(Debug)]
pub struct GrantChannel {
    socket: OwnedFd,
    peer: OwnedFd,
}

/// Message shape sent with the descriptor (also carries the anti-swap claim).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct GrantMessage {
    #[serde(rename = "type")]
    pub kind: String, // "grant" | "revoke"
    pub root: String,
    pub access: String, // "read" | "write"
    pub dev: u64,
    pub ino: u64,
}

const SCM_MAX_FD: usize = 1;

#[allow(dead_code)] // wired incrementally: session holds the channel; the
// approval callback and Python facade land in the follow-up commit.
impl GrantChannel {
    /// Create the channel; the peer descriptor is kept internally so the
    /// host can duplicate it into the child at spawn (peer_fd).
    pub(crate) fn new() -> io::Result<Self> {
        let mut fds = [0 as RawFd; 2];
        let rc = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // No CLOEXEC: the peer duplicate survives the kernel exec.
        let (host, peer) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok(Self { socket: host, peer })
    }

    /// Test constructor: channel + a duplicate of the peer (so the receiving
    /// side can recvmsg on its own copy).
    #[cfg(test)]
    pub(crate) fn with_peer_copy() -> io::Result<(Self, OwnedFd)> {
        let me = Self::new()?;
        let peer_dup = dup_no_cloexec(me.peer_fd())?;
        Ok((me, unsafe { OwnedFd::from_raw_fd(peer_dup) }))
    }

    /// Peer fd number (duplicate this into the child at spawn).
    pub(crate) fn peer_fd(&self) -> RawFd {
        self.peer.as_raw_fd()
    }

    /// Deliver one grant: open a detached mount clone of `root` and send it
    /// with its (dev, ino) claim.
    pub fn grant(&self, root: &Path, access: &str) -> io::Result<()> {
        let path = std::ffi::CString::new(root.as_os_str().as_encoded_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        // Prefer open_tree(OPEN_TREE_CLONE|AT_RECURSIVE): a detached mount
        // whose root IS `root`, clamping `..` at the grant boundary. CLONE
        // needs CAP_SYS_ADMIN over the mount ns, so unprivileged hosts get
        // EPERM — degrade to a plain O_PATH dirfd (no mount clamp; the OS
        // fence stays the actual wall, and `..`-traversal beyond the grant
        // remains subject to the kernel's Landlock/bwrap ruleset).
        const SYS_OPEN_TREE: libc::c_long = 428;
        let fd = unsafe { libc::syscall(SYS_OPEN_TREE, -100i64, path.as_ptr(), 1 | 0x8000) };
        let tree = if fd >= 0 {
            unsafe { OwnedFd::from_raw_fd(fd as i32) }
        } else {
            let open_fd = unsafe { libc::open(path.as_ptr(), libc::O_PATH | libc::O_DIRECTORY) };
            if open_fd < 0 {
                return Err(io::Error::last_os_error());
            }
            unsafe { OwnedFd::from_raw_fd(open_fd) }
        };
        let claim = stat_claim(tree.as_raw_fd())?;
        let message = GrantMessage {
            kind: "grant".to_string(),
            root: root.to_string_lossy().into_owned(),
            access: access.to_string(),
            dev: claim.0,
            ino: claim.1,
        };
        self.send_with_fd(&message, &tree)
    }

    /// Ask the kernel facade to drop the descriptor for `root`.
    pub fn revoke(&self, root: &Path) -> io::Result<()> {
        let message = GrantMessage {
            kind: "revoke".to_string(),
            root: root.to_string_lossy().into_owned(),
            access: String::new(),
            dev: 0,
            ino: 0,
        };
        self.send_with_fd(&message, &Self::dev_null()?)
    }

    fn dev_null() -> io::Result<OwnedFd> {
        let path = std::ffi::CString::new("/dev/null")?;
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Receive one message + fd (kernel/test side). `buf` must be large
    /// enough for the JSON payload.
    pub(crate) fn recv_with_fd(fd: RawFd, buf: &mut [u8]) -> io::Result<(usize, OwnedFd)> {
        let mut cmsg_buf = vec![0u8; unsafe { libc::CMSG_SPACE((SCM_MAX_FD * 4) as u32) } as usize];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut libc::c_void,
            iov_len: buf.len(),
        };
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = cmsg_buf.len();
        let n = unsafe { libc::recvmsg(fd, &mut msg, 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut received: Option<OwnedFd> = None;
        let cmsg_ptr = cmsg_buf.as_ptr() as *mut libc::cmsghdr;
        let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        while !cmsg.is_null() {
            let kind = unsafe { (*cmsg).cmsg_type };
            if kind == libc::SCM_RIGHTS {
                let data = unsafe { libc::CMSG_DATA(cmsg) } as *const i32;
                let count = ((unsafe { (*cmsg).cmsg_len } as usize
                    - unsafe { libc::CMSG_LEN(0) } as usize)
                    / std::mem::size_of::<i32>())
                .min(SCM_MAX_FD);
                for i in 0..count {
                    let fd = unsafe { *data.add(i) };
                    if received.is_none() {
                        received = Some(unsafe { OwnedFd::from_raw_fd(fd) });
                    } else {
                        unsafe { libc::close(fd) };
                    }
                }
            }
            cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg_ptr) };
        }
        let fd = received.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "grant message carried no fd")
        })?;
        Ok((n as usize, fd))
    }

    fn send_with_fd(&self, message: &GrantMessage, fd: &OwnedFd) -> io::Result<()> {
        let mut payload = serde_json::to_vec(message).map_err(io::Error::other)?;
        payload.push(b'\n');
        let mut iov = libc::iovec {
            iov_base: payload.as_ptr() as *mut libc::c_void,
            iov_len: payload.len(),
        };
        let mut cmsg_buf = vec![0u8; unsafe { libc::CMSG_SPACE((SCM_MAX_FD * 4) as u32) } as usize];
        let cmsg = cmsg_buf.as_mut_ptr() as *mut libc::cmsghdr;
        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN((SCM_MAX_FD * 4) as u32) as usize;
            let data = libc::CMSG_DATA(cmsg) as *mut i32;
            *data = fd.as_raw_fd();
        }
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = cmsg_buf.len();
        let rc = unsafe { libc::sendmsg(self.socket.as_raw_fd(), &msg, 0) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

pub(crate) fn stat_claim(fd: RawFd) -> io::Result<(u64, u64)> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::fstat(fd, &mut st) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((st.st_dev as u64, st.st_ino as u64))
}

/// Duplicate `fd` without CLOEXEC so it survives the child's exec.
pub(crate) fn dup_no_cloexec(fd: RawFd) -> io::Result<RawFd> {
    let dup = unsafe { libc::dup(fd) };
    if dup < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(dup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_delivers_grant_fd_with_matching_claim() {
        let (host, peer) = GrantChannel::with_peer_copy().expect("socketpair");
        let dir = tempfile::tempdir().expect("tempdir");

        host.grant(dir.path(), "write").expect("grant");

        let mut buf = [0u8; 1024];
        let (n, fd) = GrantChannel::recv_with_fd(peer.as_raw_fd(), &mut buf).expect("recv");
        let message: GrantMessage = serde_json::from_slice(&buf[..n]).expect("grant message json");
        assert_eq!(message.kind, "grant");
        assert_eq!(message.access, "write");
        assert_eq!(message.root, dir.path().to_string_lossy());

        // Anti-swap: fstat must match the claim.
        let (dev, ino) = stat_claim(fd.as_raw_fd()).expect("fstat received fd");
        assert_eq!(dev, message.dev);
        assert_eq!(ino, message.ino);
    }

    #[test]
    fn grant_fd_parent_traversal_behavior_matches_capabilities() {
        // When open_tree(CLONE) is available the grant fd is a detached mount
        // clone: `..` through it clamps at the grant root. Unprivileged hosts
        // (EPERM) degrade to a plain O_PATH dirfd: `..` walks to the host
        // parent — documented, and still contained by the OS fence's ruleset.
        let (host, peer) = GrantChannel::with_peer_copy().expect("socketpair");
        let dir = tempfile::tempdir().expect("tempdir");
        host.grant(dir.path(), "write").expect("grant");
        let mut buf = [0u8; 1024];
        let (_n, fd) = GrantChannel::recv_with_fd(peer.as_raw_fd(), &mut buf).expect("recv");

        let path = std::ffi::CString::new(dir.path().as_os_str().as_encoded_bytes()).unwrap();
        const SYS_OPEN_TREE: libc::c_long = 428;
        let cloned = unsafe { libc::syscall(SYS_OPEN_TREE, -100i64, path.as_ptr(), 1 | 0x8000) };

        let parent = std::ffi::CString::new("..").expect("cstring");
        let opened = unsafe {
            libc::openat(
                fd.as_raw_fd(),
                parent.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY,
            )
        };
        assert!(opened >= 0, "openat ..: {}", io::Error::last_os_error());
        let (opened_dev, opened_ino) = stat_claim(opened).expect("fstat parent");
        let (root_dev, root_ino) = stat_claim(fd.as_raw_fd()).expect("fstat grant root");
        unsafe { libc::close(opened) };
        if cloned >= 0 {
            unsafe { libc::close(cloned as i32) };
            assert_eq!(
                (opened_dev, opened_ino),
                (root_dev, root_ino),
                "with open_tree(CLONE), '..' must clamp at the grant root"
            );
        } else {
            // Degraded mode: no clamp (the fence stays the wall).
            assert_ne!((opened_dev, opened_ino), (root_dev, root_ino));
        }
    }
}
