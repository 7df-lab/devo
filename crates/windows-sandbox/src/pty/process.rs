use core::fmt;
use std::io;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use anyhow::anyhow;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

trait ChildTerminator: Send + Sync {
    fn kill(&mut self) -> io::Result<()>;
}

type ResizeFn = Box<dyn FnMut(TerminalSize) -> anyhow::Result<()> + Send>;

/// Handle for driving an interactive process spawned via a driver backend.
pub struct ProcessHandle {
    writer_tx: StdMutex<Option<mpsc::Sender<Vec<u8>>>>,
    killer: StdMutex<Option<Box<dyn ChildTerminator>>>,
    reader_handle: StdMutex<Option<JoinHandle<()>>>,
    reader_abort_handles: StdMutex<Vec<AbortHandle>>,
    writer_handle: StdMutex<Option<JoinHandle<()>>>,
    wait_handle: StdMutex<Option<JoinHandle<()>>>,
    exit_status: Arc<AtomicBool>,
    exit_code: Arc<StdMutex<Option<i32>>>,
    resizer: StdMutex<Option<ResizeFn>>,
}

impl fmt::Debug for ProcessHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessHandle").finish()
    }
}

impl ProcessHandle {
    #[allow(clippy::too_many_arguments)]
    fn new(
        writer_tx: mpsc::Sender<Vec<u8>>,
        killer: Box<dyn ChildTerminator>,
        reader_handle: JoinHandle<()>,
        reader_abort_handles: Vec<AbortHandle>,
        writer_handle: JoinHandle<()>,
        wait_handle: JoinHandle<()>,
        exit_status: Arc<AtomicBool>,
        exit_code: Arc<StdMutex<Option<i32>>>,
        resizer: Option<ResizeFn>,
    ) -> Self {
        Self {
            writer_tx: StdMutex::new(Some(writer_tx)),
            killer: StdMutex::new(Some(killer)),
            reader_handle: StdMutex::new(Some(reader_handle)),
            reader_abort_handles: StdMutex::new(reader_abort_handles),
            writer_handle: StdMutex::new(Some(writer_handle)),
            wait_handle: StdMutex::new(Some(wait_handle)),
            exit_status,
            exit_code,
            resizer: StdMutex::new(resizer),
        }
    }

    pub fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        if let Ok(writer_tx) = self.writer_tx.lock()
            && let Some(writer_tx) = writer_tx.as_ref()
        {
            return writer_tx.clone();
        }

        let (writer_tx, writer_rx) = mpsc::channel(1);
        drop(writer_rx);
        writer_tx
    }

    pub fn has_exited(&self) -> bool {
        self.exit_status.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code.lock().ok().and_then(|guard| *guard)
    }

    pub fn resize(&self, size: TerminalSize) -> anyhow::Result<()> {
        let mut resizer = self
            .resizer
            .lock()
            .map_err(|_| anyhow!("failed to lock PTY resizer"))?;
        if let Some(resizer) = resizer.as_mut() {
            resizer(size)
        } else {
            Err(anyhow!("process is not attached to a PTY"))
        }
    }

    pub fn close_stdin(&self) {
        if let Ok(mut writer_tx) = self.writer_tx.lock() {
            writer_tx.take();
        }
    }

    pub fn request_terminate(&self) {
        if let Ok(mut killer_opt) = self.killer.lock()
            && let Some(mut killer) = killer_opt.take()
        {
            let _ = killer.kill();
        }
    }

    pub fn terminate(&self) {
        self.request_terminate();

        if let Ok(mut h) = self.reader_handle.lock()
            && let Some(handle) = h.take()
        {
            handle.abort();
        }
        if let Ok(mut handles) = self.reader_abort_handles.lock() {
            for handle in handles.drain(..) {
                handle.abort();
            }
        }
        if let Ok(mut h) = self.writer_handle.lock()
            && let Some(handle) = h.take()
        {
            handle.abort();
        }
        if let Ok(mut h) = self.wait_handle.lock()
            && let Some(handle) = h.take()
        {
            handle.abort();
        }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        self.terminate();
    }
}

struct ClosureTerminator {
    inner: Option<Box<dyn FnMut() + Send + Sync>>,
}

impl ChildTerminator for ClosureTerminator {
    fn kill(&mut self) -> io::Result<()> {
        if let Some(inner) = self.inner.as_mut() {
            (inner)();
        }
        Ok(())
    }
}

/// Return value from driver-backed Windows sandbox session spawns.
#[derive(Debug)]
pub struct SpawnedProcess {
    pub session: ProcessHandle,
    pub stdout_rx: mpsc::Receiver<Vec<u8>>,
    pub stderr_rx: mpsc::Receiver<Vec<u8>>,
    pub exit_rx: oneshot::Receiver<i32>,
}

/// Driver-backed process handles for non-standard spawn backends.
pub struct ProcessDriver {
    pub writer_tx: mpsc::Sender<Vec<u8>>,
    pub stdout_rx: broadcast::Receiver<Vec<u8>>,
    pub stderr_rx: Option<broadcast::Receiver<Vec<u8>>>,
    pub exit_rx: oneshot::Receiver<i32>,
    pub terminator: Option<Box<dyn FnMut() + Send + Sync>>,
    pub writer_handle: Option<JoinHandle<()>>,
    pub resizer: Option<ResizeFn>,
}

/// Build a [`SpawnedProcess`] from a driver that supplies stdin/output/exit channels.
pub fn spawn_from_driver(driver: ProcessDriver) -> SpawnedProcess {
    let ProcessDriver {
        writer_tx,
        stdout_rx: stdout_driver_rx,
        stderr_rx: mut stderr_driver_rx,
        exit_rx,
        terminator,
        writer_handle,
        resizer,
    } = driver;

    let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(256);
    let (stderr_tx, stderr_rx) = mpsc::channel::<Vec<u8>>(256);
    let (exit_seen_tx, exit_seen_rx) = watch::channel(false);
    let spawn_stream_reader =
        |mut output_rx: broadcast::Receiver<Vec<u8>>,
         output_tx: mpsc::Sender<Vec<u8>>,
         mut exit_seen_rx: watch::Receiver<bool>| {
            tokio::spawn(async move {
                loop {
                    let recv_result = if *exit_seen_rx.borrow() {
                        output_rx.recv().await
                    } else {
                        tokio::select! {
                            _ = exit_seen_rx.changed() => {
                                continue;
                            }
                            result = output_rx.recv() => result,
                        }
                    };
                    match recv_result {
                        Ok(chunk) => {
                            if output_tx.send(chunk).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            })
        };
    let reader_handle = spawn_stream_reader(stdout_driver_rx, stdout_tx, exit_seen_rx.clone());
    let stderr_reader_handle = stderr_driver_rx
        .take()
        .map(|rx| spawn_stream_reader(rx, stderr_tx, exit_seen_rx));

    let writer_handle = writer_handle.unwrap_or_else(|| tokio::spawn(async {}));

    let (exit_tx, exit_rx_out) = oneshot::channel::<i32>();
    let exit_status = Arc::new(AtomicBool::new(false));
    let wait_exit_status = Arc::clone(&exit_status);
    let exit_code = Arc::new(StdMutex::new(None));
    let wait_exit_code = Arc::clone(&exit_code);
    let wait_handle = tokio::spawn(async move {
        let code = exit_rx.await.unwrap_or(-1);
        wait_exit_status.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut guard) = wait_exit_code.lock() {
            *guard = Some(code);
        }
        let _ = exit_seen_tx.send(true);
        let _ = exit_tx.send(code);
    });

    let handle = ProcessHandle::new(
        writer_tx,
        Box::new(ClosureTerminator { inner: terminator }),
        reader_handle,
        stderr_reader_handle
            .map(|handle| handle.abort_handle())
            .into_iter()
            .collect(),
        writer_handle,
        wait_handle,
        exit_status,
        exit_code,
        resizer,
    );

    SpawnedProcess {
        session: handle,
        stdout_rx,
        stderr_rx,
        exit_rx: exit_rx_out,
    }
}
