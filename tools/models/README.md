# Placeholder model generator

Scripted, license-clean greybox models for the publishable build — see
[`decisions.md` D41](../../docs/decisions.md) and
[`content-pipeline.md` §2/§5/§6](../../docs/content-pipeline.md).

These are **placeholders**, not art: blocky primitives welded into one mesh per object,
exported as `.glb` into a category subfolder under
[`../../assets/models/`](../../assets/models/) with a license manifest. They read fine as
top-down RTS tokens; the honest weak axis is eye-level FPS credibility (the two-view filter,
§4) — that's the accepted placeholder trade, fixed later by the mid/hero art pass.

## Layout

Models live in role subfolders under `assets/models/` (set per model by `CATEGORY` in
`gen_models.py`) so the asset tree stays browsable instead of one flat dump:

| Folder | Models |
|---|---|
| `units/` | `trooper`, `tank`, `tank_turret` |
| `structures/` | `camp_hq`, `turret`, `barricade` |
| `weapons/` | `weapon_rifle` |
| `props/` | `crate`, `tree`, `rock` |
| `fx/` | `tracer` |

The category prefix is part of the on-disk path everywhere: the renderer's `include_bytes!`
paths in `render/src/mesh.rs` and each manifest entry's `category` + `file`/`cooked` paths all
carry it. `manifest.json` itself stays at the `assets/models/` root.

## Run it

Needs **Blender 5.x** on your `PATH` (the script targets the `bpy` 5.x API; verified on
5.1.2). Generate (or regenerate) every model + the manifest:

```
pnpm assets:models
# equivalently:
blender --background --python tools/models/gen_models.py
```

Output, per object, in its `assets/models/<category>/` subfolder: a full-detail `.glb` +
cooked `.mesh`, plus a gltfpack-decimated LOD chain (`<name>.lod1.*`, `<name>.lod2.*`); plus
`manifest.json` at the `assets/models/` root (each entry records `category`, `source`,
`author`, `license: CC0-1.0`, `base_color`, `bytes`, `sha256`, and a `lods` array of per-tier
stats).

## What it builds

Eleven models — units, structures, weapons, props, scenery, fx:

| Name | Category | What |
|---|---|---|
| `trooper` | `units` | Greybox infantry unit (boxy humanoid) |
| `tank` | `units` | Greybox vehicle hull (chassis + tracks) |
| `tank_turret` | `units` | Tank turret (mantlet + barrel, slews on the hull's ring) |
| `camp_hq` | `structures` | Greybox structure (walls + pyramid roof + antenna) |
| `turret` | `structures` | Defensive structure (base/drum/housing/barrel) |
| `barricade` | `structures` | Two-course sandbag berm cover |
| `weapon_rifle` | `weapons` | First-person weapon viewmodel |
| `crate` | `props` | 1 m cover prop |
| `tree` | `props` | Scenery / soft cover (trunk + two-tier canopy) |
| `rock` | `props` | Scenery / hard cover (faceted boulder) |
| `tracer` | `fx` | Tank-shell tracer (small +X-elongated bolt) |

## The LOD chain

Each model ships three tiers in the cooked GDM1 `.mesh` format the renderer loads
(`render/src/mesh.rs`). The full-detail tier is the welded primitives; the decimated tiers come
from **gltfpack** (`-si <ratio> -sa`) re-imported into Blender and re-cooked through the *same*
`export_mesh`, so every tier is byte-format-identical with freshly recomputed flat normals.

| Tier | Cooked file | Source glb | Built from |
|---|---|---|---|
| LOD0 | `<name>.mesh` | `<name>.glb` | welded primitives (full detail) |
| LOD1 | `<name>.lod1.mesh` | `<name>.lod1.glb` | `gltfpack -si 0.5 -sa` on `<name>.glb` |
| LOD2 | `<name>.lod2.mesh` | `<name>.lod2.glb` | `gltfpack -si 0.5 -sa` chained on `<name>.lod1.glb` |

- **`-sa` (aggressive) is deliberate.** The cooked mesh is a flat-shaded soup — adjacent faces
  don't share normals — so a plain `-si` finds almost no collapsible edges. `-sa` welds across
  those discontinuities to actually decimate (geometric quality is secondary for a distance LOD).
- **LOD2 is chained off LOD1's glb**, not the source, so the pyramid is monotone:
  `tris(LOD2) ≤ tris(LOD1) ≤ tris(LOD0)` by construction.
- **Already-minimal models** (crate, rock, barricade) floor out — their lower tiers equal LOD0.
  They're still emitted so the runtime can load a uniform tier set.
- Naming contract the renderer relies on: full = `<name>.mesh`; decimated = `<name>.lod1.mesh`,
  `<name>.lod2.mesh`. The `.lodN.glb` siblings are kept for provenance.

> **Determinism caveat.** The sphere-based models (`trooper`, `tree`, `rock`) are
> **non-deterministic run-to-run** — Blender's UV-sphere tessellation varies between runs, so
> their LOD0 `.mesh` bytes (and thus the derived LOD tiers) change on regeneration even though the
> geometry is equivalent. The box/cylinder/cone models are bit-reproducible. Treat the committed
> `.mesh` files as the golden artifacts; the render crate golden test checks validity, not bytes.

## Adding / editing a model

Edit `gen_models.py`: add a `build_*()` that returns a single welded object (see
`weld()`), then add it to `MODELS`. Conventions: Z-up, base/feet at `z ≈ 0`, sizes ~metres.
Re-run `pnpm assets:models`.

## Runtime consumer

The cooked `.mesh` files are loaded by `render/src/mesh.rs` (`MeshCpu::parse` →
`MeshLibrary`), which `include_bytes!`s them so they ride into the binary/APK with no on-device
file IO. The `.glb` files remain the interchange source-of-record; the engine never parses glTF
at runtime. The **cook → LOD step is implemented here** (this script, gltfpack); per-device
ASTC/atlas/LZ4-pak cooking (axis A) is the later heavyweight step reserved for `/assets/cooked/`.
