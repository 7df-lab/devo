#![allow(clippy::unwrap_used)]

use super::psuedocon::PsuedoCon;
use filedescriptor::FileDescriptor;
use filedescriptor::Pipe;
use std::mem::ManuallyDrop;
use std::os::windows::io::RawHandle;
use std::ptr;
use winapi::um::wincon::COORD;

fn create_conpty_handles(
    cols: u16,
    rows: u16,
) -> anyhow::Result<(PsuedoCon, FileDescriptor, FileDescriptor)> {
    let stdin = Pipe::new()?;
    let stdout = Pipe::new()?;
    let con = PsuedoCon::new(
        COORD {
            X: cols as i16,
            Y: rows as i16,
        },
        stdin.read,
        stdout.write,
    )?;
    Ok((con, stdin.write, stdout.read))
}

pub struct RawConPty {
    con: PsuedoCon,
    input_write: FileDescriptor,
    output_read: FileDescriptor,
}

impl RawConPty {
    pub fn new(cols: i16, rows: i16) -> anyhow::Result<Self> {
        let (con, input_write, output_read) = create_conpty_handles(cols as u16, rows as u16)?;
        Ok(Self {
            con,
            input_write,
            output_read,
        })
    }

    pub fn pseudoconsole_handle(&self) -> RawHandle {
        self.con.raw_handle() as RawHandle
    }

    pub fn into_handles(self) -> (PsuedoCon, FileDescriptor, FileDescriptor) {
        let me = ManuallyDrop::new(self);
        unsafe {
            (
                ptr::read(&me.con),
                ptr::read(&me.input_write),
                ptr::read(&me.output_read),
            )
        }
    }
}
