## Summary

<!-- 1–3 sentences: what this changes and why. -->

## Changes

-
-

## Surface touched

- [ ] `core/` — the deterministic sim (fixed-point, no platform deps)
- [ ] `pal/` / `pal-desktop/` / `pal-android/` — the platform boundary
- [ ] `render/` — renderer, HUD, shaders
- [ ] `engine/` / `app/` / `android/` — game loop and shells
- [ ] `server/` — backend scaffolding
- [ ] Netcode / lockstep
- [ ] Assets + generator scripts (`tools/`, `assets/`)
- [ ] Infrastructure (`infra/`) or CI (`.github/`)
- [ ] Docs only

## Invariant checklist

<!-- These are the load-bearing decisions from CLAUDE.md. Tick what you
     verified. If a row genuinely doesn't apply, leave it unticked rather
     than deleting it — the next reviewer should see it was considered. -->

- [ ] **No floats in the sim.** No `f32`/`f64` and no std/libm transcendentals
      reached sim/core types or math (floats live only in rendering)
- [ ] **The PAL boundary held.** `core` and `pal` gained no `wgpu`/`winit`/JNI
      dependency, and no platform detail leaked into game logic
- [ ] **Unit AI stayed a literal executor** — no autonomous strategy was added;
      new depth went into the order/stance vocabulary
- [ ] **Sim and render stayed decoupled** — the sim touched no GPU API, the
      renderer mutated no sim state
- [ ] **Going dark stayed fair** — while embodied this surfaces alerts, not
      intel; no map reveal, no presentation path that reads un-derived state
- [ ] **No secret** landed in `.env*`, code, or any tracked file

## Tests

<!-- Per CLAUDE.md, a non-trivial change ships its tests in the same commit.
     If logic sits behind an unconstructible platform type, say which seam you
     extracted and tested instead — don't skip coverage silently. -->

- [ ] Unit tests added/extended for the changed logic
- [ ] `cargo test` green in **both** profiles (dev + release)
- [ ] Determinism matrix green (cross-arch checksum diff)
- [ ] Coverage deliberately skipped for: <!-- name the glue and why, or delete -->

## Decision log

- [ ] This resolves an open question (`Qn`) → migrated to a `Dn` in `docs/decisions.md`
- [ ] This raises a *new* fork → added to `docs/open-questions.md`
- [ ] Neither applies
