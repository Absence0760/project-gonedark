# Placeholder model generator

Scripted, license-clean greybox models for the publishable build — see
[`decisions.md` D41](../../docs/decisions.md) and
[`content-pipeline.md` §2/§5/§6](../../docs/content-pipeline.md).

These are **placeholders**, not art: primitives welded into one mesh per object, exported as
`.glb` into a category subfolder under [`../../assets/models/`](../../assets/models/) with a
license manifest. They read fine as top-down RTS tokens; the honest weak axis is eye-level FPS
credibility (the two-view filter, §4) — that's the accepted placeholder trade, fixed later by
the mid/hero art pass.

A silhouette pass sharpens the eye-level read **within the same tri budget** (no LOD0 grows
past the ~1200 infantry/tank ceiling): troopers carry a real neck + broad shoulder yoke
tapering to a narrower waist with the forearms brought forward into a weapon-ready pose (so they
read as a rifleman, not a coat-rack); the tank turrets seat on a ring foot with a sloped mantlet
+ coax MG; the rifle viewmodels gain a low-profile optic. The faction silhouette tells are
preserved (US rounded helmet / long-flat Abrams + broad turret / M4 mag-forward; FR flat-brimmed
SPECTRA / compact Leclerc + rear autoloader bustle + sight mast / FAMAS bullpup carry-handle).

A second pass gave the **structures and props** the same treatment so they read as believable
emplacements/scenery at both top-down and eye level (still modest tri counts — all well under the
infantry ceiling): the **camp HQ** is a command building (hipped roof + ridge vent, corner
pilasters framing a recessed doorway under an entrance awning, flanking windows, a rooftop vent +
antenna mast with cross-spreader); the **defensive turret** is a credible weapon emplacement
(ring-plated pad, rotating drum, armoured housing with a sloped face shield, elevation trunnions,
a sensor block, and a barrel with shroud + muzzle brake); the **barricade** is a three-course
sagging sandbag berm (offset running bond, each bag flattened/rotated with a deterministic
per-bag wobble); the **crate** carries diagonal cross-brace battens + a milled lid seam + bracing
cleats (a reinforced military crate, not a flat cube); the **tree** is a six-tier conifer whose tiers
are rotated + nudged off-axis for a ragged, hand-grown silhouette with a splayed root flare; the
**rock** is a tilted cleaved boulder with a jutting broken shard + shed chips. (The barricade and
sandbag courses are deliberately tri-light box stacks; the heavy per-bag chamfer does the rounding.)
Per-faction **`turret_us`** (crew-served .50-cal emplacement — low bolted housing, perforated jacket,
ammo can, squared shield; CARC grey-green) and **`turret_fr`** (remote weapon station — stabilised gun
pod on a slewing mast, boxed thermal sight, no crew shield; French green) give the two armies
contrasting emplacement silhouettes (`factions.md` identity language).

One budget note: at the chamfer's 40° angle limit, cylinder side edges stay un-beveled only at
**≥10 facets** (a 9- or 8-gon's wider inter-facet angle trips the limit and bevels every edge,
*adding* geometry) — so limbs/wheels/barrels stay at `verts=10`, rings at `12`.

## Layout

Models live in role subfolders under `assets/models/` (set per model by `CATEGORY` in
`gen_models.py`) so the asset tree stays browsable instead of one flat dump:

| Folder | Models |
|---|---|
| `units/` | `trooper`, `tank`, `tank_turret` |
| `structures/` | `camp_hq`, `turret`, `turret_us`, `turret_fr`, `barricade` |
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
| `camp_hq` | `structures` | Command building (hipped roof, doorway + awning, windows, antenna mast) |
| `turret` | `structures` | Weapon emplacement (pad/drum/housing/shield/trunnions/barrel + muzzle brake) |
| `turret_us` | `structures` | US crew-served .50-cal emplacement (bolted housing, perforated jacket, ammo can, squared shield) |
| `turret_fr` | `structures` | FR remote weapon station (stabilised gun pod on a slewing mast, boxed thermal sight, no crew shield) |
| `barricade` | `structures` | Three-course sagging sandbag berm cover |
| `weapon_rifle` | `weapons` | First-person weapon viewmodel |
| `crate` | `props` | 1 m reinforced cover crate (cross-brace battens + lid seam) |
| `tree` | `props` | Scenery / soft cover (trunk + six-tier ragged conifer canopy + root flare) |
| `rock` | `props` | Scenery / hard cover (cleaved boulder + broken shard) |
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

> **Determinism caveat.** The models with a **UV-sphere** part (`trooper` and its faction
> variants `trooper_us`/`trooper_fr` — the head) are **non-deterministic run-to-run**: Blender's
> UV-sphere tessellation varies between runs, so their LOD0 `.mesh` bytes (and thus the derived LOD
> tiers) change on regeneration even though the geometry is equivalent. The box/cylinder/cone/
> **icosphere** models — including `tree` (cones) and `rock` (icospheres) — are bit-reproducible.
> Treat the committed `.mesh` files as the golden artifacts; the render crate golden test checks
> validity, not bytes.

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
