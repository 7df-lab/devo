//! Shared helpers for decoding native image `UserInput` into local files.

/// Writes a `data:` image URI to a temp file under `devo-attachments`.
///
/// Only base64-encoded data URIs are accepted. `mime_type` overrides the
/// header mime when present and non-empty; otherwise the URI header (or
/// `image/png`) is used for the file extension.
pub(crate) fn write_data_uri_image_to_temp(
    uri: &str,
    mime_type: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    use base64::Engine;

    if !uri.starts_with("data:") {
        return Err("only data: image URIs are supported".to_string());
    }
    let rest = uri
        .strip_prefix("data:")
        .expect("data URI prefix checked above");
    let (header, data) = rest
        .split_once(',')
        .ok_or_else(|| "invalid data URI: missing payload".to_string())?;
    let (header_mime, is_base64) = if let Some(mime) = header.strip_suffix(";base64") {
        (mime, true)
    } else {
        (header, false)
    };
    let mime = mime_type
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| (!header_mime.is_empty()).then(|| header_mime.to_string()))
        .unwrap_or_else(|| "image/png".to_string());
    let bytes = if is_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .map_err(|error| format!("failed to decode image data URI: {error}"))?
    } else {
        return Err("only base64-encoded data URIs are supported".to_string());
    };
    let ext = match mime.as_str() {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    };
    let dir = std::env::temp_dir().join("devo-attachments");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
    std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
    Ok(path)
}
