//! Time Native list/resume/items RPCs against a generated corpus via stdio.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Parser)]
pub struct BenchArgs {
    /// Corpus root previously written by `devo-stress generate`.
    #[arg(long)]
    pub corpus: PathBuf,
    /// Path to the `devo` binary (defaults to `devo` on PATH).
    #[arg(long, default_value = "devo")]
    pub devo_bin: PathBuf,
    /// Page size for session/list and session/items/list probes.
    #[arg(long, default_value_t = 50)]
    pub page_size: u32,
    /// Repeat each timed RPC this many times (reports min/avg/max ms).
    #[arg(long, default_value_t = 3)]
    pub samples: u32,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    hot_root_id: String,
    roots: u32,
}

pub fn run(args: BenchArgs) -> Result<()> {
    let manifest_path = args.corpus.join("stress-manifest.json");
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?,
    )
    .context("parse stress-manifest.json")?;

    let mut child = Command::new(&args.devo_bin)
        .args(["server", "--transport", "stdio"])
        .env("DEVO_HOME", &args.corpus)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn {}", args.devo_bin.display()))?;

    let mut stdin = child.stdin.take().context("server stdin")?;
    let stdout = child.stdout.take().context("server stdout")?;
    let mut reader = BufReader::new(stdout);
    let mut next_id = 1u64;

    // Give indexer a moment; still measure cold session/list as first probe.
    std::thread::sleep(Duration::from_millis(500));

    let init_id = next_id;
    next_id += 1;
    let init = rpc(
        &mut stdin,
        &mut reader,
        init_id,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": {
                "name": "devo-stress",
                "title": "Devo Stress Bench",
                "version": "0.1.0"
            },
            "_meta": { "devo": { "protocol": "native" } }
        }),
    )?;
    if init.get("error").is_some() {
        bail!("initialize error: {init}");
    }

    // Index refresh is async at server start; wait until roots are visible and
    // the hot root is resumable (partial index can make list non-empty early).
    let index_wait_deadline = Instant::now() + Duration::from_secs(180);
    let expected_roots = manifest.roots.max(1);
    loop {
        let list_id = next_id;
        next_id += 1;
        let listed = rpc(
            &mut stdin,
            &mut reader,
            list_id,
            "session/list",
            json!({ "cwds": [], "limit": expected_roots }),
        )?;
        if listed.get("error").is_some() {
            bail!("session/list (index wait) error: {listed}");
        }
        let count = listed
            .pointer("/result/data")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0) as u32;

        let resume_id = next_id;
        next_id += 1;
        let resumed = rpc(
            &mut stdin,
            &mut reader,
            resume_id,
            "session/resume",
            json!({ "sessionId": manifest.hot_root_id }),
        )?;
        let hot_ready = resumed.get("error").is_none();

        if count >= expected_roots.min(50) && hot_ready {
            break;
        }
        if Instant::now() > index_wait_deadline {
            bail!(
                "timed out waiting for session index refresh (listed={count}, expected>={}, hot_ready={hot_ready})",
                expected_roots.min(50)
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let probes = [
        (
            "session/list",
            json!({
                "cwds": [],
                "limit": args.page_size
            }),
        ),
        (
            "session/list",
            json!({
                "cwds": [],
                "limit": args.page_size,
                "includeChildren": true
            }),
        ),
        (
            "session/resume",
            json!({ "sessionId": manifest.hot_root_id }),
        ),
        (
            "session/items/list",
            json!({
                "sessionId": manifest.hot_root_id,
                "limit": args.page_size
            }),
        ),
        ("agent/list", json!({ "sessionId": manifest.hot_root_id })),
    ];

    println!("Bench against corpus {}", args.corpus.display());
    println!("hot_root={}", manifest.hot_root_id);

    for (method, params) in probes {
        let label = if method == "session/list" && params.get("includeChildren").is_some() {
            "session/list(includeChildren)"
        } else {
            method
        };
        let mut samples_ms = Vec::new();
        for _ in 0..args.samples.max(1) {
            let id = next_id;
            next_id += 1;
            let started = Instant::now();
            let response = rpc(&mut stdin, &mut reader, id, method, params.clone())?;
            let elapsed = started.elapsed();
            if response.get("error").is_some() {
                bail!("{label} error: {}", response);
            }
            samples_ms.push(elapsed.as_secs_f64() * 1000.0);
        }
        let min = samples_ms.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = samples_ms.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let avg = samples_ms.iter().sum::<f64>() / samples_ms.len() as f64;
        println!(
            "{label}: min={min:.1}ms avg={avg:.1}ms max={max:.1}ms n={}",
            samples_ms.len()
        );
    }

    // Clean shutdown: close stdin so primary stdio ends.
    drop(stdin);
    let _ = child.wait();
    Ok(())
}

fn rpc(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    writeln!(stdin, "{request}").context("write rpc")?;
    stdin.flush().context("flush rpc")?;

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if Instant::now() > deadline {
            bail!("timed out waiting for response id={id} method={method}");
        }
        let mut line = String::new();
        let n = reader.read_line(&mut line).context("read server stdout")?;
        if n == 0 {
            bail!("server closed stdout while waiting for id={id}");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.get("id") == Some(&json!(id)) {
            return Ok(value);
        }
        // Skip notifications / unrelated responses.
    }
}
