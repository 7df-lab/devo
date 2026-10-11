use anyhow::Result;
use devo_windows_sandbox::to_wide;
use std::ffi::OsStr;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::System::Threading::MUTEX_ALL_ACCESS;
use windows_sys::Win32::System::Threading::OpenMutexW;
use windows_sys::Win32::System::Threading::ReleaseMutex;
use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};

const READ_ACL_MUTEX_NAME: &str = "Local\\DevoSandboxReadAcl";

pub(super) struct ReadAclMutexGuard {
    handle: HANDLE,
}

impl Drop for ReadAclMutexGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            CloseHandle(self.handle);
        }
    }
}

pub(super) fn read_acl_mutex_exists() -> Result<bool> {
    let name = to_wide(OsStr::new(READ_ACL_MUTEX_NAME));
    let handle = unsafe { OpenMutexW(MUTEX_ALL_ACCESS, 0, name.as_ptr()) };
    if handle == 0 {
        let err = unsafe { GetLastError() };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        return Err(anyhow::anyhow!("OpenMutexW failed: {err}"));
    }
    unsafe {
        CloseHandle(handle);
    }
    Ok(true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadAclWait {
    SkipBusy,
    Wait,
}

pub(super) fn acquire_read_acl_mutex(wait: ReadAclWait) -> Result<Option<ReadAclMutexGuard>> {
    acquire_named_read_acl_mutex(OsStr::new(READ_ACL_MUTEX_NAME), wait)
}

fn acquire_named_read_acl_mutex(
    name: &OsStr,
    wait: ReadAclWait,
) -> Result<Option<ReadAclMutexGuard>> {
    let name = to_wide(name);
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 1, name.as_ptr()) };
    if handle == 0 {
        return Err(anyhow::anyhow!("CreateMutexW failed: {}", unsafe {
            GetLastError()
        }));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        match wait {
            ReadAclWait::SkipBusy => {
                unsafe { CloseHandle(handle) };
                return Ok(None);
            }
            ReadAclWait::Wait => {
                let result = unsafe { WaitForSingleObject(handle, INFINITE) };
                if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
                    let error = unsafe { GetLastError() };
                    unsafe { CloseHandle(handle) };
                    return Err(anyhow::anyhow!("WaitForSingleObject failed: {error}"));
                }
            }
        }
    }
    Ok(Some(ReadAclMutexGuard { handle }))
}

#[cfg(test)]
mod tests {
    use super::{ReadAclWait, acquire_named_read_acl_mutex};
    use pretty_assertions::assert_eq;
    use std::ffi::OsStr;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn explicit_read_request_waits_for_busy_helper_instead_of_being_dropped() {
        let unique = tempfile::tempdir().expect("unique mutex identity");
        let name = format!(
            "Local\\DevoReadAclTest-{}",
            unique.path().file_name().unwrap().display()
        );
        let held = acquire_named_read_acl_mutex(OsStr::new(&name), ReadAclWait::Wait)
            .expect("acquire initial mutex")
            .expect("initial guard");
        assert_eq!(
            acquire_named_read_acl_mutex(OsStr::new(&name), ReadAclWait::SkipBusy)
                .expect("background probe")
                .is_none(),
            true,
        );
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).expect("signal waiter started");
            let acquired = acquire_named_read_acl_mutex(OsStr::new(&name), ReadAclWait::Wait)
                .expect("wait for explicit read request");
            result_tx.send(acquired.is_some()).expect("signal acquired");
        });
        started_rx.recv().expect("waiter started");
        assert_eq!(
            result_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout),
        );
        drop(held);
        assert_eq!(result_rx.recv_timeout(Duration::from_secs(5)), Ok(true));
        waiter.join().expect("waiter joined");
    }
}
