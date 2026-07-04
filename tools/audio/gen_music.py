#!/usr/bin/env python3
"""Generate the designed MUSIC set (the "going-dark" score) — deterministic Csound additive
synthesis + a SoX master pass, the music twin of `tools/audio/gen_sfx.py`.

Script-not-binary (decisions.md D41/D46): this generator + the manifest entry are the committed
source of record; the WAVs are regenerable artifacts. Music is a PRESENTATION system (invariant
#4/#6): it never touches the sim, so it lives entirely on the pal/render side. But it is not
polish — a core pillar is that the world goes dark and you fight by *sound* (invariant #6), and a
silent battlefield reads as broken. This set gives the match an atmospheric bed and its win/lose
punctuation.

## The three cues

* **combat_bed** — a low, tense, LOOPABLE A-minor drone (the going-dark mood). It is the continuous
  bus that plays under a match, installed once via `pal::mix::Mixer::set_music` and cursor-wrapped
  each frame. Because it loops by wrapping the buffer, it MUST be seamless: a discontinuity at the
  wrap would click once per loop. We guarantee seamlessness *by construction* — every partial (and
  every amplitude LFO) is quantized to a WHOLE number of cycles over the buffer length, so the last
  sample steps into the first with no jump. Critically the bed's SoX pass does **no fade and no IIR
  filter** (both would break the loop: a fade zeroes the ends, an IIR filter's startup transient
  makes sample 0 differ from the steady-state tail); it is a pure global normalize, which is linear
  and preserves the seam. All brightness/darkness is shaped *additively* (partial amplitudes), not
  by a filter, for exactly this reason.
* **win_stinger** — a short RISING, resolving figure (A3 → C#4 major lift → E4 → held A4+E5). One-shot
  musical punctuation on a match won; played through the music bus (`Mixer::play_music_oneshot`),
  ducking the bed while it sounds.
* **lose_stinger** — a short FALLING, somber figure (A4 → F4 → E4 → held low A3+C4 minor third). The
  one-shot on a match lost.

Stingers are one-shots (never looped), so their SoX pass DOES fade the tail (de-click) — the loop
constraint that forbids it for the bed does not apply.

## Determinism

The synthesis is purely tonal (Csound `oscili`, no noise opcodes), so it is deterministic with or
without a seed; SoX runs with `-D` (no dither — the one nondeterministic 16-bit stage). Nothing
embeds a timestamp. The script renders every cue TWICE and refuses to write if the bytes differ,
then records sha256 per file in the manifest.

Emits, under assets/audio/music/:
  * combat_bed.wav / win_stinger.wav / lose_stinger.wav — mono 16-bit 24 kHz PCM; pal/src/bank.rs
    include_bytes!s these (hand-rolled RIFF parse — the pal crate stays dependency-free, the same
    discipline as the SFX bank) and resamples to the device rate at stream-open.
  * manifest.json — provenance (source / license / sha256), the auditable record.

The NAMES are the contract with `pal::bank::MUSIC_ASSETS` (and the `MusicId` enum) — keep in sync.

Run: `pnpm assets:music` (or `python3 tools/audio/gen_music.py`). Requires csound + sox on PATH
(both Homebrew formulae — see content-pipeline.md §6).
"""

import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SAMPLE_RATE = 24_000  # authored low; the bank resamples up to the device rate. Keeps the loop small.
OUT_DIR = Path(__file__).resolve().parents[2] / "assets" / "audio" / "music"

# f1 = a sine wave (partials); f2 = a sine wave (the slow amplitude LFO).
ORC = f"""\
sr = {SAMPLE_RATE}
ksmps = 1
nchnls = 1
0dbfs = 1

; A summed partial with a slow, whole-cycle amplitude swell:
;   p4 = frequency (Hz), p5 = base amplitude, p6 = LFO Hz, p7 = LFO depth (fraction of amp)
instr 1
  klfo oscili p7, p6, 2
  kamp = p5 * (1 + klfo)
  a1   oscili kamp, p4, 1
  out a1
endin

; A plucked tonal note for the stingers: fast attack, exponential tail, + an octave harmonic.
;   p4 = frequency (Hz), p5 = amplitude
instr 2
  aenv expseg 0.001, 0.01, p5, p3*0.35, p5*0.55, p3*0.64, 0.001
  a1   oscili aenv, p4, 1
  a2   oscili aenv*0.3, p4*2
  out a1 + a2
endin
"""

FTABLES = "f1 0 8192 10 1\nf2 0 8192 10 1\n"


def q(hz: float, dur: float) -> float:
    """Quantize a frequency to the nearest whole number of cycles over `dur` seconds, so a partial
    (or LFO) rendered for exactly `dur` closes its loop at the wrap. `round(hz*dur)/dur`."""
    cycles = max(1, round(hz * dur))
    return cycles / dur


# ---- combat_bed: a tense A-minor drone, every freq quantized to a whole loop ---------------------
BED_DUR = 8.0
# (target Hz, amp, LFO Hz, LFO depth). A1 root + a slightly detuned root (a slow ~0.5 Hz beat =
# tension), the fifth, the octave, the MINOR third (the dark colour), plus faint upper partials for
# a touch of air. All shaped additively — no filter — so the seam survives (see module docstring).
BED_PARTIALS = [
    (55.00, 0.50, 0.125, 0.20),  # A1  root, slow 8-second swell
    (55.50, 0.30, 0.000, 0.00),  # detuned root -> ~0.5 Hz beat (tension)
    (82.41, 0.26, 0.250, 0.25),  # E2  fifth
    (110.00, 0.20, 0.375, 0.20),  # A2  octave
    (130.81, 0.18, 0.250, 0.30),  # C3  MINOR third (the minor colour)
    (164.81, 0.10, 0.500, 0.35),  # E3
    (329.63, 0.05, 0.750, 0.40),  # E4  faint shimmer / air
]

# ---- win_stinger: rising, resolving, a MAJOR lift on the way up ---------------------------------
WIN_DUR = 1.65
WIN_NOTES = [
    (0.00, 0.90, 0.90, 220.00),   # A3
    (0.15, 0.90, 0.90, 277.18),   # C#4 (major-third lift = triumphant)
    (0.30, 0.90, 0.90, 329.63),   # E4
    (0.45, 1.15, 1.15, 440.00),   # A4 (held)
    (0.45, 1.15, 0.70, 659.26),   # E5 fifth on top -> a resolved open chord
]

# ---- lose_stinger: falling, somber, resolving down into a low minor third -----------------------
LOSE_DUR = 2.20
LOSE_NOTES = [
    (0.00, 0.70, 0.75, 440.00),   # A4
    (0.30, 0.70, 0.75, 349.23),   # F4 (dark)
    (0.60, 0.70, 0.75, 329.63),   # E4
    (0.90, 1.25, 0.62, 220.00),   # A3 low resolve (held)
    (0.90, 1.25, 0.42, 261.63),   # C4 minor third held under it
]


def bed_score() -> str:
    lines = [FTABLES]
    for hz, amp, lfo_hz, depth in BED_PARTIALS:
        fq = q(hz, BED_DUR)
        lf = q(lfo_hz, BED_DUR) if lfo_hz > 0 else 0.0
        lines.append(f"i1 0 {BED_DUR} {fq:.6f} {amp} {lf:.6f} {depth}")
    return "\n".join(lines) + "\n"


def note_score(notes: list) -> str:
    lines = [FTABLES]
    for start, dur, amp, hz in notes:
        lines.append(f"i2 {start} {dur} {hz} {amp}")
    return "\n".join(lines) + "\n"


# name -> (score, total duration, SoX tail chain). The bed's chain is a pure normalize (loop-safe);
# the stingers fade their tail (one-shots, so a fade is fine). `-D` disables dither everywhere.
SOUNDS = [
    {
        "name": "combat_bed",
        "music_id": "CombatBed",
        "role": "the looping ambient combat bed — the going-dark mood bus under a match",
        "design": "tense A-minor drone (root+detuned-beat+fifth+octave+minor-third), whole-cycle "
        "partials for a seamless loop, normalized quiet to sit under the SFX mix",
        "score": bed_score(),
        # No fade, no IIR filter: a global normalize only, so the loop wrap stays seamless.
        "sox_tail": ["gain", "-n", "-10"],
    },
    {
        "name": "win_stinger",
        "music_id": "WinStinger",
        "role": "the match-won musical punctuation (one-shot on the music bus)",
        "design": "rising A3->C#4(major lift)->E4->held A4+E5 open chord — triumphant, resolved",
        "score": note_score(WIN_NOTES),
        "sox_tail": ["gain", "-n", "-5", "fade", "t", "0.005", "-0", "0.06"],
    },
    {
        "name": "lose_stinger",
        "music_id": "LoseStinger",
        "role": "the match-lost musical punctuation (one-shot on the music bus)",
        "design": "falling A4->F4->E4->held low A3+C4 minor third — somber, sinking",
        "score": note_score(LOSE_NOTES),
        "sox_tail": ["gain", "-n", "-5", "fade", "t", "0.005", "-0", "0.12"],
    },
]


def render_cue(sound: dict, out_wav: Path, work: Path) -> None:
    """Render one cue: Csound (float WAV) -> a per-cue SoX tail (normalize; fade for stingers)."""
    csd = work / f"{sound['name']}.csd"
    csd.write_text(
        "<CsoundSynthesizer>\n<CsInstruments>\n"
        + ORC
        + "</CsInstruments>\n<CsScore>\n"
        + sound["score"]
        + "</CsScore>\n</CsoundSynthesizer>\n"
    )
    raw = work / f"{sound['name']}.raw.wav"
    subprocess.run(
        ["csound", "-d", "-m0", "-W", "-f", "-o", str(raw), str(csd)],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["sox", "-D", str(raw), "-b", "16", "-c", "1", str(out_wav), *sound["sox_tail"]],
        check=True,
        capture_output=True,
    )


def wav_duration_s(path: Path) -> float:
    out = subprocess.run(
        ["soxi", "-D", str(path)], check=True, capture_output=True, text=True
    ).stdout.strip()
    return float(out)


def main() -> int:
    for tool in ("csound", "sox", "soxi"):
        if shutil.which(tool) is None:
            print(f"{tool} not found on PATH (brew install sox csound)", file=sys.stderr)
            return 1
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    entries = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for sound in SOUNDS:
            # Render twice; a designed cue that isn't byte-reproducible is a pipeline bug — refuse
            # to ship it (the SFX generator's discipline).
            wav_a = work / f"{sound['name']}.a.wav"
            wav_b = work / f"{sound['name']}.b.wav"
            render_cue(sound, wav_a, work)
            render_cue(sound, wav_b, work)
            data = wav_a.read_bytes()
            if data != wav_b.read_bytes():
                print(f"{sound['name']}: nondeterministic render", file=sys.stderr)
                return 1

            out_wav = OUT_DIR / f"{sound['name']}.wav"
            out_wav.write_bytes(data)
            entries.append(
                {
                    "name": sound["name"],
                    "file": f"{sound['name']}.wav",
                    "music_id": sound["music_id"],
                    "role": sound["role"],
                    "design": sound["design"],
                    "duration_s": round(wav_duration_s(out_wav), 4),
                    "sample_rate": SAMPLE_RATE,
                    "channels": 1,
                    "bits": 16,
                    "wav_bytes": len(data),
                    "wav_sha256": hashlib.sha256(data).hexdigest(),
                }
            )

    manifest = {
        "note": (
            "Designed MUSIC set (the going-dark score), generated by tools/audio/gen_music.py "
            "(decisions.md D41/D46). Mono 16-bit 24 kHz PCM WAVs; pal/src/bank.rs include_bytes!s "
            "each and resamples to the device rate at stream-open (names are the contract with "
            "pal::bank::MUSIC_ASSETS / the MusicId enum). combat_bed is the looping bus under a "
            "match; win_stinger / lose_stinger are one-shot match-end punctuation on the music bus. "
            "Presentation-only — never the sim (invariant #1/#4). Regenerate with `pnpm assets:music`."
        ),
        "identity": (
            "one A-minor tuning shared with the SFX set: a low tense drone bed, a rising major-lift "
            "win, a falling minor lose. The bed loops seamlessly by whole-cycle construction (no "
            "fade / no IIR filter in its master pass)."
        ),
        "source": "Csound 6 (offline additive synthesis, no noise -> deterministic) + SoX -D (no dither)",
        "license": "CC0-1.0",
        "author": "procedurally synthesised via Csound + SoX",
        "sounds": entries,
    }
    (OUT_DIR / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

    for e in entries:
        print(f"{e['file']:18} {e['duration_s']:6.3f}s  {e['wav_bytes']:7d} B  {e['wav_sha256'][:16]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
