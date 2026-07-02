#!/usr/bin/env python3
"""Procedurally generate a battlefield cover grid from a seed — the real-world CT-G sibling.

Script-not-binary (decisions.md D41/D46): the committed record is this generator + the
git-diffable `.covergrid` + a `.meta.json` sidecar. No opaque binary blob. Where `bake.py`
turns *real GIS data* into a `core::terrain`-compatible cover grid, this turns a *seed +
parameters* into one — the volume multiplier called for by CT-G in
docs/plans/content-tooling-plan.md.

OFFLINE TOOLING, NOT SIM CODE (CLAUDE.md invariant #1/#2, CT-G guardrail):
  * This never imports or touches `core`/the sim. It has its OWN seeded RNG
    (`random.Random(seed)`) — never the global `random` module state, never wall-clock,
    never unseeded entropy. Same seed + same params → BYTE-IDENTICAL output.
  * Its output is an integer, byte-stable cover grid in the SAME format `bake.py` emits, so
    generated maps lint with the existing `tools/maps/lint.py` unchanged.

Output format (identical to bake.py):
    GRID x GRID cells, one Cover level per cell, one line per grid row, north (high cy) first:
        '.' = Cover::None    open ground
        'o' = Cover::Light   hedges / scrub
        '#' = Cover::Heavy   buildings / walls (blocks movement AND line of sight)
    Trailing newline. (`core::terrain::apply_cover_grid` decodes it; see README.md.)

PvP symmetry (the CT-G fairness gate — CLAUDE.md invariant #6, the symmetric-PvP shape):
  For a symmetric mode the whole field is placed on a CANONICAL half and reflected, so cover,
  spawns and control points are EXACTLY symmetric under the declared transform. That makes
  `lint.py --pvp mirror-x|mirror-y|point` pass by construction — neither side gets a
  structural edge. GRID is even, so no cell is its own mirror image (every cell pairs cleanly).

Determinism guarantees (so a regenerate is bit-identical — the sim's discipline, applied here):
  * a single `random.Random(seed)`; every draw comes from it, in a fixed code order;
  * integer cell math only; components/paths walk cells in a fixed neighbor order;
  * connectivity repair carves in a deterministic order and mirrors each carve, so a symmetric
    map stays symmetric AND stays one connected region.

Usage:
    python3 tools/maps/generate.py --seed 1234 --symmetry mirror-x
    python3 tools/maps/generate.py --seed 7 --symmetry point --density 0.2 --spawns 2 --controls 4
    python3 tools/maps/generate.py --seed 7 --symmetry point --verify   # generate twice, assert identical
    python3 tools/maps/generate.py --batch                              # the standing verification set

Output (all committed, git-diffable, under assets/maps/generated/ so lint.py finds them):
    assets/maps/generated/<name>.covergrid   128-line ASCII cover grid (bake.py format)
    assets/maps/generated/<name>.meta.json   seed/params + spawn/control placement + sha256 + lint cmd

Lint a generated map with the existing tool (note the 'generated/' name prefix):
    python3 tools/maps/lint.py generated/<name> --spawn <cx,cy> ... --control <cx,cy> ... --pvp <mode>
"""

import argparse
import hashlib
import json
import random
from collections import deque
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
OUT_DIR = REPO / "assets" / "maps" / "generated"
GRID = 128  # == core::flow_field::GRID; lint.py requires exactly this, so it is fixed.

NONE, LIGHT, HEAVY = 0, 1, 2
GLYPH = {NONE: ".", LIGHT: "o", HEAVY: "#"}
SYMMETRIES = ("none", "mirror-x", "mirror-y", "point")
NEIGHBORS = ((1, 0), (-1, 0), (0, 1), (0, -1))  # fixed order → deterministic flood/BFS


def sha256_bytes(b):
    return hashlib.sha256(b).hexdigest()


def idx(cx, cy):
    return cy * GRID + cx


# --- symmetry --------------------------------------------------------------------------------
# Cell coords (cx, cy), cy=0 at the south edge — matching lint.py's --spawn/--control contract.
# These transforms are the cell-space equivalents of lint.py's mirror_cell().


def mirror(cx, cy, mode):
    if mode == "mirror-x":
        return (GRID - 1 - cx, cy)          # east-west flip
    if mode == "mirror-y":
        return (cx, GRID - 1 - cy)          # north-south flip
    if mode == "point":
        return (GRID - 1 - cx, GRID - 1 - cy)  # 180° rotation
    raise ValueError(f"unknown symmetry mode {mode!r}")


def canon_bounds(mode):
    """Inclusive (xmax, ymax) of the CANONICAL region we place into before reflecting.
    Everything outside is filled by the mirror, guaranteeing exact symmetry."""
    if mode == "mirror-x" or mode == "point":
        return (GRID // 2 - 1, GRID - 1)     # left half
    if mode == "mirror-y":
        return (GRID - 1, GRID // 2 - 1)     # south half
    return (GRID - 1, GRID - 1)              # 'none': the whole grid


def is_canonical(cx, cy, mode):
    xmax, ymax = canon_bounds(mode)
    return cx <= xmax and cy <= ymax


def symmetrize(grid, mode):
    """Copy every canonical cell's cover onto its mirror image → exact structural symmetry."""
    if mode == "none":
        return
    for cy in range(GRID):
        for cx in range(GRID):
            if is_canonical(cx, cy, mode):
                mx, my = mirror(cx, cy, mode)
                grid[idx(mx, my)] = grid[idx(cx, cy)]


# --- cover placement -------------------------------------------------------------------------


def place_rect(grid, cx0, cy0, w, h, cover):
    """Fill a rectangle, cover-priority max() (as bake.py). Returns newly-covered cell count."""
    added = 0
    for cy in range(cy0, min(cy0 + h, GRID)):
        for cx in range(cx0, min(cx0 + w, GRID)):
            i = idx(cx, cy)
            if grid[i] == NONE and cover != NONE:
                added += 1
            grid[i] = max(grid[i], cover)
    return added


def generate_cover(grid, rng, density, mode):
    """Scatter Heavy buildings and Light hedgerows across the canonical region until the target
    cover density is reached, then reflect. Small convex blobs keep the open field connected."""
    xmax, ymax = canon_bounds(mode)
    canon_area = (xmax + 1) * (ymax + 1)
    target = int(density * canon_area)
    covered = 0
    # Cap attempts so a pathological density request can't spin forever (deterministic bound).
    for _ in range(20000):
        if covered >= target:
            break
        if rng.random() < 0.45:
            # Heavy building: compact block.
            cover = HEAVY
            w = rng.randint(2, 6)
            h = rng.randint(2, 6)
        else:
            # Light hedgerow/scrub: long and thin, either orientation.
            cover = LIGHT
            if rng.random() < 0.5:
                w, h = rng.randint(4, 12), rng.randint(1, 2)
            else:
                w, h = rng.randint(1, 2), rng.randint(4, 12)
        cx0 = rng.randint(0, max(0, xmax - w + 1))
        cy0 = rng.randint(0, max(0, ymax - h + 1))
        covered += place_rect(grid, cx0, cy0, w, h, cover)
    symmetrize(grid, mode)


# --- spawn / control placement ---------------------------------------------------------------


def _far_enough(cx, cy, chosen, min_dist):
    for (ox, oy) in chosen:
        if abs(cx - ox) <= min_dist and abs(cy - oy) <= min_dist:
            return False
    return True


def place_points(grid, rng, count, mode, min_dist, margin, canon_x_bias):
    """Place `count` points. For symmetric modes points come in mirror pairs (count rounded UP to
    even), so each has a declared peer under the transform → lint.py --pvp passes. Returns the full
    list (canonical + mirrors) as [(cx, cy), ...]. Points are spaced by `min_dist` (Chebyshev)."""
    xmax, ymax = canon_bounds(mode)
    if mode != "none":
        pairs = (count + 1) // 2
        canon = []
        for _ in range(pairs):
            for _try in range(4000):
                # Bias canonical points toward the interior of their half and away from the seam,
                # so a spawn and its mirror don't crowd the mirror line.
                cx = rng.randint(margin, max(margin, int(xmax * canon_x_bias)))
                cy = rng.randint(margin, max(margin, ymax - margin))
                m = mirror(cx, cy, mode)
                if _far_enough(cx, cy, canon, min_dist) and (cx, cy) != m:
                    canon.append((cx, cy))
                    break
        pts = []
        for (cx, cy) in canon:
            pts.append((cx, cy))
            pts.append(mirror(cx, cy, mode))
        return pts
    # 'none': free placement anywhere, just spaced.
    pts = []
    for _ in range(count):
        for _try in range(4000):
            cx = rng.randint(margin, GRID - 1 - margin)
            cy = rng.randint(margin, GRID - 1 - margin)
            if _far_enough(cx, cy, pts, min_dist):
                pts.append((cx, cy))
                break
    return pts


def clear_around(grid, points, radius):
    """Set a Chebyshev-radius disc around each point to open. For symmetric maps the point list is
    itself symmetric and the radius uniform, so the cleared set is symmetric — symmetry preserved."""
    for (cx, cy) in points:
        for dy in range(-radius, radius + 1):
            for dx in range(-radius, radius + 1):
                nx, ny = cx + dx, cy + dy
                if 0 <= nx < GRID and 0 <= ny < GRID:
                    grid[idx(nx, ny)] = NONE


# --- connectivity repair ---------------------------------------------------------------------


def passable_components(grid):
    """4-connected components of passable cells (Heavy = impassable), in row-major discovery order.
    Matches lint.py's passability model exactly."""
    seen = [False] * (GRID * GRID)
    comps = []
    for cy in range(GRID):
        for cx in range(GRID):
            i = idx(cx, cy)
            if seen[i] or grid[i] == HEAVY:
                continue
            comp = []
            q = deque([(cx, cy)])
            seen[i] = True
            while q:
                x, y = q.popleft()
                comp.append((x, y))
                for dx, dy in NEIGHBORS:
                    nx, ny = x + dx, y + dy
                    if 0 <= nx < GRID and 0 <= ny < GRID:
                        j = idx(nx, ny)
                        if not seen[j] and grid[j] != HEAVY:
                            seen[j] = True
                            q.append((nx, ny))
            comps.append(comp)
    return comps


def _path_to_main(stranded, main_set):
    """BFS over ALL cells (ignoring cover) from every stranded cell until a main-region cell is
    reached; return the cell path. Deterministic: fixed source order + fixed neighbor order."""
    parent = {}
    q = deque()
    for cell in stranded:
        parent[cell] = None
        q.append(cell)
    src = set(stranded)
    while q:
        cur = q.popleft()
        if cur in main_set:
            path = [cur]
            while parent[path[-1]] is not None:
                path.append(parent[path[-1]])
            return path
        cx, cy = cur
        for dx, dy in NEIGHBORS:
            nx, ny = cx + dx, cy + dy
            nxt = (nx, ny)
            if 0 <= nx < GRID and 0 <= ny < GRID and nxt not in parent and nxt not in src:
                parent[nxt] = cur
                q.append(nxt)
    return None


def repair_connectivity(grid, mode):
    """Carve the map down to a single passable region. Each carved Heavy cell (and, for symmetric
    modes, its mirror) becomes open — so a symmetric map stays symmetric and stays connected.
    Monotonic (only adds passable cells) → guaranteed to terminate."""
    for _ in range(GRID * GRID):  # hard bound; converges far sooner
        comps = passable_components(grid)
        if len(comps) <= 1:
            return
        comps.sort(key=len, reverse=True)  # stable → row-major tie-break, deterministic
        main_set = set(comps[0])
        path = _path_to_main(comps[1], main_set)
        if path is None:
            return
        for (cx, cy) in path:
            grid[idx(cx, cy)] = NONE
            if mode != "none":
                mx, my = mirror(cx, cy, mode)
                grid[idx(mx, my)] = NONE


# --- emit ------------------------------------------------------------------------------------


def grid_to_text(grid):
    """Row-major, north (high cy) first, trailing newline — byte-identical to bake.py."""
    lines = []
    for cy in range(GRID - 1, -1, -1):
        lines.append("".join(GLYPH[grid[idx(cx, cy)]] for cx in range(GRID)))
    return "\n".join(lines) + "\n"


def default_name(seed, symmetry):
    return f"gen-{symmetry}-{seed}"


def generate(seed, symmetry, density, spawns, controls, name=None):
    """Build the full grid + placement metadata for one map. Pure function of the inputs."""
    if symmetry not in SYMMETRIES:
        raise ValueError(f"symmetry must be one of {SYMMETRIES}")
    rng = random.Random(seed)  # OUR OWN rng — never the global module state.
    name = name or default_name(seed, symmetry)

    grid = [NONE] * (GRID * GRID)
    generate_cover(grid, rng, density, symmetry)
    spawn_pts = place_points(grid, rng, spawns, symmetry, min_dist=24, margin=6, canon_x_bias=0.7)
    control_pts = place_points(grid, rng, controls, symmetry, min_dist=14, margin=10, canon_x_bias=0.9)
    clear_around(grid, spawn_pts, radius=3)
    clear_around(grid, control_pts, radius=2)
    repair_connectivity(grid, symmetry)

    text = grid_to_text(grid)
    counts = {"none": grid.count(NONE), "light": grid.count(LIGHT), "heavy": grid.count(HEAVY)}
    meta = {
        "name": name,
        "generator": "tools/maps/generate.py",
        "seed": seed,
        "symmetry": symmetry,
        "params": {"density": density, "spawns": spawns, "controls": controls, "grid": GRID},
        "spawns": [list(p) for p in spawn_pts],
        "controls": [list(p) for p in control_pts],
        "cover_cells": counts,
        "cover_density": round((counts["light"] + counts["heavy"]) / (GRID * GRID), 4),
        "covergrid_sha256": sha256_bytes(text.encode()),
        "note": "Seed-deterministic procedural map (CT-G). Offline tooling — never touches the sim. "
                "Same seed+params → byte-identical .covergrid.",
        "lint_cmd": _lint_cmd(name, spawn_pts, control_pts, symmetry),
    }
    return text, meta


def _lint_cmd(name, spawns, controls, symmetry):
    parts = ["python3 tools/maps/lint.py", f"generated/{name}"]
    for (cx, cy) in spawns:
        parts.append(f"--spawn {cx},{cy}")
    for (cx, cy) in controls:
        parts.append(f"--control {cx},{cy}")
    if symmetry != "none":
        parts.append(f"--pvp {symmetry}")
    return " ".join(parts)


def write_map(text, meta):
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    cover_path = OUT_DIR / f"{meta['name']}.covergrid"
    meta_path = OUT_DIR / f"{meta['name']}.meta.json"
    cover_path.write_text(text)
    meta_path.write_text(json.dumps(meta, indent=2) + "\n")
    print(f"[generate] {meta['name']}: {meta['cover_cells']}  density {meta['cover_density']}  "
          f"spawns {len(meta['spawns'])} controls {len(meta['controls'])}")
    print(f"[generate] {meta['name']}: → {cover_path.relative_to(REPO)}  "
          f"(sha256 {meta['covergrid_sha256'][:16]})")
    print(f"[generate] lint: {meta['lint_cmd']}")
    return cover_path, meta_path


# The standing verification set: a spread of seeds across every symmetry mode. Each symmetric map
# must pass `lint.py --pvp <mode>`; every map must lint without an ERROR.
BATCH = [
    dict(seed=1001, symmetry="mirror-x", density=0.16, spawns=2, controls=2),
    dict(seed=1002, symmetry="mirror-y", density=0.18, spawns=2, controls=4),
    dict(seed=1003, symmetry="point", density=0.20, spawns=2, controls=2),
    dict(seed=1004, symmetry="point", density=0.14, spawns=4, controls=4),
    dict(seed=1005, symmetry="mirror-x", density=0.22, spawns=4, controls=2),
    dict(seed=2024, symmetry="none", density=0.18, spawns=2, controls=1),
]


def main():
    ap = argparse.ArgumentParser(description="Seed-deterministic procedural battlefield generator (CT-G).")
    ap.add_argument("--seed", type=int, help="RNG seed (same seed+params → identical bytes)")
    ap.add_argument("--symmetry", choices=SYMMETRIES, default="mirror-x")
    ap.add_argument("--density", type=float, default=0.18, help="target cover fraction (0..1)")
    ap.add_argument("--spawns", type=int, default=2, help="spawn count (rounded up to even for symmetric modes)")
    ap.add_argument("--controls", type=int, default=2, help="control-point count (rounded up to even for symmetric modes)")
    ap.add_argument("--name", help="override output basename (default gen-<symmetry>-<seed>)")
    ap.add_argument("--batch", action="store_true", help="generate the standing verification set")
    ap.add_argument("--verify", action="store_true", help="generate twice and assert byte-identical output")
    args = ap.parse_args()

    if args.batch:
        for spec in BATCH:
            text, meta = generate(**spec)
            write_map(text, meta)
        return

    if args.seed is None:
        ap.error("give --seed (or --batch)")

    text, meta = generate(args.seed, args.symmetry, args.density, args.spawns, args.controls, args.name)
    if args.verify:
        text2, meta2 = generate(args.seed, args.symmetry, args.density, args.spawns, args.controls, args.name)
        if text != text2 or meta2["covergrid_sha256"] != meta["covergrid_sha256"]:
            raise SystemExit(f"[generate] NON-DETERMINISTIC: {meta['name']} re-generate differed")
        print(f"[generate] {meta['name']}: deterministic ✓ (sha256 {meta['covergrid_sha256'][:16]})")
    write_map(text, meta)


if __name__ == "__main__":
    main()
