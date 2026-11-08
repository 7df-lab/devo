//! Descriptor-relative storage: approval cannot be redirected by a later path swap.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};

pub(super) struct Directory {
    file: File,
    pub path: PathBuf,
}

#[derive(Clone, Copy)]
pub(super) enum FileMode {
    Read,
    Lock,
    Create,
}

impl Directory {
    pub fn open(path: &Path) -> io::Result<Self> {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(io::Error::other("Factory directory cannot be a symlink"));
        }
        let path = std::fs::canonicalize(path)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
                FILE_SHARE_WRITE,
            };
            options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
            // Prevent directory replacement while an approved operation owns it.
            options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other("Factory store is not a regular directory"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(io::Error::other("Factory store cannot be a reparse point"));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let current = std::fs::metadata(&path)?;
            if metadata.dev() != current.dev() || metadata.ino() != current.ino() {
                return Err(io::Error::other("Factory directory changed during open"));
            }
        }
        Ok(Self { file, path })
    }

    pub fn child(&self, name: &str) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let name = std::ffi::CString::new(name)?;
            // SAFETY: the parent descriptor is owned and the name is NUL-terminated.
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: openat returned a new owned descriptor.
            let file = unsafe { File::from_raw_fd(fd) };
            Ok(Self {
                file,
                path: self.path.join(name.to_str().expect("ASCII store name")),
            })
        }
        #[cfg(not(unix))]
        {
            Self::open(&self.path.join(name))
        }
    }

    pub fn create_child(&self, name: &str) -> io::Result<Self> {
        #[cfg(unix)]
        let result = {
            let name = std::ffi::CString::new(name)?;
            // SAFETY: the parent descriptor is owned and the name is NUL-terminated.
            if unsafe { libc::mkdirat(self.file.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        };
        #[cfg(not(unix))]
        let result = std::fs::create_dir(self.path.join(name));
        match result {
            Ok(()) => self.sync()?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        self.child(name)
    }

    pub fn open_file(&self, name: &str, mode: FileMode) -> io::Result<File> {
        #[cfg(unix)]
        let file = {
            let name = std::ffi::CString::new(name)?;
            let flags = match mode {
                FileMode::Read => libc::O_RDONLY,
                FileMode::Lock => libc::O_RDWR | libc::O_CREAT,
                FileMode::Create => libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            } | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | libc::O_CLOEXEC;
            // SAFETY: descriptor and C string remain valid; mode applies only to creation.
            let fd = unsafe { libc::openat(self.file.as_raw_fd(), name.as_ptr(), flags, 0o600) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: openat returned a new owned descriptor.
            unsafe { File::from_raw_fd(fd) }
        };
        #[cfg(not(unix))]
        let file = {
            let mut options = OpenOptions::new();
            match mode {
                FileMode::Read => {
                    options.read(true);
                }
                FileMode::Lock => {
                    options.read(true).write(true).create(true).truncate(false);
                }
                FileMode::Create => {
                    options.write(true).create_new(true);
                }
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
                options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
            }
            options.open(self.path.join(name))?
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "Factory store entry is not a regular file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(io::Error::other(
                    "Factory store entry cannot have hard links",
                ));
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(io::Error::other(
                    "Factory store entry cannot be a reparse point",
                ));
            }
        }
        Ok(file)
    }

    pub fn replace(&self, source: &str, target: &str) -> io::Result<()> {
        #[cfg(unix)]
        {
            let source = std::ffi::CString::new(source)?;
            let target = std::ffi::CString::new(target)?;
            // SAFETY: directory is owned; both names are validated, NUL-terminated basenames.
            if unsafe {
                libc::renameat(
                    self.file.as_raw_fd(),
                    source.as_ptr(),
                    self.file.as_raw_fd(),
                    target.as_ptr(),
                )
            } < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(unix))]
        std::fs::rename(self.path.join(source), self.path.join(target))?;
        self.sync()
    }

    pub fn remove(&self, name: &str) -> io::Result<()> {
        #[cfg(unix)]
        {
            let name = std::ffi::CString::new(name)?;
            // SAFETY: directory is owned; only a temporary basename is removed.
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            std::fs::remove_file(self.path.join(name))
        }
    }

    pub fn entries(&self) -> io::Result<Vec<String>> {
        #[cfg(unix)]
        {
            // Open a fresh directory description so repeated scans have independent offsets.
            let scan = self.child(".")?;
            use std::os::fd::IntoRawFd;
            let fd = scan.file.into_raw_fd();
            // SAFETY: fd is a fresh owned directory descriptor; fdopendir takes ownership on success.
            let directory = unsafe { libc::fdopendir(fd) };
            if directory.is_null() {
                let error = io::Error::last_os_error();
                // SAFETY: ownership was not transferred on fdopendir failure.
                unsafe { libc::close(fd) };
                return Err(error);
            }
            let mut names = Vec::new();
            let mut read_error = None;
            loop {
                // SAFETY: errno is thread-local; clearing it distinguishes end-of-stream.
                unsafe { *errno_location() = 0 };
                // SAFETY: directory is live and used only on this thread.
                let entry = unsafe { libc::readdir(directory) };
                if entry.is_null() {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(0) {
                        read_error = Some(error);
                    }
                    break;
                }
                // SAFETY: readdir supplies a NUL-terminated name valid until the next call.
                let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
                names.push(name.to_string_lossy().into_owned());
            }
            // SAFETY: closes the stream and its descriptor exactly once.
            unsafe { libc::closedir(directory) };
            match read_error {
                Some(error) => Err(error),
                None => Ok(names),
            }
        }
        #[cfg(not(unix))]
        {
            std::fs::read_dir(&self.path)?
                .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
                .collect()
        }
    }

    fn sync(&self) -> io::Result<()> {
        #[cfg(unix)]
        self.file.sync_all()?;
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "emscripten"))]
unsafe fn errno_location() -> *mut libc::c_int {
    // SAFETY: the platform exposes the calling thread's errno storage.
    unsafe { libc::__errno_location() }
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
unsafe fn errno_location() -> *mut libc::c_int {
    // SAFETY: the platform exposes the calling thread's errno storage.
    unsafe { libc::__error() }
}

#[cfg(target_os = "android")]
unsafe fn errno_location() -> *mut libc::c_int {
    // SAFETY: the platform exposes the calling thread's errno storage.
    unsafe { libc::__errno() }
}
