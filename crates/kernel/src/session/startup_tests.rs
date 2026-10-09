use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn startup_stderr_before_ready_does_not_break_handshake() {
    let mut child = Command::new("python")
        .args([
            "-c",
            r#"import json; print(json.dumps({'event':'stderr','text':'Startup notice\n'})); print(json.dumps({'event':'ready','protocol':3,'python':'test'}))"#,
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("fake kernel");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    assert_eq!(
        read_ready(&mut stdout)
            .await
            .expect("ready after diagnostic"),
        ReadyEvent {
            protocol: PROTOCOL_VERSION,
            python: "test".into(),
        },
    );
    assert!(child.wait().await.expect("fake kernel exit").success());
}

#[tokio::test]
async fn startup_failure_retains_the_complete_protocol_traceback() {
    let mut child = Command::new("python")
        .args([
            "-c",
            r#"import json; print(json.dumps({'event':'stderr','text':'Traceback:\n'})); print(json.dumps({'event':'stderr','text':'No usable temporary directory\n'}))"#,
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("fake kernel");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    assert_eq!(
        read_ready(&mut stdout)
            .await
            .expect_err("no ready event")
            .to_string(),
        "protocol handshake failed: kernel closed unexpectedly; startup stderr: Traceback:\nNo usable temporary directory\n",
    );
    assert!(child.wait().await.expect("fake kernel exit").success());
}
