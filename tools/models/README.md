# Placeholder model generator

Scripted, license-clean greybox models for the publishable build — see
[`decisions.md` D41](../../docs/decisions.md) and
[`content-pipeline.md` §2/§5/§6](../../docs/content-pipeline.md).

These are **placeholders**, not art: blocky primitives welded into one mesh per object,
exported as `.glb` into [`../../assets/models/`](../../assets/models/) with a license
manifest. They read fine as top-down RTS tokens; the honest weak axis is eye-level FPS
credibility (the two-view filter, §4) — that's the accepted placeholder trade, fixed later
by the mid/hero art pass.

## Run it

Needs **Blender 5.x** on your `PATH` (the script targets the `bpy` 5.x API; verified on
5.1.2). Generate (or regenerate) every model + the manifest:

```
pnpm assets:models
# equivalently:
blender --background --python tools/models/gen_models.py
```

Output: one `.glb` per object in `assets/models/`, plus `assets/models/manifest.json`
(each entry records `source`, `author`, `license: CC0-1.0`, `bytes`, `sha256`).

## What it builds

| File | What |
|---|---|
| `trooper.glb` | Greybox infantry unit (boxy humanoid) |
| `tank.glb` | Greybox vehicle unit (hull/tracks/turret/barrel) |
| `camp_hq.glb` | Greybox structure (walls + pyramid roof + antenna) |
| `weapon_rifle.glb` | First-person weapon viewmodel |
| `crate.glb` | 1 m cover prop |

## Adding / editing a model

Edit `gen_models.py`: add a `build_*()` that returns a single welded object (see
`weld()`), then add it to `MODELS`. Conventions: Z-up, base/feet at `z ≈ 0`, sizes ~metres.
Re-run `pnpm assets:models`.

## Not wired into the renderer yet

`render` currently draws procedural instanced primitives and has **no glTF loader**, so
these `.glb` are source assets with no runtime consumer. Wiring them in (a loader + the
cook → LOD step) is separate follow-on work, flagged in D41.
