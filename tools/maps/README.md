# Real-world battlefield maps — pipeline spike

Turn a real place (or a historical battlefield) into a playable Going Dark map: pull the GIS
data, bake it into the deterministic sim grid, and build a render mesh from the elevation.

This is a **spike** — a working proof of the pipeline shape, not the finished feature. It runs
end-to-end today and produces a real, `core::terrain`-compatible map from one bounding box.

```
 config/<name>.json          ingest.py             bake.py                terrain_mesh.py
 ┌──────────────┐   bbox    ┌──────────┐  features ┌──────────┐  grid    ┌──────────────┐
 │ bbox, era,   │ ────────► │ DEM +    │ ────────► │ rasterise│ ───────► │ (Rust builder│
 │ mode,        │           │ OSM      │           │ → cover  │  cover   │  + covergrid) │
 │ fidelity     │           │ fetch    │  height   │  grid    │          │              │
 └──────────────┘           └────┬─────┘           └────┬─────┘          └──────┬───────┘
                                 │ height.f32 (f32 m)    │ covergrid (ASCII)     │ height.f32
                                 ▼  RENDER-ONLY          ▼  SIM (integer)        ▼
                          ┌─────────────────────────────────────────────────────────────┐
                          │  SIM: 128×128 Cover grid → core::terrain (invariant #1, #4)   │
                          │  RENDER: displaced glTF terrain mesh (floats, decoupled)      │
                          └─────────────────────────────────────────────────────────────┘
```

## Run it

```
python3 tools/maps/ingest.py pointe-du-hoc            # → assets/maps/*.features.json, *.height.f32
python3 tools/maps/bake.py   pointe-du-hoc --verify   # → *.covergrid, *.rs.inc, manifest.json
blender --background --python tools/maps/terrain_mesh.py -- pointe-du-hoc   # → *.terrain.glb
```

`--verify` re-bakes and asserts a byte-stable sha256 — the same determinism discipline the sim
lives by. Both lockstep peers must rebuild a bit-identical map (invariant #7).

## The two-artifact split (why this is the whole design)

One source, **two decoupled artifacts** — this is invariant #4 made concrete:

| Artifact | Consumer | Numbers | Detail | In checksum? |
|---|---|---|---|---|
| `.covergrid` | **sim** (`core::terrain`) | **integer** cover levels | coarse, 128×128 | no (static map data, D28) |
| `.terrain.glb` | **render** | floats, real metres | fine, textured | n/a |

Real GIS data is floating-point (lon/lat, metres). If any of it leaked into the sim it would
desync lockstep **silently** (invariant #1 — no floats in the sim, ever). So the bake runs
offline in plain Python (floats are fine *there*) and emits an **integer, byte-stable** cover
grid. The pretty float elevation lives only in the render mesh.

## Where the data comes from

Two pipelines, picked by `mode` in the config (both requested — modern, historical, inspired):

- **Modern real locations** — cleanest. Elevation from **Copernicus DEM GLO-30** / SRTM /
  USGS 3DEP LiDAR; vector features (buildings, roads, walls, hedges, water, landcover) from
  **OpenStreetMap** (Overpass / `osmnx`), with Microsoft/Google building footprints where OSM
  is thin, and ESA WorldCover for vegetation.
- **Historical battlefields** — the terrain has *changed* (WWI cratering, moved rivers, lost
  bocage). Take a modern DEM as the geological base, then overlay period features from
  **georeferenced trench maps** (NLS, McMaster), the **American Battlefield Trust** GIS sets,
  and aerial-recon archives.
- **Real-inspired** — same ingest, then reshape freely for pacing. `fidelity: "material"`.

Every source carries a license (OSM = **ODbL-1.0**, attribution + share-alike; Copernicus/SRTM
effectively public). The manifest records `source`/`license`/`sha256` per layer (D46).

## `fidelity: "faithful"` → balance-pass workflow

You chose *faithful, then balance-pass*. `bake.py` emits **balance metrics** into the manifest —
overall cover density, per-quadrant density, and left/right + top/bottom asymmetry — so a human
tuner sees where the real terrain is one-sided before hand-adjusting. (Pointe du Hoc's sea/cliff
edge shows up immediately as a high **T/B asymmetry** — a real feature that is unfair as a
symmetric start, exactly what the balance pass is for. Fairness wins — invariant #6.)

## Wiring the baked map into `core` (the follow-up — a `/safe-edit` change)

`bake.py` emits a ready-to-wire `<name>.rs.inc`. Landing it touches sim data (high blast
radius), so do it via `/safe-edit` with a determinism test. It needs one ~12-line helper on
`core::terrain::Terrain` that decodes the git-diffable grid:

```rust
/// Rebuild a Terrain from a baked `.covergrid` (rows north-first, '.'=None 'o'=Light '#'=Heavy).
/// Deterministic + integer-only — safe for `from_map_id` (invariant #1).
pub fn apply_cover_grid(t: &mut Terrain, grid: &str) {
    for (row, line) in grid.lines().enumerate() {
        let cy = (GRID - 1 - row) as i32;               // file is north (high cy) first
        for (cx, ch) in line.chars().enumerate() {
            let cover = match ch { '#' => Cover::Heavy, 'o' => Cover::Light, _ => Cover::None };
            t.set_cover(cx as i32, cy, cover);
        }
    }
}
```

Then add a `from_map_id` arm (`1 => Some(build_pointe_du_hoc()),`) and a test asserting
`build == rebuild` and `apply_cover_grid` round-trips the glyphs. The map ships as the **generator
+ git-diffable `.covergrid` + manifest** — no opaque blob (D46).

## Known gaps (deliberately not decided here — see `docs/open-questions.md`)

- **Sim elevation.** `core::terrain` is flat (cover only). Real height feeds the render mesh but
  not the sim. Giving the sim a height layer (LOS over ridges, slope → flow-field cost, high
  ground) is a **new decision** — a fixed-point height per cell, folded into pathing/LOS. Big,
  load-bearing; needs a `Dn`.
- **Impassability.** Water/cliffs map to `Cover::Heavy` today (there is no "impassable" cover
  level). True blocking belongs in the flow field as a raised per-cell entry cost — a Phase-2
  flow-field generalisation.
- **Real GIS fetch.** `ingest.py`'s live path is stubbed (no `osmnx`/`rasterio` on this box); it
  writes a deterministic **synthetic** extract (stamped `synthetic: true`) so the pipeline runs
  offline. Fill in the fetch on a box with the GIS stack.
- **Structures for embodiment.** Buildings are flat `Heavy` cells for the RTS layer; enterable,
  room-scale interiors for the FPS layer are a separate structure-mesh pass.

See `docs/content-pipeline.md` for the asset-pipeline rules this follows (D41/D46).
