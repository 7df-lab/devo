use serde_json::json;
use std::fmt::Write as _;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::path::Path;

use crate::invocation::FunctionToolOutput;

pub(crate) fn read_directory(
    path: &Path,
    limit: usize,
    offset: usize,
) -> anyhow::Result<FunctionToolOutput> {
    let mut items = std::fs::read_dir(path)?
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if is_dir { format!("{name}/") } else { name }
        })
        .collect::<Vec<_>>();
    items.sort_by_cached_key(|name| name.to_lowercase());

    let start = offset.saturating_sub(1);
    let (sliced, truncated) = if start < items.len() {
        let end = start.saturating_add(limit).min(items.len());
        (&items[start..end], end < items.len())
    } else {
        (&[][..], false)
    };

    let mut preview = String::new();
    for (index, item) in sliced.iter().take(20).enumerate() {
        if index > 0 {
            preview.push('\n');
        }
        preview.push_str(item);
    }

    let selected_bytes = sliced.iter().map(String::len).sum::<usize>();
    let mut display_content =
        String::with_capacity(selected_bytes + sliced.len().saturating_sub(1) + 128);
    for (index, item) in sliced.iter().enumerate() {
        if index > 0 {
            display_content.push('\n');
        }
        display_content.push_str(item);
    }
    if truncated {
        let _ = write!(
            display_content,
            "\n\n(Showing {} of {} entries. Use 'offset' parameter to read beyond entry {})",
            sliced.len(),
            items.len(),
            offset + sliced.len()
        );
    } else {
        let _ = write!(display_content, "\n\n({} entries)", items.len());
    }

    let output = format!(
        "<path>{}</path>\n<type>directory</type>\n<entries>\n{display_content}\n</entries>",
        path.display()
    );

    Ok(FunctionToolOutput::success_with_metadata(
        output,
        json!({
            "preview": preview,
            "truncated": truncated,
            "loaded": []
        }),
    )
    .with_display_content(display_content))
}

pub(crate) fn read_file(
    path: &Path,
    limit: usize,
    offset: usize,
) -> anyhow::Result<FunctionToolOutput> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let start = offset.saturating_sub(1);
    let mut raw = Vec::with_capacity(limit.min(1024));
    let mut bytes = 0usize;
    let mut count = 0usize;
    let mut cut = false;
    let mut more = false;

    for line in reader.lines() {
        let mut line = line?;
        count += 1;
        if count <= start {
            continue;
        }
        if raw.len() >= limit {
            more = true;
            continue;
        }
        // TODO: check the truncate policy
        if line.len() > 2000 {
            line.truncate(2000);
            line.push_str("... (line truncated to 2000 chars)");
        }
        let size = line.len() + if raw.is_empty() { 0 } else { 1 };
        if bytes + size > 50 * 1024 {
            cut = true;
            more = true;
            break;
        }
        raw.push(line);
        bytes += size;
    }

    if count < offset && !(count == 0 && offset == 1) {
        return Ok(FunctionToolOutput::error(format!(
            "Offset {} is out of range for this file ({} lines)",
            offset, count
        )));
    }

    let mut display_content = String::with_capacity(bytes + raw.len() * 16 + 128);
    for (index, line) in raw.iter().enumerate() {
        let _ = writeln!(display_content, "{}: {}", offset + index, line);
    }

    let last = offset + raw.len().saturating_sub(1);
    let next = last + 1;
    if cut {
        let _ = write!(
            display_content,
            "\n(Output capped at 50 KB. Showing lines {}-{}. Use offset={} to continue.)",
            offset, last, next
        );
    } else if more {
        let _ = write!(
            display_content,
            "\n(Showing lines {}-{} of {}. Use offset={} to continue.)",
            offset, last, count, next
        );
    } else {
        let _ = write!(display_content, "\n(End of file - total {count} lines)");
    }
    let mut preview = String::new();
    for (index, line) in raw.iter().take(20).enumerate() {
        if index > 0 {
            preview.push('\n');
        }
        preview.push_str(line);
    }
    let output = format!(
        "<path>{}</path>\n<type>file</type>\n<content>\n{display_content}\n</content>",
        path.display()
    );

    Ok(FunctionToolOutput::success_with_metadata(
        output,
        json!({
            "preview": preview,
            "truncated": cut || more,
            "loaded": []
        }),
    )
    .with_display_content(display_content))
}

pub(crate) fn is_binary_file(path: &Path) -> anyhow::Result<bool> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "zip"
            | "tar"
            | "gz"
            | "exe"
            | "dll"
            | "so"
            | "class"
            | "jar"
            | "war"
            | "7z"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "odt"
            | "ods"
            | "odp"
            | "bin"
            | "dat"
            | "obj"
            | "o"
            | "a"
            | "lib"
            | "wasm"
            | "pyc"
            | "pyo"
    ) {
        return Ok(true);
    }

    let mut file = File::open(path)?;
    let size = file.metadata()?.len() as usize;
    if size == 0 {
        return Ok(false);
    }

    let sample_size = size.min(4096);
    let mut bytes = vec![0u8; sample_size];
    let read = file.read(&mut bytes)?;
    if read == 0 {
        return Ok(false);
    }

    let mut non_printable = 0usize;
    for byte in bytes.iter().take(read) {
        if *byte == 0 {
            return Ok(true);
        }
        if *byte < 9 || (*byte > 13 && *byte < 32) {
            non_printable += 1;
        }
    }

    Ok((non_printable as f64) / (read as f64) > 0.3)
}

pub(crate) fn missing_file_message(filepath: &str) -> String {
    let path = Path::new(filepath);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(filepath);
    let base_lower = base.to_lowercase();

    let mut suggestions = String::new();
    let mut suggestion_count = 0usize;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let name_lower = name.to_lowercase();
            if !name_lower.contains(&base_lower) && !base_lower.contains(&name_lower) {
                continue;
            }
            if suggestion_count > 0 {
                suggestions.push('\n');
            }
            let _ = write!(suggestions, "{}", dir.join(name).display());
            suggestion_count += 1;
            if suggestion_count >= 3 {
                break;
            }
        }
    }

    if suggestion_count == 0 {
        format!("File not found: {filepath}")
    } else {
        format!(
            "File not found: {filepath}\n\nDid you mean one of these?\n{}",
            suggestions
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContent;
    use crate::invocation::ToolOutput;
    use pretty_assertions::assert_eq;
    use std::env;
    use std::fs::File;
    use std::fs::{self};
    use std::hint::black_box;
    use std::io::Write;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::Instant;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    fn create_temp_dir(prefix: &str) -> PathBuf {
        let mut path = env::temp_dir();
        let ticks = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("devo-tools-read-{prefix}-{ticks}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_lines(path: &Path, lines: &[&str]) {
        let mut file = File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn output_text(output: &FunctionToolOutput) -> &str {
        output.content.text_part().expect("text content")
    }

    fn output_metadata(output: &FunctionToolOutput) -> &serde_json::Value {
        match &output.content {
            ToolContent::Mixed {
                json: Some(metadata),
                ..
            } => metadata,
            content => panic!("expected mixed output metadata, got {content:?}"),
        }
    }

    #[test]
    fn read_directory_sorts_entries_and_reports_truncation() {
        let dir = create_temp_dir("dir");
        File::create(dir.join("b.txt")).unwrap();
        File::create(dir.join("a.txt")).unwrap();
        fs::create_dir_all(dir.join("subdir")).unwrap();

        let output = read_directory(&dir, 1, 2).unwrap();
        let text = output_text(&output);
        assert!(text.contains("<type>directory</type>"));
        assert!(text.contains("b.txt"));
        assert!(
            text.contains(
                "(Showing 1 of 3 entries. Use 'offset' parameter to read beyond entry 3)"
            )
        );
        assert_eq!(
            output.display_content(),
            Some(
                "b.txt\n\n(Showing 1 of 3 entries. Use 'offset' parameter to read beyond entry 3)"
            )
        );
        assert!(!output.display_content().unwrap().contains("<entries>"));

        assert_eq!(
            output_metadata(&output)
                .get("truncated")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[test]
    #[ignore]
    fn bench_read_directory_many_entries() {
        let dir = create_temp_dir("dir-bench");
        for idx in 0..400 {
            File::create(dir.join(format!("file-{idx:04}.rs"))).unwrap();
        }
        for idx in 0..40 {
            fs::create_dir_all(dir.join(format!("module-{idx:04}"))).unwrap();
        }
        let iterations = 5_000;
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            let output = black_box(read_directory(black_box(&dir), 400, 1)).expect("read dir");
            total_len += output.content.into_string().len();
        }

        let elapsed = started.elapsed();
        assert!(total_len > 0);
        println!(
            "read_directory_many_entries iterations={iterations} entries=440 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    fn read_file_applies_limit_and_reports_more() {
        let dir = create_temp_dir("file");
        let path = dir.join("sample.txt");
        write_lines(&path, &["line1", "line2", "line3", "line4", "line5"]);

        let output = read_file(&path, 2, 2).unwrap();
        assert!(!output.is_error);
        let text = output_text(&output);
        assert!(text.contains("2: line2"));
        assert!(text.contains("3: line3"));
        assert!(text.contains("(Showing lines 2-3 of 5. Use offset=4 to continue.)"));
        assert_eq!(
            output.display_content(),
            Some("2: line2\n3: line3\n\n(Showing lines 2-3 of 5. Use offset=4 to continue.)")
        );
        assert!(!output.display_content().unwrap().contains("<content>"));
        assert!(!output.display_content().unwrap().contains("<path>"));

        assert_eq!(
            output_metadata(&output)
                .get("truncated")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn read_file_reports_offset_out_of_range() {
        let dir = create_temp_dir("error");
        let path = dir.join("short.txt");
        write_lines(&path, &["hello", "world"]);

        let output = read_file(&path, 10, 5).unwrap();
        assert!(output.is_error);
        assert!(
            output
                .content
                .text_part()
                .is_some_and(|text| text.contains("Offset 5 is out of range"))
        );
    }

    #[test]
    #[ignore]
    fn bench_read_file_many_loaded_lines() {
        let dir = create_temp_dir("file-bench");
        let path = dir.join("large.txt");
        let mut content = String::new();
        for idx in 0..400 {
            let _ = writeln!(
                content,
                "line {idx}: repeated read tool output payload for formatting"
            );
        }
        fs::write(&path, content).unwrap();
        let iterations = 5_000;
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            let output = black_box(read_file(black_box(&path), 400, 1)).expect("read file");
            total_len += output.content.into_string().len();
        }

        let elapsed = started.elapsed();
        assert!(total_len > 0);
        println!(
            "read_file_many_loaded_lines iterations={iterations} lines=400 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    fn is_binary_file_detects_null_bytes() {
        let dir = create_temp_dir("binary");
        let path = dir.join("payload.bin");
        fs::write(&path, [0u8, 1, 2]).unwrap();

        assert!(is_binary_file(&path).unwrap());
    }

    #[test]
    fn missing_file_message_includes_suggestions() {
        let dir = create_temp_dir("missing");
        let target = dir.join("example.txt");
        write_lines(&target, &["content"]);

        let missing = dir.join("example");
        let message = missing_file_message(&missing.to_string_lossy());
        assert_eq!(
            message,
            format!(
                "File not found: {}\n\nDid you mean one of these?\n{}",
                missing.display(),
                target.display()
            )
        );
    }
}
