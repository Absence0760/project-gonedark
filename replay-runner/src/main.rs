//! Headless replay record/playback driver (roadmap PC-3 — "Replays & spectating, a determinism
//! freebie"). Sibling of `sim-runner` / `net-sim-runner`: it emits the same `<tick> <checksum>`
//! stream to **stdout** (so a replay run is determinism-covered exactly like the others), and a
//! human-readable PASS/FAIL report to **stderr** (which never touches stdout, so it cannot move
//! the checksum).
//!
//! What it does, end to end:
//!   1. **record** a bundled scenario for N ticks, capturing its per-tick command log,
//!   2. **write** the replay to a byte artifact on disk (default: a temp file; `--out <path>`),
//!   3. **read it back**, **play it back** re-feeding only the recorded commands, and
//!   4. **assert** the playback checksum stream is bit-identical to the record run — the freebie.
//!
//! Usage: `gonedark-replay-runner [ticks] [scenario] [--out <path>] [--keep]`
//!   defaults: 300 ticks, `skirmish`. `--out` sets the artifact path; `--keep` leaves it on disk.
//! Exit code is non-zero if the playback stream ever diverges from the record run (a real desync).

use std::process::ExitCode;

use gonedark_replay_runner::{playback, record, Replay, Scenario};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keep = args.iter().any(|a| a == "--keep");

    // `--out <path>` (two tokens). Anything else non-flag is positional.
    let mut out_path: Option<String> = None;
    let mut positional: Vec<&String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--out" {
            if let Some(p) = args.get(i + 1) {
                out_path = Some(p.clone());
                i += 2;
                continue;
            } else {
                eprintln!("--out needs a path");
                return ExitCode::from(2);
            }
        } else if a.starts_with("--") {
            i += 1; // known bare flag (e.g. --keep) or ignored
        } else {
            positional.push(a);
            i += 1;
        }
    }

    let ticks: u64 = positional
        .first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    let scenario = positional
        .get(1)
        .map(|s| s.as_str())
        .map(|s| Scenario::parse(s).unwrap_or_else(|| fatal_scenario(s)))
        .unwrap_or(Scenario::DEFAULT);

    // Canonical seed (matches the lib tests).
    let seed: u64 = 0x60ED_DA47;

    // 1. Record.
    let (record_stream, replay) = record(scenario, seed, ticks);

    // The determinism-covered stream on stdout is the RECORD run.
    for (t, c) in record_stream.iter().enumerate() {
        println!("{t} {c:016x}");
    }

    // 2. Write the artifact to disk (a genuine round-trip through bytes-on-disk, not just memory).
    let path = out_path.unwrap_or_else(|| {
        std::env::temp_dir()
            .join(format!("gonedark-replay-{}-{ticks}.gdr", scenario.token()))
            .to_string_lossy()
            .into_owned()
    });
    let bytes = replay.encode();
    if let Err(e) = std::fs::write(&path, &bytes) {
        eprintln!("failed to write replay artifact {path}: {e}");
        return ExitCode::FAILURE;
    }

    // 3. Read it back + play it back.
    let disk = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read replay artifact {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let decoded = match Replay::decode(&disk) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to decode replay artifact {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let playback_stream = playback(&decoded);

    if !keep {
        let _ = std::fs::remove_file(&path);
    }

    // 4. The proof, to stderr.
    eprintln!(
        "replay: scenario={} seed={seed:#018x} ticks={ticks} commands={} artifact={} bytes",
        scenario.token(),
        decoded.command_count(),
        bytes.len(),
    );
    eprintln!("  artifact path: {path}{}", if keep { " (kept)" } else { " (removed)" });
    eprintln!(
        "  record  final tick {} checksum {:016x}",
        record_stream.len().saturating_sub(1),
        record_stream.last().copied().unwrap_or(0),
    );
    eprintln!(
        "  playback final tick {} checksum {:016x}",
        playback_stream.len().saturating_sub(1),
        playback_stream.last().copied().unwrap_or(0),
    );

    if record_stream == playback_stream {
        eprintln!(
            "  PASS: playback checksum stream is bit-identical to record over {} ticks",
            record_stream.len()
        );
        ExitCode::SUCCESS
    } else {
        // Find the first diverging tick for a useful message.
        let first = record_stream
            .iter()
            .zip(&playback_stream)
            .position(|(a, b)| a != b);
        match first {
            Some(t) => eprintln!(
                "  FAIL: diverged at tick {t}: record {:016x} != playback {:016x}",
                record_stream[t], playback_stream[t]
            ),
            None => eprintln!(
                "  FAIL: streams differ in length: record {} vs playback {}",
                record_stream.len(),
                playback_stream.len()
            ),
        }
        ExitCode::FAILURE
    }
}

fn fatal_scenario(s: &str) -> ! {
    eprintln!("unknown scenario {s:?}; expected `skirmish`");
    std::process::exit(2);
}
