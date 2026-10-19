//! Timing probe for large-rollout resume cost (#[ignore]; run explicitly).
//!
//! Measures the phases of a cold resume against a real journal so the
//! write-amplification fix can be targeted. Not part of the regular suite.

#[cfg(test)]
mod tests {
    use std::time::Instant;

    fn probe_path() -> Option<std::path::PathBuf> {
        std::env::var("DEVO_RESUME_PERF_PROBE")
            .ok()
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
    }

    /// DEVO_RESUME_PERF_PROBE=<rollout.jsonl> DEVO_RESUME_PERF_TURN=<turn_id> \
    ///   cargo test -p devo-core --lib resume_perf -- --ignored --nocapture
    #[test]
    #[ignore]
    fn resume_perf_probe() {
        let Some(path) = probe_path() else {
            return;
        };
        let turn_id = std::env::var("DEVO_RESUME_PERF_TURN").unwrap_or_default();

        let t0 = Instant::now();
        let text = std::fs::read_to_string(&path).expect("read rollout");
        println!("[probe] read_to_string: {:.1?}", t0.elapsed());

        let t1 = Instant::now();
        let lines: Vec<&str> = text.lines().collect();
        println!(
            "[probe] split lines ({}): {:.1?}",
            lines.len(),
            t1.elapsed()
        );

        let t2 = Instant::now();
        let mut parsed = 0usize;
        let mut skipped = 0usize;
        for line in &lines {
            if line.trim().is_empty() {
                continue;
            }
            match crate::parse_rollout_line(line) {
                Ok(_) => parsed += 1,
                Err(_) => skipped += 1,
            }
        }
        println!(
            "[probe] parse_rollout_line all: parsed={parsed} err={skipped} {:.1?}",
            t2.elapsed()
        );

        let t3 = Instant::now();
        let history = crate::read_canonical_history(&path).expect("canonical history");
        println!(
            "[probe] read_canonical_history: items={} turns={} {:.1?}",
            history.items.len(),
            history.turns.len(),
            t3.elapsed()
        );

        if !turn_id.is_empty() {
            let tid = devo_protocol::native::ids::TurnId::from_string(turn_id);
            let t4 = Instant::now();
            let replay = crate::durable_execution::read_execution_replay(&path, &tid)
                .expect("execution replay");
            println!(
                "[probe] read_execution_replay: items={} has_checkpoint={} {:.1?}",
                replay.items.len(),
                replay.has_checkpoint,
                t4.elapsed()
            );
        }
    }
}
