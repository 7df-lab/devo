//! Shared file-change metadata for `write` / `edit` tool results.

use std::path::Path;
use std::time::SystemTime;

use diffy::PatchFormatter;
use diffy::create_patch;
use serde_json::json;

pub(crate) fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

pub(crate) fn build_write_metadata(
    path: &std::path::Path,
    previous: Option<&str>,
    content: &str,
) -> serde_json::Value {
    match previous {
        None => {
            let additions = content.lines().count();
            let mut added_content = String::with_capacity(content.len() + additions);
            for (index, line) in content.lines().enumerate() {
                if index > 0 {
                    added_content.push('\n');
                }
                added_content.push('+');
                added_content.push_str(line);
            }
            json!({
                "diff": format!(
                    "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n@@ -0,0 +1,{1} @@\n{2}",
                    path.display(),
                    additions,
                    added_content
                ),
                "files": [{
                    "path": path.display().to_string(),
                    "filePath": path.display().to_string(),
                    "kind": "add",
                    "content": content,
                    "additions": additions,
                    "deletions": 0
                }]
            })
        }
        Some(old) => {
            let patch = create_patch(old, content);
            let patch_text = PatchFormatter::new().fmt_patch(&patch).to_string();
            let additions = content.lines().count();
            let deletions = old.lines().count();
            json!({
                "diff": format!(
                    "diff --git a/{0} b/{0}\n{1}",
                    path.display(),
                    patch_text
                ),
                "files": [{
                    "path": path.display().to_string(),
                    "filePath": path.display().to_string(),
                    "kind": "update",
                    "content": content,
                    "postContent": content,
                    "post_content": content,
                    "oldContent": old,
                    "preContent": old,
                    "pre_content": old,
                    "additions": additions,
                    "deletions": deletions
                }]
            })
        }
    }
}

pub(crate) fn write_tool_result(
    path: &std::path::Path,
    previous: Option<&str>,
    content: &str,
    summary: String,
) -> crate::contracts::ToolResult {
    use crate::contracts::{ToolResult, ToolResultContent};

    let metadata = build_write_metadata(path, previous, content);
    let mut result = ToolResult::success(
        ToolResultContent::Mixed {
            text: Some(summary.clone()),
            json: Some(metadata),
        },
        &summary,
    );
    result.display_content = Some(summary);
    result
}
