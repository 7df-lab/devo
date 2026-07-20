//! PTY/process adapter for Windows sandbox session drivers.
#![allow(dead_code)]

#[cfg(windows)]
mod conpty;
#[cfg(windows)]
mod process;
#[cfg(windows)]
mod procthreadattr;
#[cfg(windows)]
mod psuedocon;
#[cfg(windows)]
mod windows_input;

#[cfg(windows)]
pub use conpty::RawConPty;
#[cfg(windows)]
pub use process::{ProcessDriver, SpawnedProcess, TerminalSize, spawn_from_driver};
#[cfg(windows)]
pub use psuedocon::PsuedoCon;
#[cfg(windows)]
pub use windows_input::WindowsTtyInputNormalizer;
