//! Host-owned, bounded control and cursor output for interactive shell tasks.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const OUTPUT_CAP: usize = 256 * 1024;
const MAX_WRITE: usize = 64 * 1024;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

type Reply = oneshot::Sender<Result<usize, String>>;
pub(super) enum Control {
    Write(Vec<u8>, Reply),
    Close(Reply),
    Resize(u16, u16, Reply),
}

struct State {
    ready: bool,
    finished: bool,
    incomplete: bool,
    sender: Option<mpsc::Sender<Control>>,
    output: VecDeque<(u8, u8)>,
    last_activity: Instant,
    end: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            ready: false,
            finished: false,
            incomplete: false,
            sender: None,
            output: VecDeque::new(),
            end: 0,
            last_activity: Instant::now(),
        }
    }
}

struct Inner {
    tty: bool,
    state: Mutex<State>,
    changed: Notify,
    stopped: CancellationToken,
}

/// An opaque host-owned I/O capability. Clone shares state, not read cursors.
#[derive(Clone)]
pub struct InteractiveIo(Arc<Inner>);

impl std::fmt::Debug for InteractiveIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InteractiveIo")
            .field("tty", &self.0.tty)
            .finish_non_exhaustive()
    }
}

impl Default for InteractiveIo {
    fn default() -> Self {
        Self::new(/*tty*/ false)
    }
}

impl InteractiveIo {
    pub fn new(tty: bool) -> Self {
        Self(Arc::new(Inner {
            tty,
            state: Mutex::new(State::default()),
            changed: Notify::new(),
            stopped: CancellationToken::new(),
        }))
    }

    #[cfg(test)]
    pub(super) fn owner_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }

    pub fn idle_for(&self) -> Duration {
        self.0.state.lock().unwrap().last_activity.elapsed()
    }

    pub(super) fn activity(&self) {
        self.0.state.lock().unwrap().last_activity = Instant::now();
    }

    pub fn is_ready(&self) -> bool {
        self.0.state.lock().unwrap().ready
    }

    pub async fn ready(&self) -> Result<(), String> {
        loop {
            let notified = self.0.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = self.0.state.lock().unwrap();
                if state.ready {
                    return Ok(());
                }
                if state.finished {
                    return Err("interactive process ended before becoming ready".into());
                }
            }
            notified.await;
        }
    }

    pub(super) fn connect(&self) -> Result<mpsc::Receiver<Control>, String> {
        let (sender, receiver) = mpsc::channel(1);
        let mut state = self.0.state.lock().unwrap();
        if state.finished || state.ready {
            return Err("interactive I/O cannot be relaunched".into());
        }
        state.sender = Some(sender);
        state.ready = true;
        state.last_activity = Instant::now();
        drop(state);
        self.0.changed.notify_waiters();
        Ok(receiver)
    }

    pub fn finish(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.finished = true;
        state.sender = None;
        drop(state);
        self.0.stopped.cancel();
        self.0.changed.notify_waiters();
    }

    pub(super) fn mark_incomplete(&self) {
        self.0.state.lock().unwrap().incomplete = true;
    }

    pub(super) fn stopped(&self) -> CancellationToken {
        self.0.stopped.clone()
    }

    pub(super) fn append(&self, bytes: &[u8], stream: &'static str) {
        let mut state = self.0.state.lock().unwrap();
        if state.finished {
            return;
        }
        state.end = state.end.saturating_add(bytes.len() as u64);
        state.last_activity = Instant::now();
        let stream = match stream {
            "stdout" => 0,
            "stderr" => 1,
            _ => 2,
        };
        let bytes = &bytes[bytes.len().saturating_sub(OUTPUT_CAP)..];
        let discard = (state.output.len() + bytes.len()).saturating_sub(OUTPUT_CAP);
        state.output.drain(..discard);
        state.output.extend(bytes.iter().map(|b| (*b, stream)));
        drop(state);
        self.0.changed.notify_waiters();
    }

    async fn control(&self, make: impl FnOnce(Reply) -> Control) -> Result<usize, String> {
        self.ready().await?;
        let sender = self
            .0
            .state
            .lock()
            .unwrap()
            .sender
            .clone()
            .ok_or("interactive process has exited")?;
        let (reply, result) = oneshot::channel();
        // A full queue is not an unbounded producer buffer. Timeout before send
        // guarantees no submission; after send, delivery can be partial/unknown.
        tokio::time::timeout(CONTROL_TIMEOUT, sender.send(make(reply)))
            .await
            .map_err(|_| "interactive control queue busy; request was not submitted".to_string())?
            .map_err(|_| "interactive process input is closed".to_string())?;
        drop(sender);
        tokio::time::timeout(CONTROL_TIMEOUT, result).await
            .map_err(|_| "interactive control timed out; delivery may be partial, do not automatically resend".to_string())?
            .map_err(|_| "interactive process closed; delivery may be partial".to_string())?
    }

    pub async fn write(&self, bytes: Vec<u8>) -> Result<usize, String> {
        if bytes.len() > MAX_WRITE {
            return Err("write exceeds 65536 bytes".into());
        }
        let written = self.control(|reply| Control::Write(bytes, reply)).await?;
        if written > 0 {
            self.activity();
        }
        Ok(written)
    }

    pub async fn close_stdin(&self) -> Result<(), String> {
        if self.0.tty {
            return Err(
                "PTY has no portable stdin half-close; send explicit terminal input or terminate"
                    .into(),
            );
        }
        self.control(Control::Close).await.map(|_| ())
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        if !self.0.tty {
            return Err("resize requires PTY mode".into());
        }
        if cols == 0 || rows == 0 {
            return Err("terminal size must be positive".into());
        }
        self.control(|reply| Control::Resize(cols, rows, reply))
            .await
            .map(|_| ())
    }

    /// Cursor counts observed bytes across streams; a gap means old bytes expired.
    /// Text is a lossy view; chunks contain base64 bytes for lossless decoding.
    pub async fn read(&self, cursor: u64, max_bytes: usize, wait_ms: u64) -> Value {
        use base64::Engine;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms.min(30_000));
        loop {
            let notified = self.0.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = self.0.state.lock().unwrap();
                if cursor > state.end {
                    return json!({"error":"cursor is beyond current output"});
                }
                if state.end > cursor || state.finished || tokio::time::Instant::now() >= deadline {
                    let begin = state.end.saturating_sub(state.output.len() as u64);
                    let start = cursor.max(begin);
                    let mut bytes: Vec<_> = state
                        .output
                        .iter()
                        .skip((start - begin) as usize)
                        .take(max_bytes.clamp(1, MAX_WRITE))
                        .copied()
                        .collect();
                    let mut groups: Vec<(&str, Vec<u8>)> = Vec::new();
                    let mut returned = 0;
                    for (byte, code) in &bytes {
                        let stream = match code {
                            0 => "stdout",
                            1 => "stderr",
                            _ => "pty",
                        };
                        if let Some((last_stream, data)) = groups.last_mut()
                            && *last_stream == stream
                        {
                            data.push(*byte);
                        } else {
                            if groups.len() == 256 {
                                break;
                            }
                            groups.push((stream, vec![*byte]));
                        }
                        returned += 1;
                    }
                    bytes.truncate(returned);
                    let chunks: Vec<_> = groups.into_iter().map(|(stream, data)| json!({"stream":stream,"data_base64":base64::engine::general_purpose::STANDARD.encode(&data),"text":String::from_utf8_lossy(&data)})).collect();
                    let raw: Vec<_> = bytes.iter().map(|(byte, _)| *byte).collect();
                    return json!({"text":String::from_utf8_lossy(&raw),"chunks":chunks,"next_cursor":start + bytes.len() as u64,"gap":cursor < begin,"truncated":cursor < begin || state.incomplete,"data":base64::engine::general_purpose::STANDARD.encode(&raw),"eof":state.finished && start + bytes.len() as u64 == state.end});
                }
            }
            let _ = tokio::time::timeout_at(deadline, notified).await;
        }
    }
}

/// Marks failed launches and cancelled execution futures terminal as well.
pub(super) struct FinishIo(pub Option<InteractiveIo>);
impl Drop for FinishIo {
    fn drop(&mut self) {
        if let Some(io) = &self.0 {
            io.finish();
        }
    }
}

pub(super) fn pipe_writer(
    mut receiver: mpsc::Receiver<Control>,
    mut stdin: Option<tokio::process::ChildStdin>,
    io: InteractiveIo,
) {
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let stopped = io.stopped();
        loop {
            let command = tokio::select! { biased; _ = stopped.cancelled() => break, command = receiver.recv() => command };
            let Some(command) = command else {
                break;
            };
            match command {
                Control::Write(bytes, reply) => {
                    if reply.is_closed() {
                        continue;
                    }
                    let result = if let Some(stdin) = &mut stdin {
                        tokio::select! { biased;
                            _ = stopped.cancelled() => Err("process ended; input may be partial".into()),
                            result = async {
                                let mut written = 0;
                                while written < bytes.len() {
                                    let n = stdin.write(&bytes[written..]).await?;
                                    if n == 0 { return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "stdin write returned zero")); }
                                    written += n;
                                    io.activity();
                                }
                                Ok::<usize, std::io::Error>(written)
                            } => result.map_err(|error| format!("input write failed (possibly partial): {error}")),
                        }
                    } else {
                        Err("stdin is closed".into())
                    };
                    let _ = reply.send(result);
                }
                Control::Close(reply) => {
                    if reply.is_closed() {
                        continue;
                    }
                    stdin.take();
                    let _ = reply.send(Ok(0));
                }
                Control::Resize(_, _, reply) => {
                    let _ = reply.send(Err("resize requires PTY mode".into()));
                }
            }
        }
    });
}
