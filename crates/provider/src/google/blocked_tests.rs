use super::*;
use devo_protocol::{ModelProfileKey, SamplingControls};
use futures::StreamExt;
use pretty_assertions::assert_eq;

#[test]
fn prompt_blocks_are_non_retryable_and_redact_untrusted_details() {
    for reason in [
        "SAFETY",
        "OTHER",
        "BLOCKLIST",
        "PROHIBITED_CONTENT",
        "IMAGE_SAFETY",
        "PRIVATE_SENTINEL",
    ] {
        let error = parse_response(json!({
            "promptFeedback": {"blockReason": reason, "blockReasonMessage": "PRIVATE_SENTINEL"},
            "candidates": []
        }))
        .expect_err("explicit block must fail");
        let error = error
            .downcast_ref::<ProviderError>()
            .expect("structured error");
        assert_eq!(error.error_code(), "CONTENT_FILTERED_ERROR");
        assert!(!error.is_recoverable());
        assert!(!error.is_transient());
        assert!(!format!("{error:?}").contains("PRIVATE_SENTINEL"));
        assert_eq!(error.recovery_hint(), None);
    }
}

#[test]
fn absent_or_unspecified_prompt_block_keeps_existing_response() {
    let expected = parse_response(json!({"candidates": []})).unwrap();
    for feedback in [
        json!({}),
        json!({"blockReason": ""}),
        json!({"blockReason": "BLOCK_REASON_UNSPECIFIED"}),
        json!({"blockReason": null}),
    ] {
        assert_eq!(
            parse_response(json!({"promptFeedback": feedback, "candidates": []})).unwrap(),
            expected
        );
    }
    let response = parse_response(json!({
        "promptFeedback": {"safetyRatings": []},
        "candidates": [{"content": {"parts": [{"text": "Answer"}]}, "finishReason": "STOP"}]
    }))
    .unwrap();
    assert_eq!(
        response.content,
        vec![ResponseContent::Text("Answer".into())]
    );
    assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
}

#[tokio::test]
async fn blocked_http_completion_and_stream_keep_failure_and_usage() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock");
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for (index, include_usage) in [true, true, false, true, false, true, true]
            .into_iter()
            .enumerate()
        {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let bytes_read = socket.read(&mut request).await.unwrap();
            assert!(bytes_read > 0);
            let mut body = json!({"promptFeedback": {"blockReason": "SAFETY", "blockReasonMessage": "PRIVATE_SENTINEL"}});
            if index == 5 {
                body = json!({"candidates": [{"content": {"parts": [{"text": "Answer"}]}, "finishReason": "STOP"}]});
            }
            if include_usage {
                body["usageMetadata"] = if index == 6 {
                    json!({"promptTokenCount": 0, "totalTokenCount": 0})
                } else {
                    json!({"promptTokenCount": 3, "totalTokenCount": 3})
                };
            }
            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let provider = GoogleGenerativeAiProvider::new(format!("http://{address}"), "fixture-key");
    let request = ModelRequest {
        model_slug: ModelProfileKey::Generic,
        model: "gemini-2.5-flash".into(),
        system: None,
        messages: vec![],
        max_tokens: 64,
        tools: None,
        hosted_tools: vec![],
        sampling: SamplingControls::default(),
        request_thinking: None,
        reasoning_effort: None,
        extra_body: None,
    };
    let error = provider
        .completion(request.clone())
        .await
        .expect_err("HTTP200 prompt block is failure");
    assert!(matches!(
        error.downcast_ref::<ProviderError>(),
        Some(ProviderError::ContentFilteredError { .. })
    ));
    assert!(!format!("{error:?}").contains("PRIVATE_SENTINEL"));

    let mut events = provider.completion_stream(request.clone()).await.unwrap();
    let first = events.next().await.unwrap().unwrap();
    let mut usage =
        parse_usage(&json!({"usageMetadata": {"promptTokenCount": 3, "totalTokenCount": 3}}));
    usage.terminal_sample = true;
    assert_eq!(first, StreamEvent::UsageDelta(usage));
    let error = events
        .next()
        .await
        .unwrap()
        .expect_err("stream fails after usage");
    assert!(matches!(
        error.downcast_ref::<ProviderError>(),
        Some(ProviderError::ContentFilteredError { .. })
    ));
    assert!(!format!("{error:?}").contains("PRIVATE_SENTINEL"));
    assert!(events.next().await.is_none(), "no successful MessageDone");

    let mut events = provider.completion_stream(request.clone()).await.unwrap();
    assert!(
        events.next().await.unwrap().is_err(),
        "no invented usage when absent"
    );
    assert!(events.next().await.is_none());
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = observed.clone();
    let observer: UsageObserver = std::sync::Arc::new(move |usage| {
        captured.lock().unwrap().push(usage.clone());
    });
    let error = provider
        .completion_with_usage_observer(request.clone(), observer.clone())
        .await
        .expect_err("blocked observed completion");
    assert!(matches!(
        error.downcast_ref::<ProviderError>(),
        Some(ProviderError::ContentFilteredError { .. })
    ));
    let mut expected_usage =
        parse_usage(&json!({"usageMetadata": {"promptTokenCount": 3, "totalTokenCount": 3}}));
    expected_usage.terminal_sample = true;
    assert_eq!(*observed.lock().unwrap(), vec![expected_usage]);
    assert!(
        provider
            .completion_with_usage_observer(request.clone(), observer.clone())
            .await
            .is_err()
    );
    assert_eq!(
        observed.lock().unwrap().len(),
        1,
        "absent usage is not invented"
    );
    let response = provider
        .completion_with_usage_observer(request.clone(), observer.clone())
        .await
        .unwrap();
    assert_eq!(
        response.content,
        vec![ResponseContent::Text("Answer".into())]
    );
    {
        let samples = observed.lock().unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[1], response.usage);
    }
    assert!(
        provider
            .completion_with_usage_observer(request, observer)
            .await
            .is_err()
    );
    {
        let samples = observed.lock().unwrap();
        assert_eq!(samples.len(), 3);
        let mut zero_usage =
            parse_usage(&json!({"usageMetadata": {"promptTokenCount": 0, "totalTokenCount": 0}}));
        zero_usage.terminal_sample = true;
        assert_eq!(samples[2], zero_usage);
    }
    server.await.unwrap();
}
