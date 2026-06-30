# Embodied game-feel floor (CP-2 acceptance gate)

This is the **written "good-enough floor"** the visual-design plan's WS-A calls for — the checklist a
playtester reads to decide *"does the gunplay clear the bar?"* The bar is deliberately modest: a
Delta-Force / CoD-Mobile player should pick up an embodied unit and **not bounce in ten seconds**. It
is *not* UE5 parity (we concede photoreal fidelity and compete on the hybrid — see
[`positioning`](positioning/positioning.md)); it is *"the gun feels alive and the shot reads."*

Everything below is **presentation/feel only** — none of it touches deterministic sim state
(invariant #4). All of it is stepped from the **wall-clock `dt`**, never the 60 Hz sim tick, and lives
in the host/render path (`engine::recoil`, `engine::scope`, `render::{hud,impact,world}`,
`pal::mix`). The authoritative cone/dispersion/hit resolution stays in `core`, untouched.

---

## The checklist (playtest against this)

A build **clears the floor** when every line is a yes:

- [ ] **Hit feedback is present and immediate.** A connecting shot snaps a centered hitmarker (the
  white "X", WS-4), a coupled **impact spark/dust** appears at the point you hit, and you hear *both*
  the UI hit-tick and a world thud — within the same ~0.15 s window.
- [ ] **The gun reads as firing.** Every trigger pull flares a **shaped muzzle flash** (hot core +
  star), kicks the weapon viewmodel, and **cracks audibly even when it misses** (the host-clock fire
  cue, decoupled from whether the shot landed). Silence on a missed shot is a fail.
- [ ] **Recoil is readable, recovers, and never disorients.** Sustained fire climbs the view (an
  upward pitch punch) and **blooms the crosshair** outward; both **settle back to rest within ~0.5 s**
  of releasing the trigger. The punch is a climb, not a lurch (~2.4° at a saturated gun).
- [ ] **The crosshair communicates spread.** At rest the reticle is tight; under fire the four arms
  spread with the bloom and pull back in as the gun settles. It stays square on a wide (phone) window.
- [ ] **ADS is responsive and worth using.** Right-click / the ADS button **steadies and tightens** the
  shot: the FOV narrows snappily (~0.125 s) and the look sensitivity drops so aiming is calmer — but
  never *glued* (floored at 45%). Infantry get a gentle iron-sight (~1.7×); the tank keeps its
  sniper gun-sight (~3.3×) + scope chrome.
- [ ] **Audio is coupled to the visual.** Fire/impact cues land in lockstep with the muzzle/impact
  flashes — no audible lag, no double-thunk. (Deliberate *sound identity* is CP-6; WS-A owns only the
  **coupling timing**, and ships placeholder synth.)
- [ ] **Nothing reads as unfair (invariant #6).** No feedback element reveals an unseen enemy: the
  hitmarker/impact/muzzle are feedback on **your own** action; ADS *narrows* the frustum (reveals
  less). "World goes dark" still strips the strategic map, not the soldier in your sights.
- [ ] **It holds on a phone-sized viewport.** Verify the crosshair, ADS, and bursts on a 16:9 (and
  narrow) window — round elements stay round, the cross stays square (the raw-NDC chrome footgun).

A **playtest sign-off** (a human firing an embodied unit for a minute and ticking the list) is the
other half of the gate — carried honestly, not faked. The `viz-runner` pixel-asserts the
kick/flash/crosshair on a firing frame; the *feel* half is human.

---

## Shipped tunables (the starting values)

These are what WS-A shipped, and the knobs to turn during playtest. Each is a named const in the
seam cited — change it there, re-run the suite (dev + release), and re-shoot the `viz-runner` PNG.

### Recoil / view-kick / crosshair bloom — `engine::recoil`

| Tunable | Value | Meaning |
|---|---|---|
| `RECOIL_PER_SHOT` | `1.0` | recoil units added per trigger pull |
| `RECOIL_MAX` | `3.5` | accumulator ceiling (sustained fire plateaus) |
| `RECOIL_RECOVERY` | `7.0` /s | settle rate → full gun recovers in ~0.5 s |
| `KICK_PITCH_PER_RECOIL` | `0.012` rad | upward view climb per recoil unit (~2.4° max) |
| `BLOOM_PER_RECOIL` | `0.016` NDC | crosshair arm spread per recoil unit |

Crosshair geometry (`render::hud`): `CROSSHAIR_GAP = 0.030` (resting half-gap, NDC),
`CROSSHAIR_DOT_HALF = 0.011` (tick size).

> **Design note — pitch, not yaw.** The view-kick climbs the camera *pitch* only (cosmetic: the sim
> aim is 2-D yaw, so the bullet never moves and the screen-center crosshair stays on the fire
> direction). The horizontal half of recoil is carried by the **crosshair bloom**, not a camera-yaw
> offset (which would desync the reticle from the shot). A true horizontal camera-yaw kick is a
> possible follow-up if a recoil *pattern* is ever wanted, but it needs a fairness pass first.

### Aim-down-sight — `engine::scope`

| Tunable | Value | Meaning |
|---|---|---|
| `ADS_FOV_DEG` | `42.0` | infantry iron-sight FOV (~1.7× from the 60° base) |
| `SCOPED_FOV_DEG` | `20.0` | tank gun-sight FOV (~3.3×) |
| `ZOOM_RATE` | `8.0` /s | FOV ease — full ADS in ~0.125 s |
| `ADS_SENS_FLOOR` | `0.45` | look-sensitivity never drops below 45% even fully zoomed |

Look sensitivity scales `1.0 → 1/magnification` with the zoom (`ads_look_scale`), floored above.

### Muzzle / impact VFX

`render::world` muzzle flash: `MUZZLE_ANCHOR = (0.14, -0.07)` (NDC), `MUZZLE_FLASH_TICKS = 8`
(~0.13 s). `render::impact` burst: `IMPACT_TICKS = 9` (~0.15 s), warm spark `IMPACT_COLOR =
(1.0, 0.80, 0.45)`, core radius `0.040` NDC shrinking with the fade, dust ring `0.025 → 0.085` NDC
expanding as it ages.

### Audio coupling — `pal::SoundId` / `pal::mix`

New host-clock cues (placeholder synth; identity is CP-6): `WeaponFire` (a press-time crack, decoupled
from the connecting-shot `Gunfire`), `Impact` (a strike thud coupled to the impact VFX). The existing
`HitConfirm` UI tick still fires on a landed shot.

---

## What is explicitly **out** of this floor

- **Sound identity** (the *character* of the cues) → CP-6. WS-A only owns coupling timing.
- **Animation** (locomotion / fire / death clips) → CP-3 / WS-B.
- **Tracers** — deferred (a full tracer pass isn't "cheap"); the muzzle flash + impact burst carry the
  shot read for now. A follow-up if playtest says the round needs a visible streak.
- **Per-weapon recoil patterns / gunsmith-driven feel** — the gunplay-feel layer is one shared curve
  today; per-weapon tuning rides on CP-1's gunsmith breadth later (still fairness-bounded, horizontal).
