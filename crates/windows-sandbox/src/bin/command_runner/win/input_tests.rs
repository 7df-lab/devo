use super::*;
use pretty_assertions::assert_eq;
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};

#[test]
fn disconnect_child() {
    if std::env::var_os("DEVO_RUNNER_DISCONNECT_CHILD").is_some() {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn parent_disconnect_stops_child_on_eof_or_invalid_frame() {
    for input in [Vec::new(), vec![1, 0, 0, 0, b'{']] {
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", "win::input_tests::disconnect_child"])
                .env("DEVO_RUNNER_DISCONNECT_CHILD", "1")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("child process");
        let mut reader = tempfile::tempfile().expect("input file");
        use std::io::{Seek, Write};
        reader.write_all(&input).expect("write input");
        reader.rewind().expect("rewind input");
        let input_loop = spawn_input_loop(
            reader,
            /*stdin_handle*/ None,
            Arc::new(StdMutex::new(None)),
            Arc::new(StdMutex::new(Some(child.as_raw_handle() as HANDLE))),
            /*log_dir*/ None,
        );
        input_loop.join().expect("input loop completes");
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().expect("child status") {
                break status;
            }
            assert!(Instant::now() < deadline, "disconnected child must stop");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(1));
    }
}
