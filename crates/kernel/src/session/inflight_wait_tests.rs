use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fast_cells_wake_budgeted_and_unbounded_waiters() {
    let mut command = Command::new("python");
    command.args([
        "-c",
        r#"
import json, sys
print(json.dumps({'event':'ready','protocol':3,'python':'fake'}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    if request['type'] == 'shutdown':
        break
    if request['type'] == 'execute':
        print(json.dumps({'event':'stdout','id':request['id'],'text':'probe\n'}), flush=True)
        print(json.dumps({'event':'done','id':request['id'],'status':'ok'}), flush=True)
"#,
    ]);
    let session = KernelSession::spawn_command(command, /*host_handler*/ None)
        .await
        .expect("fake kernel");
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut waiters = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let session = Arc::clone(&session);
            waiters.spawn(async move {
                let expected = CellOutput {
                    stdout: "probe\n".into(),
                    status: "ok".into(),
                    ..CellOutput::default()
                };
                for _ in 0..16 {
                    let cell = session.begin_execute("probe").await.expect("budgeted cell");
                    let outcome = cell.wait_for(Duration::from_secs(5)).await.expect("wait");
                    let CellWaitOutcome::Done(output) = outcome else {
                        panic!("a completed fast cell lost its completion notification");
                    };
                    assert_eq!(output, expected);
                    let cell = session
                        .begin_execute("probe")
                        .await
                        .expect("unbounded cell");
                    assert_eq!(cell.wait_until_done().await.expect("done"), expected);
                }
            });
        }
        while let Some(result) = waiters.join_next().await {
            result.expect("cell waiter");
        }
    })
    .await
    .expect("completed cells must wake every waiter promptly");
    session.shutdown().await.expect("shutdown fake kernel");
}

#[tokio::test]
async fn maintenance_try_execute_does_not_queue_behind_an_active_cell() {
    let mut command = Command::new("python");
    command.args([
        "-c",
        r#"
import json, sys, time
print(json.dumps({'event':'ready','protocol':3,'python':'fake'}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    if request['type'] == 'shutdown':
        break
    if request['type'] == 'execute':
        if request['code'] == 'parked':
            time.sleep(0.5)
        print(json.dumps({'event':'stdout','id':request['id'],'text':request['code']}), flush=True)
        print(json.dumps({'event':'done','id':request['id'],'status':'ok'}), flush=True)
"#,
    ]);
    let session = KernelSession::spawn_command(command, /*host_handler*/ None)
        .await
        .expect("fake kernel");
    let active = session.begin_execute("parked").await.expect("active");
    let skipped = tokio::time::timeout(
        Duration::from_millis(100),
        session.try_execute("must not queue"),
    )
    .await
    .expect("nonblocking maintenance")
    .expect("try execute");
    assert_eq!(skipped, None);
    assert_eq!(
        active.wait_until_done().await.expect("done").stdout,
        "parked"
    );
    assert_eq!(
        session
            .try_execute("idle maintenance")
            .await
            .expect("try execute")
            .unwrap()
            .stdout,
        "idle maintenance"
    );
    session.shutdown().await.expect("shutdown");
}
