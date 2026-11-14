//! Bounded, best-effort name discovery. Never execute Python or snapshot values.

use super::*;

impl KernelSession {
    /// Read a bounded sample of live top-level names without inspecting values.
    ///
    /// Busy kernels are skipped, not interrupted. A missing/older/unresponsive
    /// runtime yields `None`; it never starts or restores a kernel. This is an
    /// incomplete observation, not proof that an omitted binding is absent.
    pub async fn namespace_brief(&self) -> Option<Vec<String>> {
        if !self.ready.supports_namespace_brief {
            // Never probe an old v3 runtime: its unknown-request ProtocolError
            // is untagged and could otherwise arrive during the next cell.
            return None;
        }
        let _gate = self.execute_gate.try_lock().ok()?;
        let mut stdin = Arc::clone(&self.io.stdin).try_lock_owned().ok()?;
        tokio::time::timeout(Duration::from_millis(200), async {
            let id = format!("b{}", self.cell_seq.fetch_add(1, Ordering::Relaxed) + 1);
            self.set_active_request(id.clone(), /*host_handler*/ None)
                .await;
            let request = KernelRequest::NamespaceBrief { id: id.clone() };
            // write_all is not cancellation-safe. Let an admitted frame finish
            // under its owned stdin lock even if this observation times out;
            // dropping it mid-frame would corrupt the next execute request.
            tokio::spawn(async move { write_request(&mut stdin, &request).await })
                .await
                .ok()?
                .ok()?;
            // Do not collect arbitrary stdout, stderr, reprs, or display values.
            // Late replies from a timed-out brief cannot become another cell's
            // result: all collectors and the event pump fence on request id.
            let mut events = self.io.events.lock().await;
            loop {
                let event = events.recv().await?.ok()?;
                if let KernelEvent::Done {
                    id: event_id,
                    status,
                    names,
                    ..
                } = event
                    && event_id == id
                {
                    if status != "ok" {
                        return None;
                    }
                    return Some(
                        names?
                            .into_iter()
                            .take(32)
                            .filter(|name| {
                                !name.is_empty()
                                    && name.len() <= 64
                                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                            })
                            .collect(),
                    );
                }
            }
        })
        .await
        .ok()
        .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn namespace_brief_reads_real_runtime_without_touching_objects() {
        let runtime = default_runtime_pythonpath(Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("runtime source");
        let session = KernelSession::spawn(KernelSessionConfig {
            python: PathBuf::from("python"),
            python_path_entries: vec![runtime],
            ..KernelSessionConfig::default()
        })
        .await
        .expect("real CPython runtime");
        let output = session
            .execute(
                r#"
class Untouchable:
    def __repr__(self):
        raise AssertionError('must not inspect values')
    def __reduce__(self):
        raise AssertionError('must not serialize')
opaque_handle = Untouchable()
_secret = 'must not emit'
binding = 'VALUE_NOT_FOR_THE_MODEL'
_original = id(opaque_handle)
"#,
            )
            .await
            .expect("define state");
        assert_eq!(output.status, "ok");
        assert_eq!(
            session.namespace_brief().await,
            Some(vec![
                "Untouchable".into(),
                "binding".into(),
                "opaque_handle".into()
            ])
        );
        let output = session.execute("assert id(opaque_handle) == _original; assert binding == 'VALUE_NOT_FOR_THE_MODEL'")
            .await.expect("state survived");
        assert_eq!(
            output,
            CellOutput {
                status: "ok".into(),
                ..CellOutput::default()
            }
        );
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn namespace_brief_busy_and_late_reply_do_not_interrupt_or_poison_cells() {
        let mut command = Command::new("python");
        command.args(["-c", r#"
import json, sys, time
print(json.dumps({'event':'ready','protocol':3,'python':'fake','supports_namespace_brief':True}), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    if req['type'] == 'shutdown':
        break
    if req['type'] == 'namespace_brief':
        time.sleep(0.35)
        print(json.dumps({'event':'done','id':req['id'],'status':'ok','names':['stale']}), flush=True)
    elif req['type'] == 'execute':
        print(json.dumps({'event':'stdout','id':req['id'],'text':req['code']}), flush=True)
        print(json.dumps({'event':'done','id':req['id'],'status':'ok'}), flush=True)
    else:
        raise AssertionError('must never interrupt or snapshot')
"#]);
        let session = KernelSession::spawn_command(command, /*host_handler*/ None)
            .await
            .expect("fake runtime");
        let gate = session.execute_gate.lock().await;
        assert_eq!(session.namespace_brief().await, None);
        drop(gate);
        let stdin = session.io.stdin.lock().await;
        assert_eq!(session.namespace_brief().await, None);
        drop(stdin);
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(300), session.namespace_brief())
                .await
                .expect("200ms budget"),
            None
        );
        let output = tokio::time::timeout(Duration::from_secs(2), session.execute("next"))
            .await
            .expect("next cell proceeds")
            .expect("next cell");
        assert_eq!(
            output,
            CellOutput {
                stdout: "next".into(),
                status: "ok".into(),
                ..CellOutput::default()
            }
        );
        session.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn namespace_brief_never_probes_an_older_v3_runtime() {
        let mut command = Command::new("python");
        command.args(["-c", r#"
import json, sys, time
print(json.dumps({'event':'ready','protocol':3,'python':'old'}), flush=True)
for line in sys.stdin:
    req = json.loads(line)
    if req['type'] == 'shutdown':
        break
    if req['type'] == 'namespace_brief':
        time.sleep(0.35)
        print(json.dumps({'event':'error','id':None,'ename':'ProtocolError','evalue':'unknown request type','traceback':[]}), flush=True)
    elif req['type'] == 'execute':
        print(json.dumps({'event':'stdout','id':req['id'],'text':req['code']}), flush=True)
        print(json.dumps({'event':'done','id':req['id'],'status':'ok'}), flush=True)
"#]);
        let session = KernelSession::spawn_command(command, /*host_handler*/ None)
            .await
            .expect("old v3 runtime");
        assert_eq!(session.namespace_brief().await, None);
        assert_eq!(
            session.execute("next").await.expect("next cell"),
            CellOutput {
                stdout: "next".into(),
                status: "ok".into(),
                ..CellOutput::default()
            }
        );
        session.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    enum Abandonment {
        Timeout,
        CallerCancellation,
    }

    // A real OS pipe is filled while the Python reader is held behind a file
    // barrier. The small brief frame really blocks in write_all, not merely
    // behind a mutex or waiting for a deliberately delayed Done.
    #[cfg(unix)]
    async fn assert_blocked_writer_survives(abandonment: Abandonment) {
        let temp = tempfile::tempdir().expect("tempdir");
        let release = temp.path().join("release-reader");
        let mut command = Command::new("python");
        command.args(["-c", r#"
import json, pathlib, sys, time
print(json.dumps({'event':'ready','protocol':3,'python':'blocked','supports_namespace_brief':True}), flush=True)
while not pathlib.Path(sys.argv[1]).exists():
    time.sleep(0.005)
for line in sys.stdin:
    req = json.loads(line)
    if req['type'] == 'shutdown':
        break
    if req['type'] == 'namespace_brief':
        print(json.dumps({'event':'done','id':req['id'],'status':'ok','names':['old']}), flush=True)
    elif req['type'] == 'execute':
        print(json.dumps({'event':'stdout','id':req['id'],'text':req['code']}), flush=True)
        print(json.dumps({'event':'done','id':req['id'],'status':'ok'}), flush=True)
"#]).arg(&release);
        let session = KernelSession::spawn_command(command, /*host_handler*/ None)
            .await
            .expect("blocked reader");
        let mut stdin = session.io.stdin.lock().await;
        let mut blocked = false;
        for _ in 0..1024 {
            // write (unlike write_all) is cancellation-safe. Leading JSON
            // whitespace fills the pipe without creating a separate request.
            if tokio::time::timeout(Duration::from_millis(20), stdin.write(&[b' '; 4096]))
                .await
                .is_err()
            {
                blocked = true;
                break;
            }
        }
        assert!(blocked, "test must saturate the real pipe");
        drop(stdin);
        let brief_session = Arc::clone(&session);
        let brief = tokio::spawn(async move { brief_session.namespace_brief().await });
        match abandonment {
            Abandonment::Timeout => {
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(1), brief)
                        .await
                        .expect("bounded caller wait")
                        .expect("brief task"),
                    None
                );
            }
            Abandonment::CallerCancellation => {
                tokio::time::timeout(Duration::from_millis(150), async {
                    loop {
                        if session
                            .active_request
                            .lock()
                            .await
                            .as_ref()
                            .is_some_and(|request| request.id.starts_with('b'))
                            && session.io.stdin.try_lock().is_err()
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("brief write admitted");
                brief.abort();
                assert!(brief.await.expect_err("caller cancelled").is_cancelled());
            }
        }
        // The admitted frame's writer still owns stdin after its caller leaves.
        assert!(session.io.stdin.try_lock().is_err());
        std::fs::write(&release, b"read").expect("release reader");
        let next = tokio::time::timeout(Duration::from_secs(2), session.execute("next"))
            .await
            .expect("writer finishes and next cell proceeds")
            .expect("next cell");
        assert_eq!(
            next,
            CellOutput {
                stdout: "next".into(),
                status: "ok".into(),
                ..CellOutput::default()
            }
        );
        session.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn namespace_brief_timeout_during_real_blocked_write_keeps_frame_intact() {
        assert_blocked_writer_survives(Abandonment::Timeout).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn namespace_brief_caller_cancellation_during_real_blocked_write_keeps_frame_intact() {
        assert_blocked_writer_survives(Abandonment::CallerCancellation).await;
    }
}
