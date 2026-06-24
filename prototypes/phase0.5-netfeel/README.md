# Phase 0.5 — netfeel harness *(THROWAWAY)*

Disposable **Godot 4.6** harness that feel-tests embodied 1v1 FPS combat over
deterministic-lockstep + input-delay netcode, to resolve
[`../../docs/open-questions.md`](../../docs/open-questions.md) **Q7** (netcode model)
and **Q8** (tick rate) *before* the Rust engine spine. Full rationale, test matrix, and
decision tree: **[`../../docs/phase-0.5-plan.md`](../../docs/phase-0.5-plan.md)**.

**Not the engine. Not a determinism test** — floats are fine here; both peers run
identical code on the same exchanged inputs and we do not checksum. Delete this dir once
Q7/Q8 are settled (alongside the Phase 0 prototype).

## What it implements

- **Lockstep core**: peers exchange per-tick **input commands** (not state, redundant
  window of `W`) and both simulate both avatars at a fixed **30/60 Hz** tick; an input
  sampled at tick `T` executes at `T+D` (`D` auto-derived from the injected RTT).
- **Latency injector**: tunable RTT / jitter / loss on the send path (a LAN is ~0 ms and
  would flatter lockstep). Cycle presets with **NET**.
- **MODE A — pure lockstep**: your camera/aim/move lag by `D` ticks (the mushy baseline).
- **MODE B — avatar-local prediction**: camera/aim respond immediately, movement is
  predicted (optimistic, smoothed → rubber-bands on mispredict); the authoritative result
  still resolves at `T+D` and is what fire + the remote view use. Toggle with **MODE**.
- 1v1 fight: left-thumb stick, right-drag look, **FIRE** hitscan, hp/respawn. Diagnostics
  HUD shows role · mode · tick · `D` · injector preset · `send→sim` lag · HP.

## Run it — you need two clients

Easiest bring-up — **desktop hosts, phone joins**:

    ./deploy.sh host                 # HOST on this desktop; note its LAN IP on screen
    ./deploy.sh                      # build+install+launch on the phone, then tap JOIN <that IP>

Or **two phones** on one Wi-Fi (the realistic pass): install on both, one HOSTs, the other
JOINs the host's shown IP. For a quick injector sweep with no second device, run two
desktop instances: `./deploy.sh host` and `./deploy.sh join 127.0.0.1`.

Needs `godot` (4.6.x) + `adb` on PATH. Package `com.gonedark.phase05netfeel`, arm64,
gl_compatibility.

## What to do with it

Work the matrix in [`../../docs/phase-0.5-plan.md`](../../docs/phase-0.5-plan.md) §7 (modes
× tick × injected RTT/jitter/loss), judging feel in hand, then record the Q7/Q8 outcome as
a decision. Tune constants at the top of `Main.gd`.
