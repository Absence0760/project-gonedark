# Content Pipeline — assets, quality tiers & open-source sourcing

> How art and world content get *sourced*, *graded*, and *cooked* into the game —
> without a 3× authoring bill, without a license landmine, and without breaking the
> "one world, two views" constraint ([`architecture.md`](architecture.md)). Assets are
> **render-only**, so none of this touches the deterministic sim — the no-floats
> invariant does not reach here. That's the one piece of good news up front.

This is the production-side companion to the runtime asset notes in
[`architecture.md`](architecture.md) ("Asset pipeline & loading") and the device-tier
notes in [`platforms.md`](platforms.md) §6. Those cover *runtime*; this covers *where
content comes from* and *how it's graded* before it gets there.

## 1. "Low / medium / high" is three axes, not one

The instinct is to think of "quality" as a single dial. It isn't — conflating these is
how you end up authoring the same asset three times.

| Axis | What varies | Owner | Status |
|---|---|---|---|
| **A. Device tier** (runtime) | same asset, scaled per phone class — low/mid/flagship | cook + runtime | designed ([`platforms.md`](platforms.md) §6) |
| **B. View / LOD** (runtime) | top-down RTS token vs eye-level FPS mesh | cook (LOD chain) | greybox LOD chain **implemented** (gltfpack, §2); hero-tier tension flagged ([`architecture.md`](architecture.md)) |
| **C. Production maturity** (temporal) | greybox placeholder → mid → final source art | sourcing | **this doc** |

**The load-bearing rule:** author **one high-quality source asset**, then *derive* the
lower tiers in the **cook step**. Low/medium/high are *build outputs*, not three art
tasks. The single source feeds the LOD chain (axis B) and the per-device ASTC/atlas
variants (axis A). A small team can only afford low/med/high if they fall out of the
build, not the budget.

> **Don't confuse high *source* with flagship *device*.** A flagship phone still runs a
> cooked-down variant of the high source. "How good is the art" (axis C) and "how much
> can this GPU push" (axis A) are separate knobs that happen to both say "high."

```
   ONE high-quality source (.glb + PBR maps)
            │
            ▼   cook step  (offline; see architecture.md "Asset pipeline & loading")
   ┌────────┴─────────┬──────────────┬──────────────┐
   ▼                  ▼              ▼              ▼
 LOD0 eye-level   LOD1..n decimate  ASTC tiers   atlas + LZ4 pak
 (FPS view)       (top-down/dist)   (low/mid/    (mmap-ready)
                                     flagship)
```

## 2. The production ladder (axis C)

Placeholder-first. Don't buy or commission art before the slice proves the space works
in both views.

- **Low / greybox** — kit-bashed primitives + CC0 mobile packs (procedurally generated
  where possible, see §5) **plus scripted procedural models** ([`decisions.md`](decisions.md)
  D41). Purpose: *play* the Phase 1 slice and prove the "one world, two views" thesis
  ([`architecture.md`](architecture.md)) before spending a cent on art. Disposable.
  **For the push to a publishable build (D41), this is the *default* tier for every visible
  object** — units, structures, environment props, and the embodied weapon — built by a
  Claude-authored headless **Blender (`bpy`) script** (`tools/models/gen_models.py` → one `.glb`
  and one cooked `.mesh` per object; the `.mesh` is the runtime format the renderer loads,
  [`decisions.md`](decisions.md) D44), not pulled from an external text-to-3D service. That keeps it license-clean by construction: code-authored
  geometry from primitives has no third-party tool terms to vet, so each asset's manifest reads
  `source: procedural (Blender bpy)`, `license: CC0-1.0` — license hygiene (§3) is *satisfied*,
  not a question. These ride the *same* pipeline as anything else (cook → LOD §1, two-view filter
  §4); their honest weak axis is eye-level FPS credibility (§4), the accepted placeholder trade.
  **The LOD chain (axis B) is now built for this tier.** After the full-detail cook,
  `gen_models.py` runs `gltfpack -si <ratio> -sa` over each model's `.glb` and re-imports the
  simplified result into Blender to re-run the *same* `.mesh` cook — so every tier lands in the
  identical GDM1 format with freshly recomputed flat normals. Each model emits a monotone
  decimation pyramid beside its full-detail `<name>.mesh`:

  | Tier | Cooked file | ~Triangles | Built from |
  |---|---|---|---|
  | LOD0 | `<name>.mesh` | full detail | the welded primitives (unchanged path) |
  | LOD1 | `<name>.lod1.mesh` | ~½ of LOD0 | `gltfpack -si 0.5 -sa` on `<name>.glb` |
  | LOD2 | `<name>.lod2.mesh` | ~¼ of LOD0 | `gltfpack -si 0.5 -sa` chained on `<name>.lod1.glb` |

  `-sa` (aggressive) is required because the flat-shaded soup splits normals at every face, so a
  plain `-si` finds almost no shared edges to collapse; chaining LOD2 off LOD1's glb keeps the
  pyramid monotone (simplification never *adds* triangles). Already-minimal models (crate, rock,
  barricade) floor out and their lower tiers equal LOD0 — still emitted, so the runtime loads a
  uniform tier set. Per-tier byte/sha/`tri_count` stats live in each asset's `lods` array in
  `manifest.json`. The renderer selects a tier by on-screen size: LOD0 for the embodied eye-level
  view, the decimated tiers for distant / top-down command-view tokens.
- **Mid** — curated open-source assets, decimated and re-textured to mobile budget, that
  pass the two-view filter (§4). The default tier most of the game ships at — the endgame
  target the D41 AI placeholders are eventually *replaced* by, not the launch tier.
- **High / hero** — final assets (commissioned, bought, or AI-assisted) reserved for the
  few things the camera *lingers* on at eye level — the embodied weapon, the player's own
  unit, signature structures. Everything else stays mid. Hero art is rationed, not
  spread. (D41 pulls the AI-assisted route *forward* to the whole greybox tier for now; the
  hero tier stays the later, rationed spend.)

## 3. License hygiene (hard constraint)

This repo is potentially public (invariant #8). Every asset carries a tracked license,
and the build enforces it.

| License class | Verdict | Why |
|---|---|---|
| **CC0 / public domain** | ✅ preferred | No attribution burden, no viral terms. Default target. |
| **CC-BY** | ✅ with attribution | Fine *if* it lands in a generated `CREDITS`/attribution manifest. |
| **CC-BY-SA**, GPL-art | ⚠️ avoid | Share-alike / viral terms can reach into the project. |
| **CC-BY-NC** | ❌ no | Non-commercial kills any shipping plan. |
| **EULA-bound "free"** (e.g. Mixamo) | ⚠️ read terms | "Free" ≠ redistributable; per-asset legal check. |

**Mechanism:** every asset ships with a manifest entry — `source`, `author`, `license`,
`url`, `sha256` — and CI **fails the build** on a missing or disallowed license. The
`CREDITS` file is *generated* from those manifests, never hand-maintained.

## 4. The two-view filter (the real killer)

Most CC0 RTS asset packs are low-poly and **top-down-only** — they look like garbage at
eye level. An asset qualifies only if it reads as an RTS token *and* holds up as an FPS
space mesh. This is exactly the unbudgeted "one world, two views" tension in
[`architecture.md`](architecture.md), now made a *gating check on sourcing*:

- A **two-view validation harness** renders any candidate asset both top-down and at
  eye level, side by side, so the filter is a fast yes/no — not a discovery made after
  it's in the level.
- Eye-level credibility (silhouette, mesh density, texel density, collision proxy) is the
  expensive axis. Budget hero detail only where the avatar can walk up to it.

## 5. Worlds & terrain — lean procedural

Fixed-license "world" megapacks rarely survive §3 *and* §4. Prefer generation:

- **Terrain / layout** — procedural (Blender geometry-nodes offline, or in-engine
  generation) gives CC0-clean, budget-tunable worlds and greybox levels for free.
- **Materials** — CC0 PBR libraries (ambientCG, Poly Haven) decimated/atlased in the cook
  step; scanned 4K sources are desktop-weight and must be down-ranged for mobile.
- **Collision** is authored/derived separately from visual LOD — the FPS view needs fine
  collision the top-down view never does (the axis-B cost again).

**Vetted CC0/CC-BY starting sources:** Kenney (CC0, game-ready, mobile-friendly),
Quaternius (CC0 characters/props), Poly Haven (CC0 HDRIs/textures/models), ambientCG
(CC0 materials), Sketchfab (filter to CC0/CC-BY), OpenGameArt (mixed — vet per-asset).

## 6. What Claude builds vs what it can't

Honest boundary: **Claude owns the pipeline and the curation, not the art.**

| Claude **can** | Claude **cannot** |
|---|---|
| Build & own the **cook step** (glb → cooked `.mesh` (greybox) + LOD + ASTC + atlas + LZ4 pak; tier derivation) | Sculpt hero meshes to a final-art bar |
| Enforce **license/provenance** in CI; generate `CREDITS` | Paint textures, rig/animate characters |
| Generate **procedural & greybox** content (terrain, kit-bash, collision proxies) | Make art-direction calls |
| Write & maintain the **sourcing guide**; vet licenses | Own a EULA legal decision (flags it, you decide) |
| Build the **two-view validation harness** | — |
| Integrate **AI-asset tools** (text-to-3D) as a pipeline stage | — |

AI-asset generators (Meshy, Tripo, etc.) can sit in the pipeline as a *stage* Claude
scripts — but their **license terms and output quality are yours to own**, and generated
meshes still pass §4's filter and §1's cook like any other source.

**The installed toolbox (machine-wide, headless-scriptable).** The "can" column is backed
by concrete CLIs on the workstation that Claude drives the way D41 drives Blender —
`--background` / `--export` / a script file, no GUI. **Reach for these first** when a task
needs an asset; script the generator and commit the *script* + a manifest entry
(`source` / `license` / `sha256`, §3), never an opaque binary blob.

| Tool | Lane | Used for |
|---|---|---|
| **Blender** (`bpy`) | 3D author | procedural/greybox meshes, geometry-nodes terrain, rig/anim, glTF export (`tools/models/gen_models.py`, D41) |
| **gltfpack** | 3D cook | glTF mesh/texture compression (meshopt/Draco) for the mobile / 200-unit budget; **drives the greybox LOD chain** (`-si … -sa`, gen_models.py §2) |
| **SoX** | audio | SFX synthesis + processing |
| **Csound** | audio | deterministic, **seed-scripted** SFX — regenerable + git-diffable, the audio analogue of D41 (audio is a primary system, invariant #6) |
| **Inkscape** (`--export-type=png`) | 2D / UI | vector → PNG HUD / command-layer icons across DPIs |
| **ImageMagick** (`magick`) | 2D | scripted textures, atlases, noise / normal maps |

Install provenance + how each was added lives in the workstation conventions (`~/CLAUDE.md`);
the toolchain choice is logged as **D46**.

## 7. Open fork

How far to lean **CC0-curated** vs **commissioned** vs **AI-generated** for the hero
tier (axis C, §2) is a genuine cost/identity fork, tracked as
[`open-questions.md`](open-questions.md) **Q11** — not decided here. The *pipeline* above
is rate- and source-agnostic: it cooks and license-checks whatever the hero strategy
turns out to be.
