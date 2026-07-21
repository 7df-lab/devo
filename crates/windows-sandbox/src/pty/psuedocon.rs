#![allow(clippy::expect_used)]
#![allow(clippy::upper_case_acronyms)]

// ConPTY bindings adapted from wezterm (MIT license).

use anyhow::Error;
use anyhow::ensure;
use filedescriptor::FileDescriptor;
use lazy_static::lazy_static;
use shared_library::shared_library;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use winapi::shared::minwindef::DWORD;
use winapi::shared::ntdef::NTSTATUS;
use winapi::shared::ntstatus::STATUS_SUCCESS;
use winapi::shared::winerror::HRESULT;
use winapi::shared::winerror::S_OK;
use winapi::um::handleapi::INVALID_HANDLE_VALUE;
use winapi::um::wincon::COORD;
use winapi::um::winnt::HANDLE;
use winapi::um::winnt::OSVERSIONINFOW;

pub type HPCON = HANDLE;

pub const PSEUDOCONSOLE_RESIZE_QUIRK: DWORD = 0x2;

const MIN_CONPTY_BUILD: u32 = 17_763;

shared_library!(ConPtyFuncs,
    pub fn CreatePseudoConsole(
        size: COORD,
        hInput: HANDLE,
        hOutput: HANDLE,
        flags: DWORD,
        hpc: *mut HPCON
    ) -> HRESULT,
    pub fn ResizePseudoConsole(hpc: HPCON, size: COORD) -> HRESULT,
    pub fn ClosePseudoConsole(hpc: HPCON),
);

shared_library!(Ntdll,
    pub fn RtlGetVersion(
        version_info: *mut OSVERSIONINFOW
    ) -> NTSTATUS,
);

fn load_conpty() -> ConPtyFuncs {
    let kernel = ConPtyFuncs::open(Path::new("kernel32.dll")).expect(
        "this system does not support conpty.  Windows 10 October 2018 or newer is required",
    );

    if let Ok(sideloaded) = ConPtyFuncs::open(Path::new("conpty.dll")) {
        sideloaded
    } else {
        kernel
    }
}

lazy_static! {
    static ref CONPTY: ConPtyFuncs = load_conpty();
}

pub fn conpty_supported() -> bool {
    windows_build_number().is_some_and(|build| build >= MIN_CONPTY_BUILD)
}

fn windows_build_number() -> Option<u32> {
    let ntdll = Ntdll::open(Path::new("ntdll.dll")).ok()?;
    let mut info: OSVERSIONINFOW = unsafe { mem::zeroed() };
    info.dwOSVersionInfoSize = mem::size_of::<OSVERSIONINFOW>() as u32;
    let status = unsafe { (ntdll.RtlGetVersion)(&mut info) };
    if status == STATUS_SUCCESS {
        Some(info.dwBuildNumber)
    } else {
        None
    }
}

pub struct PsuedoCon {
    con: HPCON,
    _input: FileDescriptor,
    _output: FileDescriptor,
}

unsafe impl Send for PsuedoCon {}
unsafe impl Sync for PsuedoCon {}

impl Drop for PsuedoCon {
    fn drop(&mut self) {
        unsafe { (CONPTY.ClosePseudoConsole)(self.con) };
    }
}

impl PsuedoCon {
    pub fn raw_handle(&self) -> HPCON {
        self.con
    }

    pub fn new(size: COORD, input: FileDescriptor, output: FileDescriptor) -> Result<Self, Error> {
        let mut con: HPCON = INVALID_HANDLE_VALUE;
        let result = unsafe {
            (CONPTY.CreatePseudoConsole)(
                size,
                input.as_raw_handle() as _,
                output.as_raw_handle() as _,
                PSEUDOCONSOLE_RESIZE_QUIRK,
                &mut con,
            )
        };
        ensure!(
            result == S_OK,
            "failed to create psuedo console: HRESULT {result}"
        );
        Ok(Self {
            con,
            _input: input,
            _output: output,
        })
    }

    pub fn resize(&self, size: COORD) -> Result<(), Error> {
        let result = unsafe { (CONPTY.ResizePseudoConsole)(self.con, size) };
        ensure!(
            result == S_OK,
            "failed to resize console to {}x{}: HRESULT: {}",
            size.X,
            size.Y,
            result
        );
        Ok(())
    }
}
