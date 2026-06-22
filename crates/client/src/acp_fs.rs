use std::path::Path;

use devo_protocol::ACP_FS_READ_TEXT_FILE_METHOD;
use devo_protocol::ACP_FS_WRITE_TEXT_FILE_METHOD;
use devo_protocol::AcpFsReadTextFileParams;
use devo_protocol::AcpFsReadTextFileResult;
use devo_protocol::AcpFsWriteTextFileParams;
use devo_protocol::AcpSuccessResponse;

pub(crate) async fn handle_acp_fs_request(
    request_id: serde_json::Value,
    method: &str,
    params: serde_json::Value,
) -> std::result::Result<serde_json::Value, String> {
    match method {
        ACP_FS_READ_TEXT_FILE_METHOD => {
            let params = serde_json::from_value::<AcpFsReadTextFileParams>(params)
                .map_err(|error| format!("invalid fs/read_text_file params: {error}"))?;
            let content = read_acp_text_file(&params.path, params.line, params.limit).await?;
            Ok(acp_fs_success_response(
                request_id,
                AcpFsReadTextFileResult {
                    content,
                    meta: None,
                },
            ))
        }
        ACP_FS_WRITE_TEXT_FILE_METHOD => {
            let params = serde_json::from_value::<AcpFsWriteTextFileParams>(params)
                .map_err(|error| format!("invalid fs/write_text_file params: {error}"))?;
            write_acp_text_file(&params.path, params.content).await?;
            Ok(acp_fs_success_response(request_id, serde_json::Value::Null))
        }
        _ => Err(format!("unknown ACP filesystem method {method}")),
    }
}

async fn read_acp_text_file(
    path: &Path,
    line: Option<u64>,
    limit: Option<u64>,
) -> std::result::Result<String, String> {
    validate_absolute_fs_path(ACP_FS_READ_TEXT_FILE_METHOD, path)?;
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| format!("failed to read text file {}: {error}", path.display()))?;
    select_text_lines(&content, line, limit)
}

async fn write_acp_text_file(path: &Path, content: String) -> std::result::Result<(), String> {
    validate_absolute_fs_path(ACP_FS_WRITE_TEXT_FILE_METHOD, path)?;
    tokio::fs::write(path, content)
        .await
        .map_err(|error| format!("failed to write text file {}: {error}", path.display()))
}

fn validate_absolute_fs_path(method: &str, path: &Path) -> std::result::Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{method} params.path must be absolute"));
    }
    Ok(())
}

fn select_text_lines(
    content: &str,
    line: Option<u64>,
    limit: Option<u64>,
) -> std::result::Result<String, String> {
    let start_line = line.unwrap_or(1);
    if start_line == 0 {
        return Err("fs/read_text_file params.line must be 1-based".to_string());
    }
    let skip_count = usize::try_from(start_line.saturating_sub(1)).unwrap_or(usize::MAX);
    let selected = content.split_inclusive('\n').skip(skip_count);
    let selected = match limit {
        Some(limit) => {
            let limit = usize::try_from(limit).unwrap_or(usize::MAX);
            selected.take(limit).collect()
        }
        None => selected.collect(),
    };
    Ok(selected)
}

fn acp_fs_success_response<T: serde::Serialize>(
    id: serde_json::Value,
    result: T,
) -> serde_json::Value {
    serde_json::to_value(AcpSuccessResponse::new(id, result))
        .expect("serialize ACP filesystem success response")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use pretty_assertions::assert_eq;

    use super::*;

    static ACP_FS_TEST_NEXT_ID: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn select_text_lines_preserves_line_endings_and_applies_range() {
        assert_eq!(
            select_text_lines("first\r\nsecond\nthird", Some(2), Some(2))
                .expect("line selection succeeds"),
            "second\nthird"
        );
    }

    #[test]
    fn select_text_lines_rejects_zero_line() {
        assert_eq!(
            select_text_lines("first\n", Some(0), None).expect_err("line zero is rejected"),
            "fs/read_text_file params.line must be 1-based"
        );
    }

    #[tokio::test]
    async fn read_text_file_requires_absolute_path() {
        let error = handle_acp_fs_request(
            serde_json::json!(1),
            ACP_FS_READ_TEXT_FILE_METHOD,
            serde_json::to_value(AcpFsReadTextFileParams {
                session_id: devo_protocol::SessionId::new(),
                path: PathBuf::from("relative.txt"),
                line: None,
                limit: None,
                meta: None,
            })
            .expect("serialize read params"),
        )
        .await
        .expect_err("relative path is rejected");

        assert_eq!(
            error,
            "fs/read_text_file params.path must be absolute".to_string()
        );
    }

    #[tokio::test]
    async fn write_then_read_text_file_with_line_limit() {
        let path = temp_test_path();
        let _ = tokio::fs::remove_file(&path).await;
        let session_id = devo_protocol::SessionId::new();

        let write_response = handle_acp_fs_request(
            serde_json::json!(2),
            ACP_FS_WRITE_TEXT_FILE_METHOD,
            serde_json::to_value(AcpFsWriteTextFileParams {
                session_id,
                path: path.clone(),
                content: "first\nsecond\nthird".to_string(),
                meta: None,
            })
            .expect("serialize write params"),
        )
        .await
        .expect("write succeeds");
        assert_eq!(
            write_response,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": null
            })
        );

        let read_response = handle_acp_fs_request(
            serde_json::json!(3),
            ACP_FS_READ_TEXT_FILE_METHOD,
            serde_json::to_value(AcpFsReadTextFileParams {
                session_id,
                path: path.clone(),
                line: Some(2),
                limit: Some(1),
                meta: None,
            })
            .expect("serialize read params"),
        )
        .await
        .expect("read succeeds");

        assert_eq!(
            serde_json::from_value::<AcpSuccessResponse<AcpFsReadTextFileResult>>(read_response)
                .expect("decode read response"),
            AcpSuccessResponse::new(
                serde_json::json!(3),
                AcpFsReadTextFileResult {
                    content: "second\n".to_string(),
                    meta: None,
                }
            )
        );

        tokio::fs::remove_file(path)
            .await
            .expect("remove temp file");
    }

    fn temp_test_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "devo-acp-fs-{}-{}.txt",
            std::process::id(),
            ACP_FS_TEST_NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ))
    }
}
