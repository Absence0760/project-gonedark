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
//! Usage: `gonedark-replay-runner [ticks] [scenario] [--multi] [--out <path>] [--keep]`
//!   defaults: 300 ticks, `skirmish`. `--out` sets the artifact path; `--keep` leaves it on disk.
//!   `--multi` records/plays the **multi-peer** (lockstep PvP) form: a per-tick, per-peer command
//!   log merged in ascending peer order — the same rule `core::lockstep` uses — proving a recorded
//!   multi-peer match replays bit-identically regardless of the order peers' inputs arrived.
//! Exit code is non-zero if the playback stream ever diverges from the record run (a real desync).

use std::process::ExitCode;

use gonedark_replay_runner::{
    playback, playback_multi, record, record_multi, MultiReplay, Replay, ReplayError, Scenario,
    CANONICAL_SEED,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keep = args.iter().any(|a| a == "--keep");
    let multi = args.iter().any(|a| a == "--multi");

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

    let ticks: u64 = parse_ticks(positional.first().map(|s| s.as_str()))
        .unwrap_or_else(|bad| fatal_ticks(&bad));
    let scenario = positional
        .get(1)
        .map(|s| s.as_str())
        .map(|s| Scenario::parse(s).unwrap_or_else(|| fatal_scenario(s)))
        .unwrap_or(Scenario::DEFAULT);

    let seed: u64 = CANONICAL_SEED;

    if multi {
        // 1. Record the multi-peer per-peer log.
        let (record_stream, replay) = record_multi(scenario, seed, ticks);
        let labels = RunLabels {
            error_label: "multi-peer replay",
            header_prefix: "replay(multi-peer)",
            pass_extra: "multi-peer ",
            artifact_suffix: "-multi",
        };
        return run_replay_flow(
            labels,
            scenario,
            seed,
            ticks,
            out_path,
            keep,
            record_stream,
            replay,
            MultiReplay::encode,
            MultiReplay::decode,
            playback_multi,
            |r| format!(" peers={} commands={}", r.peer_count(), r.command_count()),
        );
    }

    // 1. Record.
    let (record_stream, replay) = record(scenario, seed, ticks);
    let labels = RunLabels {
        error_label: "replay",
        header_prefix: "replay",
        pass_extra: "",
        artifact_suffix: "",
    };
    run_replay_flow(
        labels,
        scenario,
        seed,
        ticks,
        out_path,
        keep,
        record_stream,
        replay,
        Replay::encode,
        Replay::decode,
        playback,
        |r| format!(" commands={}", r.command_count()),
    )
}

/// The per-flow wording that differs between the single-peer and multi-peer paths — everything
/// else (the write→read→decode→playback→PASS/FAIL skeleton) is shared verbatim by
/// [`run_replay_flow`], so the two flows can never again silently drift apart on that shape.
struct RunLabels {
    /// Used in "failed to {write,read,decode} `{error_label}` artifact …" (e.g. `"replay"` /
    /// `"multi-peer replay"`).
    error_label: &'static str,
    /// The report header's leading token (e.g. `"replay"` / `"replay(multi-peer)"`).
    header_prefix: &'static str,
    /// Prefixed onto the PASS line (e.g. `""` / `"multi-peer "`).
    pass_extra: &'static str,
    /// Appended to the artifact filename before `-{ticks}.gdr` (e.g. `""` / `"-multi"`).
    artifact_suffix: &'static str,
}

/// Shared record → write-to-disk → read-back → decode → playback → PASS/FAIL-report skeleton for
/// both the single-peer (`main`) and multi-peer (`--multi`) replay flows. Generic over the
/// artifact type `T` via caller-supplied `encode`/`decode`/`playback`/`extra_header` — every
/// printed word besides the [`RunLabels`] substitutions is identical between the two call sites,
/// preserving the exact stdout/stderr contract other tooling may grep.
#[allow(clippy::too_many_arguments)]
fn run_replay_flow<T>(
    labels: RunLabels,
    scenario: Scenario,
    seed: u64,
    ticks: u64,
    out_path: Option<String>,
    keep: bool,
    record_stream: Vec<u64>,
    replay: T,
    encode: fn(&T) -> Vec<u8>,
    decode: fn(&[u8]) -> Result<T, ReplayError>,
    playback: fn(&T) -> Vec<u64>,
    extra_header: fn(&T) -> String,
) -> ExitCode {
    // The determinism-covered stream on stdout is the RECORD run.
    for (t, c) in record_stream.iter().enumerate() {
        println!("{t} {c:016x}");
    }

    // 2. Write the artifact to disk (a genuine round-trip through bytes-on-disk, not just memory).
    let path = out_path.unwrap_or_else(|| {
        std::env::temp_dir()
            .join(format!(
                "gonedark-replay-{}{}-{ticks}.gdr",
                scenario.token(),
                labels.artifact_suffix
            ))
            .to_string_lossy()
            .into_owned()
    });
    let bytes = encode(&replay);
    if let Err(e) = std::fs::write(&path, &bytes) {
        eprintln!("failed to write {} artifact {path}: {e}", labels.error_label);
        return ExitCode::FAILURE;
    }

    // 3. Read it back + play it back.
    let disk = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read {} artifact {path}: {e}", labels.error_label);
            return ExitCode::FAILURE;
        }
    };
    let decoded = match decode(&disk) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to decode {} artifact {path}: {e}", labels.error_label);
            return ExitCode::FAILURE;
        }
    };
    let playback_stream = playback(&decoded);

    if !keep {
        let _ = std::fs::remove_file(&path);
    }

    // 4. The proof, to stderr.
    eprintln!(
        "{}: scenario={} seed={seed:#018x} ticks={ticks}{} artifact={} bytes",
        labels.header_prefix,
        scenario.token(),
        extra_header(&decoded),
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
            "  PASS: {}playback checksum stream is bit-identical to record over {} ticks",
            labels.pass_extra,
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

/// Parse the `ticks` CLI arg: absent falls back to the default (300); *present but unparseable*
/// (e.g. a mistyped CI arg) must fail loudly instead of silently taking the default — a garbled
/// arg silently defaulting would report a spuriously "passing" but wrongly-shaped replay run.
/// Pure — the testable seam behind `main`'s `fatal_ticks` exit.
fn parse_ticks(arg: Option<&str>) -> Result<u64, String> {
    match arg {
        None => Ok(300),
        Some(s) => s.parse::<u64>().map_err(|_| s.to_string()),
    }
}

fn fatal_ticks(s: &str) -> ! {
    eprintln!("invalid tick count {s:?}; expected a non-negative integer");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ticks_absent_defaults_to_300() {
        assert_eq!(parse_ticks(None), Ok(300));
    }

    #[test]
    fn parse_ticks_present_and_valid_parses() {
        assert_eq!(parse_ticks(Some("150")), Ok(150));
        assert_eq!(parse_ticks(Some("0")), Ok(0));
    }

    #[test]
    fn parse_ticks_present_and_malformed_errors_loudly_instead_of_defaulting() {
        // A garbled arg must be reported, not silently treated as absent (the L1 bug).
        assert_eq!(parse_ticks(Some("abc")), Err("abc".to_string()));
        assert_eq!(parse_ticks(Some("")), Err(String::new()));
        assert_eq!(parse_ticks(Some("-5")), Err("-5".to_string())); // u64: no negatives
    }
}
