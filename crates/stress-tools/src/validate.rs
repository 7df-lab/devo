//! Validate every JSONL line with the production v2 rollout parser.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result, bail};
use devo_core::parse_rollout_line;

pub fn run(corpus: &Path) -> Result<()> {
    if !corpus.is_dir() {
        bail!("corpus {} is not a directory", corpus.display());
    }

    let mut files = Vec::new();
    collect_jsonl(corpus, &mut files)?;
    if files.is_empty() {
        bail!("no .jsonl files under {}", corpus.display());
    }

    let mut line_count = 0u64;
    let mut file_count = 0u64;
    for path in &files {
        file_count += 1;
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let reader = BufReader::new(file);
        for (index, line) in reader.lines().enumerate() {
            let line = line.with_context(|| format!("read {}", path.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            parse_rollout_line(&line).with_context(|| {
                format!(
                    "invalid rollout line {} in {}:\n{}",
                    index + 1,
                    path.display(),
                    truncate(&line, 240)
                )
            })?;
            line_count += 1;
        }
    }

    println!(
        "Validated {file_count} jsonl files / {line_count} lines under {}",
        corpus.display()
    );
    Ok(())
}

fn collect_jsonl(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
