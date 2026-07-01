#!/usr/bin/env python3
"""Bake a battlefield's vector features into a deterministic sim cover grid — stage 2.

Script-not-binary (decisions.md D41/D46): this generator + the git-diffable `.covergrid` +
the manifest entry are the committed record. No opaque binary blob.

This is the LOAD-BEARING determinism step. Real GIS data is floating-point (lon/lat, metres);
if any of that leaked into the sim it would desync lockstep silently (invariant #1). So the
bake runs offline in plain Python (floats fine HERE), and its OUTPUT is an integer, byte-stable
cover grid that maps exactly onto the existing `core::terrain::Terrain` model:

    128x128 cells (== core::flow_field::GRID), one Cover level per cell:
        '.' = Cover::None    open ground
        'o' = Cover::Light   hedges / scrub / forest (partial mitigation, sight passes)
        '#' = Cover::Heavy   buildings / walls / water edge (mitigation AND blocks line of sight)

Feature kind → cover, by priority (higher wins on overlap):
    building, wall, water  → Heavy  (2)
    hedge, scrub, forest   → Light  (1)
    (nothing)              → None   (0)

  NOTE on water: core::terrain has only None/Light/Heavy — no "impassable". Water becomes
  Heavy today (a wall: blocks movement paths against it and blocks sight). True impassability
  belongs in the flow field as a raised entry cost; that is a future decision, flagged in the
  manifest and the docs open question. Not silently deciding it here.

Determinism guarantees (so both lockstep peers rebuild a bit-identical map — invariant #7):
  * features processed in a fixed sorted order (by id);
  * integer cell math only, cover priority is max() so overlap order can't matter;
  * emitting the same features.json twice yields the same `.covergrid` sha256 (`--verify`).

Elevation is intentionally IGNORED here: the sim has no height layer yet. Height feeds the
render mesh (terrain_mesh.py), never the cover grid.

Usage:
    python3 tools/maps/bake.py pointe-du-hoc
    python3 tools/maps/bake.py --all
    python3 tools/maps/bake.py pointe-du-hoc --verify   # re-bake, assert stable sha256

Output (all committed):
    assets/maps/<name>.covergrid   git-diffable 128-line ASCII map
    assets/maps/<name>.rs.inc      generated Rust builder (ready to wire into from_map_id)
    assets/maps/manifest.json      provenance + balance metrics (append/update this map's entry)
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CONFIG_DIR = Path(__file__).resolve().parent / "config"
OUT_DIR = REPO / "assets" / "maps"
GRID = 128  # == core::flow_field::GRID

NONE, LIGHT, HEAVY = 0, 1, 2
GLYPH = {NONE: ".", LIGHT: "o", HEAVY: "#"}
KIND_COVER = {
    "building": HEAVY,
    "wall": HEAVY,
    "water": HEAVY,
    "hedge": LIGHT,
    "scrub": LIGHT,
    "forest": LIGHT,
}


def sha256_bytes(b):
    return hashlib.sha256(b).hexdigest()


def to_cell(nx, ny):
    """Bbox-relative [0,1] → clamped cell (cx, cy). Mirrors core's shifted-floor-then-clamp."""
    cx = int(nx * GRID)
    cy = int(ny * GRID)
    return max(0, min(GRID - 1, cx)), max(0, min(GRID - 1, cy))


def point_in_poly(px, py, ring):
    """Even-odd ray cast. Deterministic (no float tolerance games needed for a raster bake)."""
    inside = False
    n = len(ring)
    j = n - 1
    for i in range(n):
        xi, yi = ring[i]
        xj, yj = ring[j]
        if (yi > py) != (yj > py):
            xcross = xi + (py - yi) * (xj - xi) / (yj - yi)
            if px < xcross:
                inside = not inside
        j = i
    return inside


def raster_polygon(grid, ring, cover):
    """Fill every cell whose CENTRE lies inside the polygon."""
    xs = [p[0] for p in ring]
    ys = [p[1] for p in ring]
    cx0, cy0 = to_cell(min(xs), min(ys))
    cx1, cy1 = to_cell(max(xs), max(ys))
    for cy in range(cy0, cy1 + 1):
        py = (cy + 0.5) / GRID
        for cx in range(cx0, cx1 + 1):
            px = (cx + 0.5) / GRID
            if point_in_poly(px, py, ring):
                idx = cy * GRID + cx
                grid[idx] = max(grid[idx], cover)


def raster_line(grid, pts, cover):
    """Walk each segment across cells (integer supercover-ish DDA) and mark them."""
    for a, b in zip(pts, pts[1:]):
        x0, y0 = to_cell(a[0], a[1])
        x1, y1 = to_cell(b[0], b[1])
        dx = abs(x1 - x0)
        dy = abs(y1 - y0)
        sx = 1 if x1 > x0 else -1
        sy = 1 if y1 > y0 else -1
        err = dx - dy
        x, y = x0, y0
        while True:
            idx = y * GRID + x
            grid[idx] = max(grid[idx], cover)
            if x == x1 and y == y1:
                break
            e2 = 2 * err
            if e2 > -dy:
                err -= dy
                x += sx
            if e2 < dx:
                err += dx
                y += sy


def bake_grid(features):
    grid = [NONE] * (GRID * GRID)
    for feat in sorted(features, key=lambda f: f["id"]):  # fixed order → deterministic
        cover = KIND_COVER.get(feat["kind"])
        if cover is None:
            continue
        if feat["geom"] == "polygon":
            raster_polygon(grid, feat["coords"], cover)
        elif feat["geom"] == "line":
            raster_line(grid, feat["coords"], cover)
    return grid


def grid_to_text(grid):
    """Row-major, one line per grid row (cy), origin bottom-left → print top row (high cy) first
    so the ASCII map reads north-up like a map. Trailing newline."""
    lines = []
    for cy in range(GRID - 1, -1, -1):
        lines.append("".join(GLYPH[grid[cy * GRID + cx]] for cx in range(GRID)))
    return "\n".join(lines) + "\n"


def balance_metrics(grid):
    """Data for the 'faithful, then balance-pass' workflow: where is cover concentrated, and how
    symmetric is the field? A human tuner reads this to decide where the real terrain is unfair."""
    total = GRID * GRID
    covered = sum(1 for c in grid if c != NONE)

    def quad_density(qx, qy):
        n = cov = 0
        for cy in range(qy * GRID // 2, (qy + 1) * GRID // 2):
            for cx in range(qx * GRID // 2, (qx + 1) * GRID // 2):
                n += 1
                if grid[cy * GRID + cx] != NONE:
                    cov += 1
        return round(cov / n, 4)

    quads = {
        "sw": quad_density(0, 0),
        "se": quad_density(1, 0),
        "nw": quad_density(0, 1),
        "ne": quad_density(1, 1),
    }
    left = (quads["sw"] + quads["nw"]) / 2
    right = (quads["se"] + quads["ne"]) / 2
    bottom = (quads["sw"] + quads["se"]) / 2
    top = (quads["nw"] + quads["ne"]) / 2
    return {
        "cover_density": round(covered / total, 4),
        "quadrant_density": quads,
        "lr_asymmetry": round(abs(left - right), 4),
        "tb_asymmetry": round(abs(top - bottom), 4),
        "note": "asymmetry near 0 = balanced start positions; large values flag a hand-tune target",
    }


RUST_TEMPLATE = '''\
// @generated by tools/maps/bake.py — DO NOT EDIT.
// Regenerate: python3 tools/maps/bake.py {name}
//
// Ready-to-wire builder for the "{title}" map (map_id {map_id}).
// To integrate (a /safe-edit change to core::terrain — sim data, high blast radius):
//   1. add `apply_cover_grid` to core::terrain (see tools/maps/README.md — ~12 lines);
//   2. add a `from_map_id` arm: `{map_id} => Some(build_{ident}()),`
//   3. ship the determinism test asserting build == rebuild (README has it).
pub fn build_{ident}() -> Terrain {{
    let mut t = Terrain::open();
    apply_cover_grid(&mut t, include_str!("../../assets/maps/{name}.covergrid"));
    t
}}
'''


def emit_rust(cfg):
    ident = cfg["name"].replace("-", "_")
    return RUST_TEMPLATE.format(
        name=cfg["name"], title=cfg["title"], map_id=cfg["map_id"], ident=ident
    )


def load_manifest():
    path = OUT_DIR / "manifest.json"
    if path.exists():
        return json.loads(path.read_text())
    return {
        "note": "Real-world battlefield maps, baked from GIS data. Regenerate with "
        "`python3 tools/maps/ingest.py <name> && python3 tools/maps/bake.py <name>`. "
        "The cover grid feeds core::terrain (invariant #1: integer, checksum-stable). "
        "Elevation is render-only pending the sim-elevation decision.",
        "grid": GRID,
        "cell_world_units": 1,
        "maps": [],
    }


def build(cfg, verify=False):
    name = cfg["name"]
    feats_path = OUT_DIR / f"{name}.features.json"
    if not feats_path.exists():
        sys.exit(f"[bake] missing {feats_path} — run `python3 tools/maps/ingest.py {name}` first")
    fdoc = json.loads(feats_path.read_text())
    features = fdoc["features"]

    grid = bake_grid(features)
    text = grid_to_text(grid)
    text_bytes = text.encode()

    if verify:
        again = grid_to_text(bake_grid(features)).encode()
        if sha256_bytes(again) != sha256_bytes(text_bytes):
            sys.exit(f"[bake] NON-DETERMINISTIC: {name} re-bake produced a different grid")
        print(f"[bake] {name}: deterministic ✓ (sha256 {sha256_bytes(text_bytes)[:16]})")

    cover_path = OUT_DIR / f"{name}.covergrid"
    cover_path.write_text(text)
    rs = emit_rust(cfg)
    rs_path = OUT_DIR / f"{name}.rs.inc"
    rs_path.write_text(rs)

    counts = {"none": grid.count(NONE), "light": grid.count(LIGHT), "heavy": grid.count(HEAVY)}

    manifest = load_manifest()
    entry = {
        "name": name,
        "title": cfg["title"],
        "map_id": cfg["map_id"],
        "mode": cfg["mode"],
        "fidelity": cfg["fidelity"],
        "era": cfg.get("era"),
        "bbox": cfg["bbox"],
        "grid": GRID,
        "cell_world_units": 1,
        "synthetic_source": fdoc.get("synthetic", False),
        "sources": cfg.get("sources", {}),
        "features_file": feats_path.name,
        "features_sha256": sha256_bytes(feats_path.read_bytes()),
        "covergrid_file": cover_path.name,
        "covergrid_sha256": sha256_bytes(text_bytes),
        "rust_include": rs_path.name,
        "rust_sha256": sha256_bytes(rs.encode()),
        "cover_cells": counts,
        "balance": balance_metrics(grid),
        "elevation": "render-only (height.f32); sim has no elevation layer — see docs open question",
        "water_handling": "mapped to Cover::Heavy; true impassability pending flow-field decision",
        "license": "features: see sources (OSM = ODbL-1.0); baked grid: derived data",
        "generator": "tools/maps/bake.py",
    }
    maps = [m for m in manifest["maps"] if m["name"] != name]
    maps.append(entry)
    manifest["maps"] = sorted(maps, key=lambda m: m["map_id"])
    (OUT_DIR / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

    b = entry["balance"]
    print(f"[bake] {name}: {counts}  density {b['cover_density']}  "
          f"asym L/R {b['lr_asymmetry']} T/B {b['tb_asymmetry']}")
    print(f"[bake] {name}: → {cover_path.relative_to(REPO)}  (sha256 {entry['covergrid_sha256'][:16]})")


def load_cfg(name):
    return json.loads((CONFIG_DIR / f"{name}.json").read_text())


def main():
    ap = argparse.ArgumentParser(description="Bake a battlefield cover grid from ingested features.")
    ap.add_argument("name", nargs="?")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--verify", action="store_true", help="re-bake and assert stable sha256")
    args = ap.parse_args()

    if args.all:
        cfgs = [load_cfg(p.stem) for p in sorted(CONFIG_DIR.glob("*.json"))]
    elif args.name:
        cfgs = [load_cfg(args.name)]
    else:
        ap.error("give a config name or --all")

    for cfg in cfgs:
        build(cfg, verify=args.verify)


if __name__ == "__main__":
    main()
