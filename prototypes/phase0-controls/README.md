# Phase 0 — control prototype *(THROWAWAY)*

A disposable **Godot 4.6** build whose only job is to answer the Phase 0 question
from [`../../docs/roadmap.md`](../../docs/roadmap.md):

> Does the **embody ↔ command** loop feel good in hand on a touchscreen?

This is **not** the engine. The real engine is Rust / `wgpu` (decision **D10**), built
fresh in Phase 1. **Delete this whole directory once Phase 0 is settled** — do not grow
it into the game. It deliberately models *feel only*: no fixed-point sim, no ECS, no
netcode, none of the load-bearing invariants. (Those start in Phase 1.)

## What it does (faithful to the locked design, minimal as possible)

- **One unit** on a small field — the entire Phase 0 scope.
- **Command layer** (top-down): tap to move (the unit is a *literal executor* — it just
  walks straight there, design §8), drag to pan, pinch to zoom.
- **Embody**: one tap swaps **the same entity's** input from orders to live player
  control and flips vision to avatar-only — an input-source swap + a vision toggle, *not*
  a character system (invariant #5, D6/D7). Surfacing swaps back; the unit stays where
  you walked it.
- **Going dark** (design §6): while embodied the strategic map is simply *gone*; a
  constant vignette + "● BLIND" tell sells the cost; you get **alerts, not intel** — a
  periodic directional banner + haptic buzz ("taking fire on EAST camp"), never a map
  reveal.
- **Embodied controls**: left thumb = virtual stick (move), right drag = look, FIRE
  button (hitscan within an aim cone). Tune-by-feel constants live at the top of
  `Main.gd`.

## What is faked / out of scope

Audio (design calls it primary — stubbed to haptics + visuals for now), real enemy AI,
any sim/render split, determinism, multiplayer. All intentionally absent.

## Run it

**On desktop** (quick logic check — mouse emulates touch):

    godot --path . --quit-after 0      # or just: godot --path .

**On the phone** (the *real* test — touch feel is the whole point):

    ./deploy.sh          # export APK → adb install → launch → logcat
    ./deploy.sh build    # just (re)build out/gonedark-phase0.apk

Needs `godot` (4.6.x) and `adb` on PATH and a phone in USB-debugging mode. Package id
`com.gonedark.phase0proto`; arm64-v8a; gl_compatibility renderer for wide device reach.

## Exit criterion

Per the roadmap: **the embody ↔ command loop feels good in hand.** If it doesn't, rework
or kill the concept *here* — before any engine work. Capture the verdict as a decision in
[`../../docs/decisions.md`](../../docs/decisions.md) and update the roadmap.
