use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
#[cfg(unix)]
async fn slow_consumer_preserves_original_size_and_reports_buffer_loss() {
    let (process, mut rx) = UnifiedExecProcess::spawn(
        150,
        "printf BEGIN; head -c 2097152 /dev/zero; printf END",
        Path::new("."),
        /*shell*/ Some("/bin/sh"),
        /*login*/ false,
        /*tty*/ false,
    )
    .await
    .expect("spawn should succeed");
    tokio::time::timeout(Duration::from_secs(10), async {
        while process.is_running() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("output producer should exit without a consumer");

    let output = collect_output(&mut rx, &process, 250, 1_000_000).await;
    assert_eq!(
        (
            output.output.len(),
            output.truncated,
            output.original_token_count,
            output.exit_code,
        ),
        (1_048_576, true, approximate_token_count(2_097_160), Some(0)),
    );
    assert!(output.output.starts_with("BEGIN"));
    assert!(output.output.ends_with("END"));

    let empty = collect_output(&mut rx, &process, 250, 1_000_000).await;
    assert_eq!(
        (empty.output, empty.truncated, empty.original_token_count),
        (String::new(), false, 0),
    );

    // Entering the tail does not imply loss: the combined capacity is 1 MiB.
    process
        .output_buffer
        .lock()
        .await
        .push(&vec![b'x'; 600_000]);
    let retained = collect_output(&mut rx, &process, 250, 1_000_000).await;
    assert_eq!(
        (
            retained.output.len(),
            retained.truncated,
            retained.original_token_count
        ),
        (600_000, false, approximate_token_count(600_000)),
    );

    // Also preserve losses arriving during the final trailing-output grace drain.
    let trailing = async {
        tokio::task::yield_now().await;
        process
            .output_buffer
            .lock()
            .await
            .push(&vec![b'y'; 2_097_152]);
    };
    let (output, ()) = tokio::join!(collect_output(&mut rx, &process, 250, 1_000_000), trailing,);
    assert_eq!(
        (
            output.output.len(),
            output.truncated,
            output.original_token_count
        ),
        (1_048_576, true, approximate_token_count(2_097_152)),
    );
}
