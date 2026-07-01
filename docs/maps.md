# Real-world battlefield maps

How Going Dark turns a real place — or a historical battlefield — into a playable map, and how
you diagnose one when it misbehaves. The tooling lives in [`tools/maps/`](../tools/maps/); the
decision behind it is [D80](decisions.md); it is the real-world-sourced sibling of the procedural
generator in [`content-tooling-plan.md`](plans/content-tooling-plan.md) (CT-C/CT-F/CT-G).

> **Status:** a working **spike**. The ingest→bake→lint pipeline runs end-to-end and produces a
> real, `core::terrain`-compatible map (the Pointe du Hoc sample). Sim-side elevation, true
> impassability, and destructibility are deliberately *not* built yet — see
> [§ Known gaps](#known-gaps).

---

## The one idea: two decoupled artifacts from one source

A real battlefield is **not** one asset. It is two, decoupled per [invariant #4](../CLAUDE.md)
(sim/render separation), because real GIS data is floating-point and [invariant #1](../CLAUDE.md)
forbids a single float in the sim:

| Artifact | Consumer | Numbers | Detail | In the checksum? |
|---|---|---|---|---|
| `.covergrid` | **sim** (`core::terrain`) | **integer** `Cover` | coarse, 128×128 | no — static map data ([D28](decisions.md)/[D77](decisions.md)) |
| `*.terrain.glb` | **render** | float, real metres | fine, textured | n/a (render-only) |

Real elevation (a DEM in float metres) is exactly what a **render mesh** wants and exactly what the
sim must never see. So the pipeline runs the float work **offline** in the baker and emits an
**integer, byte-stable** cover grid; the pretty float terrain lives only in the render mesh. This is
the whole design — everything else is plumbing.

```
 config/<name>.json      ingest.py            bake.py               terrain_mesh.py
 ┌──────────────┐  bbox  ┌──────────┐ features┌──────────┐  grid    ┌──────────────┐
 │ bbox, era,   │ ─────► │ DEM +    │ ──────► │ rasterise│ ───────► │ render mesh   │
 │ mode,        │        │ OSM      │         │ → Cover  │  cover   │ (glTF, float, │
 │ fidelity     │        │ fetch    │ height  │  grid    │          │  render-only) │
 └──────────────┘        └────┬─────┘         └────┬─────┘          └──────┬───────┘
                              │ height.f32          │ .covergrid            │ height.f32
                              ▼  RENDER-ONLY         ▼  SIM (integer)        ▼
                     ┌──────────────────────────────────────────────────────────────┐
                     │ SIM: 128×128 Cover grid → core::terrain (invariants #1, #4)    │
                     │ RENDER: displaced glTF terrain mesh (float, decoupled)         │
                     └──────────────────────────────────────────────────────────────┘
```

---

## Where the data comes from

Three subjects, one pipeline, selected by the config's `mode`:

- **Modern real locations** (`mode: modern`) — the cleanest. Elevation from a public **DEM**
  (Copernicus DEM GLO-30, SRTM, or USGS 3DEP LiDAR); vector features (buildings, walls, hedges,
  forest, water, landcover) from **OpenStreetMap** via Overpass / `osmnx`, with Microsoft/Google
  building footprints where OSM is thin and ESA WorldCover for vegetation.
- **Historical battlefields** (`mode: historical`) — harder, because *the terrain has changed*
  (WWI cratering, moved rivers, lost bocage). Take a modern DEM as the geological base, then overlay
  period features from **georeferenced trench maps** (NLS, McMaster), **American Battlefield Trust**
  GIS sets, and aerial-recon archives.
- **Real-inspired** (`mode: inspired`, `fidelity: material`) — same ingest, then reshape freely for
  pacing; the real data is raw material, not a 1:1 copy.

Every source carries a licence, tracked per-layer in the manifest ([D46](decisions.md)): **OSM is
ODbL-1.0** (attribution + share-alike); Copernicus/SRTM are effectively public.

---

## The pipeline

Four scripted stages (`pnpm assets:maps` runs ingest+bake; `pnpm maps:lint` lints):

| Stage | Tool | In → out | Notes |
|---|---|---|---|
| 1. Ingest | `ingest.py` | bbox → `*.features.json` + `*.height.f32` | Live OSM/DEM fetch where a GIS stack is installed; otherwise a **deterministic synthetic** extract stamped `synthetic: true` so the pipeline runs offline. |
| 2. Bake | `bake.py` | features → `.covergrid` + `.rs.inc` + manifest | **The load-bearing determinism step.** Pure-stdlib, integer, sorted-order; `--verify` asserts a byte-stable re-bake. Emits balance metrics. |
| 3. Lint | `lint.py` | `.covergrid` → report (+ PNG preview) | Playability checks; exit non-zero on error (CI-able). See [§ Diagnostics](#diagnostics). |
| 4. Mesh | `terrain_mesh.py` | `*.height.f32` → `*.terrain.glb` | Blender heightgrid → decimated glTF. **RENDER-ONLY** — real float elevation, never the sim. |

**Feature → cover** (higher priority wins on overlap): `building`/`wall`/`water` → `Heavy`
(mitigation **and** blocks sight); `hedge`/`scrub`/`forest` → `Light`; otherwise `None`. Water is
`Heavy` today because `Cover` has no *impassable* level — see [Q24](open-questions.md#q24--terrain-traversal-cost).

---

## How it fits the content model (and the interim bridge)

The intended terrain model is already decided:
[D76](decisions.md) (mission/map data via a host-side airlock) + [D77](decisions.md)
(**content-addressed terrain**: a map carries its fixed-point cover grid as data, identified by a
**content-hash** of its canonical bytes; `persist` serializes only the id). The bake pipeline is the
generator that produces that data — `Terrain::from_cover_grid`/`apply_cover_grid` are exactly the
D77 "grid data → `Terrain`" primitive.

**But the code is not yet migrated to D77** — `core::terrain` still uses the `u16` `from_map_id`
registry. So the first baked map is wired the only way the current code allows, an **explicit interim
bridge** ([D80](decisions.md)):

- `Terrain::POINTE_DU_HOC_MAP_ID` + a `from_map_id` arm that `include_str!`s its `.covergrid`;
- `Sim::load_map(id)` sets `map_id` **and** rebuilds `terrain` together, so a reconnect snapshot
  (which carries only `map_id`, [D28](decisions.md)) can't silently rebuild the wrong map
  ([invariant #7](../CLAUDE.md)).

When the D77/D76 content-set loader lands, the `.covergrid` becomes CT-C map content identified by
its content hash, `Terrain::from_content` replaces the hardcoded arm, and the bridge is deleted.
`tools/maps/lint.py` is likewise the interim, real-world analogue of the **CT-F** content-lint and
CT-G PvP-symmetry validator.

---

## Fidelity: faithful, then balance-passed

The project stance ([D80](decisions.md)): **import faithfully, then hand-tune for fairness.** A
faithful real valley can be a one-sided killbox, and a competitive/PvE map must be fair
([invariant #6](../CLAUDE.md)) — so realism is the *starting material* and fairness is the *veto*.

To make that pass data-driven, `bake.py` writes **balance metrics** into the manifest: overall cover
density, per-quadrant density, and **left/right + top/bottom asymmetry**. A high asymmetry flags a
hand-tune target — e.g. Pointe du Hoc's sea/cliff edge shows up immediately as a large T/B
asymmetry (a real feature that is unfair as a symmetric start).

---

## Diagnostics

Map bugs are found **two** ways — a live in-engine overlay **and** a headless harness (the standing
rule for debug scenes):

### Headless — `tools/maps/lint.py`

Reads the `.covergrid` (no engine build needed) and reports:

- **reachability** (ERROR) — passable cells walled off from the main region (units stuck /
  objectives unreachable); Heavy = impassable, 4-connected flood fill;
- **sealed pockets** (WARN) — count of disconnected passable regions;
- **spawn validity** (ERROR) — `--spawn cx,cy` must be passable **and** reachable;
- **wall specks** (WARN) — isolated single-cell walls (often ingest noise);
- **symmetry** (info) — left/right mirror mismatch, for the balance pass;
- **structures** (info) — connected `Heavy` blobs enumerated as objects with **bbox + centroid in
  cell coordinates**, so a bug report can say *"building at (26,32)-(34,38)"*;
- **PNG preview** (`--preview`) — a labelled image with coordinate gridlines every 16 cells.

Exit code is non-zero on any ERROR, so `pnpm maps:lint` gates CI. Example — it catches a spawn
buried in a wall:

```
$ python3 tools/maps/lint.py pointe-du-hoc --spawn 20,20 --spawn 100,100
  ERROR  spawn (20,20) is inside a wall (Heavy)
  info   spawn (100,100) ok ✓
  info   6 structure(s) (Heavy blobs ≥4 cells) — for bug reports:
  info     #1   88 cells  bbox cell (36,75)-(46,82)  centroid (41, 79)
```

### In-engine — the cover overlay + `MapInspect` scene

- `render::debug::covergrid_lines(&Terrain)` outlines every non-open cover cell as a world-space
  square (Light = amber, Heavy = steel), drawn under the **F3** debug overlay. This makes the sim's
  **actual** cover grid — the cells the flow field and line-of-sight read — visible over the field,
  so a wall a cell off, a sealed pocket, or water where it shouldn't be jumps out.
- `Scene::MapInspect` (`app --scene map`) loads a baked map with the cover overlay **on by default**
  and a few troops (plus HoldFire enemies to draw Player→Enemy line-of-sight connectors against the
  real walls). It is the "run a map in debug mode and check things" sandbox.
- `viz-runner` renders it headlessly to `target/viz/map_inspect.png` and asserts the overlay draws
  (a frame diff on toggling F3), so the visual path is CI-covered too.

### Verifying destructibility (when it lands)

Destructible buildings are **not** built yet ([Q25](open-questions.md#q25--destructible-terrain)) —
and can't be slipped in, because destructible terrain is *mutable per-tick state* that **must** enter
the checksum ([invariant #7](../CLAUDE.md)). The current lean is to destroy **entity cover-props**
([D50](decisions.md)), which are already in the ECS/checksum, rather than mutate the grid. When it
exists, the `MapInspect` scene + cover overlay are the place to verify it: watch a prop's cover cell
clear when it's destroyed, with the headless linter re-run on any post-destruction grid state.

---

## Known gaps

Deliberately deferred to open questions — none block a map shipping today:

- **Sim elevation** ([Q23](open-questions.md#q23--sim-elevation)) — the sim is flat; real height
  feeds only the render mesh. A fixed-point height layer (high-ground LoS, slope cost) is a new
  decision.
- **Impassability / traversal cost** ([Q24](open-questions.md#q24--terrain-traversal-cost)) —
  water/cliffs are `Cover::Heavy` (a wall) until the flow field gains a per-cell entry-cost layer.
- **Destructible terrain** ([Q25](open-questions.md#q25--destructible-terrain)) — terrain is static;
  destruction is entity-prop-first, grid-mutation deferred.
- **Live GIS fetch** — `ingest.py`'s real path is stubbed (no `osmnx`/`rasterio` on the dev box); it
  writes a deterministic synthetic extract until a GIS stack is installed.
- **Enterable structures** — buildings are flat `Heavy` cells for the RTS layer; room-scale FPS
  interiors are a separate structure-mesh pass.

---

## See also

- [`tools/maps/README.md`](../tools/maps/README.md) — how to run each stage.
- [`content-pipeline.md`](content-pipeline.md) — the scripted-asset rules ([D41](decisions.md)/[D46](decisions.md)).
- [`plans/content-tooling-plan.md`](plans/content-tooling-plan.md) — CT-C/CT-F/CT-G, the content model this feeds.
- [D80](decisions.md) (the pipeline), [D76](decisions.md)/[D77](decisions.md) (the target terrain model),
  [D28](decisions.md) (snapshot/terrain-by-id), [D50](decisions.md) (cover props).
