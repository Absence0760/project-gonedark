#!/usr/bin/env python3
"""Generate the designed SFX set (CP-6 audio identity) — Csound synthesis + a shared SoX chain.

Script-not-binary (decisions.md D41/D46): this generator + the manifest entry are the committed
source of record; the WAVs are regenerable artifacts. Audio is LOAD-BEARING here, not polish —
while embodied, the going-dark alert channel is directional flash + AUDIO (invariant #6), and the
four alert-ping sounds (engine::alert_cues) reuse the Gunfire / UnitDown / BaseHit / Capture
world cues, so those four must stay unmistakably distinct BY EAR and pan well.

## The sound identity ("night-ops radio")

One coherent palette instead of N isolated bleeps:

* **One tuning.** Every tonal cue sits in A minor (A4 = 440 Hz): losses FALL through the triad
  (UnitDown: E5-C5-A4), gains RISE through it (Capture: A4-E5; HitConfirm: E6-A6 — a rising
  fourth, "confirmed"), and neutral confirmations sit on the root (ProductionReady: A5+A6).
* **Two timbre families.** *World* sounds are noise-transient based (Gunfire, WeaponFire,
  Impact, BaseHit — cracks, thuds, struck metal); *signal* sounds are clean tones (UnitDown,
  Capture, ProductionReady, HitConfirm — the command-layer/feedback voice).
* **One processing chain.** Every cue passes the same SoX master chain (rumble highpass, a
  gentle 2.5 kHz presence lift, de-click fades, normalize to -2.9 dBFS ≈ 0.72 peak — inside the
  mixer's [-0.8, 0.8] stacking headroom). The shared chain is what makes the set read as one
  game.
* **Directional-friendly by construction.** The desktop/Android mixers pan by equal-power ILD
  only (pal::mix::voice_from_cue), and pure low frequencies don't lateralize — so every cue,
  including the 62 Hz BaseHit boom, carries deliberate energy above ~1.5 kHz (noise bands,
  metallic partials, 4th-harmonic sparkle). That plus the presence lift is what makes the four
  alert pings readable as left/right while the map is dark (invariant #6).

## Alert-palette separability (the invariant-#6 contract)

| AlertKind (engine::alert_cues) | SoundId | reads as |
|---|---|---|
| TakingFire       | Gunfire  | band-limited rifle crack (noise family) |
| UnitLost         | UnitDown | FALLING three-note minor motif (tonal, dark) |
| BaseUnderAttack  | BaseHit  | low boom + struck-metal clang (percussive-metallic) |
| TerritoryLost    | Capture  | RISING two-note chime (tonal, bright) |

Four different *classes* of sound (noise / falling tone / metallic hit / rising tone), not four
variations of one — separable by ear even panned hard, quiet, or stacked.

## Determinism

Csound renders offline with a fixed `seed` (its noise opcodes are seed-scripted PRNGs), SoX runs
with `-D` (no dither — the one nondeterministic stage in a 16-bit conversion), and nothing embeds
a timestamp. The script renders every cue TWICE and refuses to write if the bytes differ, then
records sha256 per file in the manifest.

Emits, per cue:
  * assets/audio/<name>.wav   — mono 16-bit 48 kHz PCM; pal/src/bank.rs include_bytes!s these
                                (hand-rolled RIFF parse — the pal crate stays dependency-free,
                                the same discipline as render's .gray font atlas) and resamples
                                to the device rate at stream-open
  * assets/audio/manifest.json — provenance (source / license / sha256), the auditable record

The NAMES + count are the contract with `pal::bank::ASSETS` — keep them in sync.

Run: `pnpm assets:sfx` (or `python3 tools/audio/gen_sfx.py`). Requires csound + sox on PATH
(both Homebrew formulae — see content-pipeline.md §6).
"""

import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SAMPLE_RATE = 48_000
SEED = 271828  # fixed Csound PRNG seed — regeneration is byte-identical
OUT_DIR = Path(__file__).resolve().parents[2] / "assets" / "audio"

# The shared SoX master chain — the single processing "voice" of the set (see module docstring).
# -D disables dither so the 16-bit conversion is deterministic.
SOX_CHAIN = [
    "highpass", "45",                     # rumble control under the boom fundamentals
    "equalizer", "2500", "1.2q", "+2",    # presence lift: the ILD-pan localization band
    "fade", "t", "0.002", "-0", "0.012",  # de-click both ends
    "gain", "-n", "-2.9",                 # normalize to -2.9 dBFS ~= 0.72 peak (mixer headroom)
]

ORC_HEADER = f"""\
sr = {SAMPLE_RATE}
ksmps = 1
nchnls = 1
0dbfs = 1
seed {SEED}
"""

# ---- The cue set --------------------------------------------------------------------------------
# name        -> must match pal::bank::ASSETS + the SoundId doc row in the manifest
# orchestra   -> Csound instruments (header prepended)
# score       -> Csound score events
# Frequencies are A-minor pitches (A4 = 440): A3 220, E4 329.63, A4 440, C5 523.25, E5 659.26,
# A5 880, E6 1318.5, A6 1760.
SOUNDS = [
    {
        "name": "gunfire",
        "sound_id": "Gunfire",
        "role": "field weapons fire (world mix) + the TakingFire alert ping",
        "design": "distant band-limited rifle crack: 1.7 kHz noise band over a low body, fast decay",
        "orchestra": """\
instr 1
  anoise rand 0.9
  aband  butterbp anoise, 1700, 1600
  abody  butterlp anoise, 320
  aenv   expseg 1, 0.018, 0.28, 0.14, 0.0004
  out (aband*0.85 + abody*0.55) * aenv
endin
""",
        "score": "i1 0 0.16\n",
    },
    {
        "name": "weapon_fire",
        "sound_id": "WeaponFire",
        "role": "the embodied avatar's OWN gun crack, host-clock on the trigger press (WS-A)",
        "design": "close hot crack: full crack + 2.6 kHz snap over a 108 Hz body thump — bigger and nearer than gunfire",
        "orchestra": """\
instr 1
  anoise rand 0.9
  acrack butterhp anoise, 900
  asnap  butterbp anoise, 2600, 1800
  aenvc  expseg 1, 0.012, 0.3, 0.17, 0.0003
  abody  oscili 1, 108
  aenvb  expseg 0.9, 0.05, 0.25, 0.11, 0.0005
  out acrack*0.6*aenvc + asnap*0.55*aenvc + abody*0.55*aenvb
endin
""",
        "score": "i1 0 0.2\n",
    },
    {
        "name": "impact",
        "sound_id": "Impact",
        "role": "the avatar's shot landing — coupled to the impact VFX (WS-A)",
        "design": "very short strike: bright noise tick over a 235 Hz knock, faster decay than any crack",
        "orchestra": """\
instr 1
  anoise rand 0.9
  atick  butterhp anoise, 2500
  aenvt  expseg 1, 0.008, 0.2, 0.055, 0.0005
  aknock oscili 1, 235
  aenvk  expseg 1, 0.03, 0.15, 0.032, 0.001
  out atick*0.7*aenvt + aknock*0.5*aenvk
endin
""",
        "score": "i1 0 0.08\n",
    },
    {
        "name": "hit_confirm",
        "sound_id": "HitConfirm",
        "role": "the embodied hitmarker tick (feedback on the player's own action)",
        "design": "two rising blips E6->A6 (a rising fourth: confirmed) — the shortest, highest cue; pure UI",
        "orchestra": """\
instr 1
  aenv expseg 0.9, p3*0.25, 0.5, p3*0.75, 0.001
  a1   oscili aenv, p4, 1
  out a1
endin
""",
        "score": """\
f1 0 8192 10 1 0.28 0.12
i1 0    0.035 1318.5
i1 0.05 0.045 1760
""",
    },
    {
        "name": "unit_down",
        "sound_id": "UnitDown",
        "role": "one of your units died (world mix) + the UnitLost alert ping",
        "design": "FALLING three-note A-minor motif E5-C5-A4, dark tone + 4th-harmonic sparkle for pan localization",
        "orchestra": """\
instr 1
  aenv expseg 0.8, p3*0.85, 0.12, p3*0.15, 0.001
  a1   oscili aenv, p4, 1
  a2   oscili aenv*0.12, p4*4
  out a1 + a2
endin
""",
        "score": """\
f1 0 8192 10 1 0.4 0.2 0.08
i1 0    0.13 659.26
i1 0.12 0.13 523.25
i1 0.24 0.16 440
""",
    },
    {
        "name": "base_hit",
        "sound_id": "BaseHit",
        "role": "a building of yours being hit (world mix) + the BaseUnderAttack alert ping",
        "design": "low pitched-down boom (130->55 Hz) under struck-metal inharmonic partials (742/1178/1876/2814 Hz) — the clang localizes, the boom carries weight",
        "orchestra": """\
instr 1
  kpitch expseg 130, 0.09, 62, 0.2, 55
  aboom  oscili 1, kpitch
  aenvb  expseg 1, 0.28, 0.002
  am1    oscili 1, 742
  am2    oscili 0.7, 1178
  am3    oscili 0.8, 1876
  am4    oscili 0.6, 2814
  aenvm  expseg 0.8, 0.014, 0.35, 0.14, 0.001
  anoise rand 0.8
  aclank butterhp anoise, 2000
  aenvn  expseg 0.9, 0.025, 0.001
  out aboom*0.85*aenvb + (am1+am2+am3+am4)*0.3*aenvm + aclank*0.6*aenvn
endin
""",
        "score": "i1 0 0.3\n",
    },
    {
        "name": "capture",
        "sound_id": "Capture",
        "role": "a control point changed hands (world mix) + the TerritoryLost alert ping",
        "design": "RISING two-note chime A4->E5 with a slight inharmonic shimmer — the contour opposite of unit_down",
        "orchestra": """\
instr 1
  aenv expseg 0.7, p3*0.3, 0.4, p3*0.7, 0.002
  a1   oscili aenv, p4, 1
  a2   oscili aenv*0.18, p4*3.01
  out a1 + a2
endin
""",
        "score": """\
f1 0 8192 10 1 0.25 0.1
i1 0    0.11 440
i1 0.10 0.18 659.26
""",
    },
    {
        "name": "production_ready",
        "sound_id": "ProductionReady",
        "role": "a queued unit finished production (command-layer confirmation)",
        "design": "single clean root blip A5 with an A6 octave sparkle — neutral 'done', neither rising nor falling",
        "orchestra": """\
instr 1
  aenv expseg 0.8, 0.012, 0.5, 0.095, 0.001
  a1   oscili aenv, 880, 1
  a2   oscili aenv*0.22, 1760
  out a1 + a2
endin
""",
        "score": "f1 0 8192 10 1 0.2\ni1 0 0.11\n",
    },
]


def render_cue(sound: dict, out_wav: Path, work: Path) -> None:
    """Render one cue: Csound (float WAV, no clipping headroom worries) -> the shared SoX chain."""
    csd = work / f"{sound['name']}.csd"
    csd.write_text(
        "<CsoundSynthesizer>\n<CsInstruments>\n"
        + ORC_HEADER
        + sound["orchestra"]
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
        ["sox", "-D", str(raw), "-b", "16", "-c", "1", str(out_wav), *SOX_CHAIN],
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
            # Render twice; a designed cue that isn't byte-reproducible is a pipeline bug
            # (an unseeded opcode, dither, a timestamp) — refuse to ship it.
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
                    "sound_id": sound["sound_id"],
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
            "Designed SFX set (roadmap CP-6 audio identity), generated by "
            "tools/audio/gen_sfx.py (decisions.md D41/D46). Mono 16-bit 48 kHz PCM WAVs; "
            "pal/src/bank.rs include_bytes!s each and resamples to the device rate at "
            "stream-open (sound names are the contract with pal::bank::ASSETS). The four "
            "alert-ping cues (gunfire / unit_down / base_hit / capture) carry the going-dark "
            "alert channel (invariant #6) and must stay separable by ear. Presentation-only; "
            "regenerate with `pnpm assets:sfx`."
        ),
        "identity": (
            "night-ops radio: one A-minor tuning (losses fall, gains rise, confirmations sit "
            "on the root), two timbre families (noise-transient world sounds, clean-tone "
            "signal sounds), one shared SoX master chain, and deliberate >1.5 kHz content in "
            "every cue so equal-power ILD panning localizes"
        ),
        "source": "Csound 6 (seed-scripted offline synthesis) + SoX -D (shared master chain, no dither)",
        "license": "CC0-1.0",
        "author": "procedurally synthesised (seed-scripted) via Csound + SoX",
        "seed": SEED,
        "sounds": entries,
    }
    (OUT_DIR / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

    for e in entries:
        print(f"{e['file']:22} {e['duration_s']:6.3f}s  {e['wav_bytes']:6d} B  {e['wav_sha256'][:16]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
