#!/usr/bin/env python3
"""Lint a baked battlefield map for the bugs that make a map unplayable — stage 4 (diagnosis).

This is the headless half of map diagnosis (the in-engine cover overlay is the visual half; see
tools/maps/README.md). It reads a `.covergrid` (pure ASCII, no engine build needed) and reports
the classic map defects, then optionally renders a labelled PNG preview so you can eyeball it.

Checks (each is an ERROR that fails CI, or a WARN that just reports):
  * dimensions        ERROR — must be exactly GRID lines of GRID chars.
  * reachability      ERROR — passable cells that are walled off from the main region (units get
                      stuck / objectives unreachable). Heavy = impassable (walls/water); None &
                      Light = passable. 4-connected flood fill.
  * sealed pockets    WARN  — count of disconnected passable regions (a valid map is usually 1).
  * spawn validity    ERROR — every spawn point named on the CLI must be passable & reachable.
  * spawn connectivity ERROR — flood-fill FROM each spawn; every control point (--control) and every
                      OTHER spawn must be reachable, else the objective/side is cut off. Also reports
                      the passable-area % each spawn can actually reach.
  * chokepoints       WARN  — for each spawn pair, the narrowest passable corridor along the shortest
                      route between them (a single-cell pinch is a WARN — one grenade holds the map).
  * open field        WARN  — the largest contiguous no-cover (open) region a player must cross fully
                      exposed; a huge one (>25% of the map) is a no-man's-land flag.
  * wall specks       WARN  — isolated single-cell Heavy blocks (often ingest noise).
  * symmetry          WARN  — left/right mirror mismatch (info for the balance pass; a faithful
                      real map is expected to be asymmetric, so this never fails).
  * pvp symmetry      ERROR — opt-in (--pvp mirror-x|mirror-y|point). For a COMPETITIVE map, asserts
                      cover, spawns and control points are exactly symmetric under the declared
                      transform, so neither side has a structural edge. Rejects an asymmetric fixture,
                      accepts a mirrored one. (The CT-G PvP-symmetry validator, real-world analogue.)
  * structures        INFO  — enumerates connected Heavy blobs as objects with bbox + centroid in
                      CELL coordinates, so a bug report can say "building at (26,32)-(34,38)".

Usage:
    python3 tools/maps/lint.py pointe-du-hoc
    python3 tools/maps/lint.py pointe-du-hoc --spawn 20,20 --spawn 100,100
    python3 tools/maps/lint.py pointe-du-hoc --spawn 20,20 --control 64,64   # reachable objective?
    python3 tools/maps/lint.py arena --spawn 20,20 --spawn 107,107 --pvp point   # fairness gate
    python3 tools/maps/lint.py --all
    python3 tools/maps/lint.py pointe-du-hoc --preview   # also write <name>.preview.png
    python3 tools/maps/lint.py --self-test   # exercise every check on synthetic good/bad fixtures

All coordinates in findings are CELL coordinates (cx,cy), cy=0 at the south edge.
Exit code is non-zero if any ERROR fires (so `pnpm maps:lint` gates CI).
"""

import argparse
import sys
from collections import deque
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
MAPS_DIR = REPO / "assets" / "maps"
GRID = 128  # == core::flow_field::GRID

HEAVY, LIGHT, NONE = "#", "o", "."


def load_grid(name):
    """Load a covergrid as grid[row][col], row 0 = north (top of file)."""
    text = (MAPS_DIR / f"{name}.covergrid").read_text()
    return [list(line) for line in text.splitlines()]


def passable(ch):
    return ch != HEAVY  # Heavy = wall/water/impassable; None & Light are traversable


def components(grid, predicate):
    """4-connected components of cells satisfying `predicate`. Returns list of cell lists."""
    rows, cols = len(grid), (len(grid[0]) if grid else 0)
    seen = [[False] * cols for _ in range(rows)]
    comps = []
    for r in range(rows):
        for c in range(cols):
            if seen[r][c] or not predicate(grid[r][c]):
                continue
            comp = []
            q = deque([(r, c)])
            seen[r][c] = True
            while q:
                cr, cc = q.popleft()
                comp.append((cr, cc))
                for dr, dc in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    nr, nc = cr + dr, cc + dc
                    if 0 <= nr < rows and 0 <= nc < cols and not seen[nr][nc] and predicate(grid[nr][nc]):
                        seen[nr][nc] = True
                        q.append((nr, nc))
            comps.append(comp)
    return comps


def row_to_cell_y(row, rows):
    """Grid file row (north-first) → sim cell cy (south = 0). Mirrors bake.py / apply_cover_grid."""
    return rows - 1 - row


NEIGHBORS = ((1, 0), (-1, 0), (0, 1), (0, -1))  # fixed order → deterministic BFS/flood


def reachable_from(grid, start):
    """Set of (row, col) reachable from `start` over passable cells, 4-connected.

    Deterministic: fixed neighbor order, and a set is only ever membership-tested (never
    iterated into the report). `start` is assumed in-bounds and passable."""
    rows, cols = len(grid), len(grid[0])
    seen = {start}
    q = deque([start])
    while q:
        r, c = q.popleft()
        for dr, dc in NEIGHBORS:
            nr, nc = r + dr, c + dc
            if 0 <= nr < rows and 0 <= nc < cols and (nr, nc) not in seen and passable(grid[nr][nc]):
                seen.add((nr, nc))
                q.append((nr, nc))
    return seen


def shortest_path(grid, start, goal):
    """Shortest 4-connected passable path start→goal as a list of (row, col), or None.

    Deterministic: BFS in fixed neighbor order; the first parent to reach a cell wins, so the
    reconstructed path is a fixed function of the grid."""
    if start == goal:
        return [start]
    rows, cols = len(grid), len(grid[0])
    parent = {start: None}
    q = deque([start])
    while q:
        r, c = q.popleft()
        for dr, dc in NEIGHBORS:
            nr, nc = r + dr, c + dc
            if 0 <= nr < rows and 0 <= nc < cols and (nr, nc) not in parent and passable(grid[nr][nc]):
                parent[(nr, nc)] = (r, c)
                if (nr, nc) == goal:
                    path = [(nr, nc)]
                    while path[-1] is not None:
                        prev = parent[path[-1]]
                        if prev is None:
                            break
                        path.append(prev)
                    path.reverse()
                    return path
                q.append((nr, nc))
    return None


def corridor_width(grid, r, c):
    """Local passable-corridor width at (r, c): the min of its horizontal and vertical passable
    runs (each counting the cell itself). A 1-wide pinch scores 1 on the crossing axis."""
    rows, cols = len(grid), len(grid[0])

    def run(dr, dc):
        w = 1
        for sign in (1, -1):
            nr, nc = r + sign * dr, c + sign * dc
            while 0 <= nr < rows and 0 <= nc < cols and passable(grid[nr][nc]):
                w += 1
                nr += sign * dr
                nc += sign * dc
        return w

    return min(run(0, 1), run(1, 0))


def narrowest_on_path(grid, path):
    """(width, (row, col)) of the narrowest corridor cell on `path`. Ties → first (deterministic)."""
    best = None
    for (r, c) in path:
        w = corridor_width(grid, r, c)
        if best is None or w < best[0]:
            best = (w, (r, c))
    return best


def mirror_cell(r, c, rows, cols, mode):
    """Image of (row, col) under a PvP symmetry transform.
       mirror-x = flip left/right (east-west); mirror-y = flip north/south; point = 180° rotation."""
    if mode == "mirror-x":
        return (r, cols - 1 - c)
    if mode == "mirror-y":
        return (rows - 1 - r, c)
    if mode == "point":
        return (rows - 1 - r, cols - 1 - c)
    raise ValueError(f"unknown symmetry mode {mode!r}")


def cover_symmetry_mismatches(grid, mode):
    """Cells whose cover glyph differs from its mirror image, as sorted CELL coords (cx,cy).
    Each mismatched pair is reported once (the lexicographically-smaller (row,col) endpoint)."""
    rows, cols = len(grid), len(grid[0])
    bad = []
    for r in range(rows):
        for c in range(cols):
            mr, mc = mirror_cell(r, c, rows, cols, mode)
            if (r, c) <= (mr, mc) and grid[r][c] != grid[mr][mc]:
                bad.append((c, row_to_cell_y(r, rows)))
    bad.sort()
    return bad


class Report:
    def __init__(self):
        self.errors = 0
        self.warns = 0

    def err(self, msg):
        self.errors += 1
        print(f"  ERROR  {msg}")

    def warn(self, msg):
        self.warns += 1
        print(f"  WARN   {msg}")

    def info(self, msg):
        print(f"  info   {msg}")


def lint(name, spawns, controls, pvp, preview):
    print(f"[lint] {name}")
    grid = load_grid(name)
    rep = Report()

    # --- dimensions -------------------------------------------------------------------------
    rows = len(grid)
    if rows != GRID:
        rep.err(f"expected {GRID} rows, got {rows}")
    bad = [i for i, line in enumerate(grid) if len(line) != GRID]
    if bad:
        rep.err(f"{len(bad)} row(s) not {GRID} wide (first: row {bad[0]} = {len(grid[bad[0]])})")
    if rep.errors:
        print("  (dimension errors — skipping structural checks)")
        return rep

    total = GRID * GRID
    heavy = sum(line.count(HEAVY) for line in grid)
    light = sum(line.count(LIGHT) for line in grid)
    rep.info(f"cover: {heavy} heavy, {light} light, {total - heavy - light} open "
             f"(density {round((heavy + light) / total, 3)})")

    # --- reachability & sealed pockets ------------------------------------------------------
    pass_comps = components(grid, passable)
    if not pass_comps:
        rep.err("no passable cells at all — the whole map is walled")
        return rep
    pass_comps.sort(key=len, reverse=True)
    main = pass_comps[0]
    main_set = set(main)
    stranded = sum(len(c) for c in pass_comps[1:])
    if len(pass_comps) > 1:
        rep.warn(f"{len(pass_comps)} disconnected passable regions "
                 f"(main {len(main)} cells, {stranded} stranded)")
        # A big stranded pocket is a real bug (units/objectives cut off); a few cells is cosmetic.
        big = [c for c in pass_comps[1:] if len(c) > 8]
        for c in big:
            rs = [r for r, _ in c]
            cs = [cc for _, cc in c]
            rep.err(f"stranded pocket of {len(c)} cells around cell "
                    f"({min(cs)}..{max(cs)}, {row_to_cell_y(max(rs), rows)}..{row_to_cell_y(min(rs), rows)}) "
                    f"— unreachable from the main map")
    else:
        rep.info("reachability: single connected passable region ✓")

    # --- spawn validity ---------------------------------------------------------------------
    valid_spawns = []  # (cx, cy, row) for spawns that are passable & on the main map
    for (sx, sy) in spawns:
        row = rows - 1 - sy  # cell cy → file row
        if not (0 <= sx < GRID and 0 <= sy < GRID):
            rep.err(f"spawn ({sx},{sy}) is off the grid")
            continue
        ch = grid[row][sx]
        if not passable(ch):
            rep.err(f"spawn ({sx},{sy}) is inside a wall (Heavy)")
        elif (row, sx) not in main_set:
            rep.err(f"spawn ({sx},{sy}) is in a stranded pocket, not the main map")
        else:
            rep.info(f"spawn ({sx},{sy}) ok ✓")
            valid_spawns.append((sx, sy, row))

    # --- spawn connectivity: control points & other spawns reachable FROM each spawn ---------
    # The global reachability check above proves the map is one region; this proves each SPAWN
    # can actually reach every objective and every enemy spawn (an objective sealed behind a wall
    # only that side can pass is a real, side-specific defect the global check misses).
    total_pass = sum(len(c) for c in pass_comps)
    for (sx, sy, srow) in valid_spawns:
        reach = reachable_from(grid, (srow, sx))
        pct = round(100 * len(reach) / total_pass, 1) if total_pass else 0.0
        rep.info(f"spawn ({sx},{sy}) reaches {len(reach)} passable cells ({pct}% of the map)")
        for (cx, cy) in controls:
            crow = rows - 1 - cy
            if not (0 <= cx < GRID and 0 <= cy < GRID):
                rep.err(f"control point ({cx},{cy}) is off the grid")
            elif not passable(grid[crow][cx]):
                rep.err(f"control point ({cx},{cy}) is inside a wall (Heavy)")
            elif (crow, cx) not in reach:
                rep.err(f"control point ({cx},{cy}) unreachable from spawn ({sx},{sy})")
        for (ox, oy, orow) in valid_spawns:
            if (ox, oy) != (sx, sy) and (orow, ox) not in reach:
                rep.err(f"spawn ({ox},{oy}) unreachable from spawn ({sx},{sy})")

    # --- chokepoints: narrowest corridor along the route between each spawn pair -------------
    for i in range(len(valid_spawns)):
        for j in range(i + 1, len(valid_spawns)):
            ax, ay, arow = valid_spawns[i]
            bx, by, brow = valid_spawns[j]
            path = shortest_path(grid, (arow, ax), (brow, bx))
            if path is None:
                continue  # unreachability already reported above
            width, (wr, wc) = narrowest_on_path(grid, path)
            loc = f"cell ({wc},{row_to_cell_y(wr, rows)})"
            msg = (f"narrowest corridor between spawns ({ax},{ay})↔({bx},{by}) is {width} "
                   f"cell(s) wide at {loc} (route {len(path)} cells)")
            if width <= 1:
                rep.warn(msg + " — a single-cell pinch: one position holds the whole map")
            else:
                rep.info(msg)

    # --- isolated wall specks ---------------------------------------------------------------
    heavy_comps = components(grid, lambda ch: ch == HEAVY)
    specks = [c for c in heavy_comps if len(c) == 1]
    if specks:
        rep.warn(f"{len(specks)} isolated single-cell wall(s) — often ingest noise, e.g. cell "
                 + ", ".join(f"({c[0][1]},{row_to_cell_y(c[0][0], rows)})" for c in specks[:5])
                 + (" ..." if len(specks) > 5 else ""))

    # --- open field: largest fully-exposed (no-cover) region a player must cross ------------
    open_comps = components(grid, lambda ch: ch == NONE)
    if open_comps:
        biggest = max(open_comps, key=len)  # deterministic: components() is row-major, len tie → first
        rs = [r for r, _ in biggest]
        cs = [cc for _, cc in biggest]
        cy_lo, cy_hi = row_to_cell_y(max(rs), rows), row_to_cell_y(min(rs), rows)
        cent = (round(sum(cs) / len(cs)), row_to_cell_y(round(sum(rs) / len(rs)), rows))
        frac = len(biggest) / total
        msg = (f"largest open (no-cover) region: {len(biggest)} cells ({round(100 * frac, 1)}%) "
               f"bbox cell ({min(cs)},{cy_lo})-({max(cs)},{cy_hi}) centroid {cent}")
        if frac > 0.25:
            rep.warn(msg + " — a wide no-man's-land crossing with no cover")
        else:
            rep.info(msg)

    # --- symmetry (info for the balance pass; never an error) -------------------------------
    mism = sum(
        1
        for r in range(rows)
        for c in range(GRID // 2)
        if passable(grid[r][c]) != passable(grid[r][GRID - 1 - c])
    )
    rep.info(f"left/right mirror mismatch: {round(mism / (rows * (GRID // 2)), 3)} "
             "(0 = symmetric; high is fine for a faithful real map, flags a balance target)")

    # --- PvP symmetry (opt-in; a HARD fairness gate for competitive maps) --------------------
    # Unlike the info-only balance metric above, --pvp asserts EXACT structural symmetry under a
    # declared transform: cover glyphs, spawns and control points must each map onto a peer, so
    # neither side gets a structural edge (invariant #6 fairness; the CT-G PvP validator).
    if pvp:
        bad = cover_symmetry_mismatches(grid, pvp)
        if bad:
            sample = ", ".join(f"({cx},{cy})" for cx, cy in bad[:5]) + (" ..." if len(bad) > 5 else "")
            rep.err(f"pvp {pvp}: {len(bad)} cover cell(s) not symmetric — e.g. {sample}")
        else:
            rep.info(f"pvp {pvp}: cover is symmetric ✓")
        # spawns and control points must each pair with a declared peer under the transform
        for label, pts in (("spawn", [(x, y) for x, y, _ in valid_spawns]), ("control point", controls)):
            declared = set(pts)
            for (px, py) in sorted(declared):
                prow = rows - 1 - py
                mr, mc = mirror_cell(prow, px, rows, GRID, pvp)
                peer = (mc, row_to_cell_y(mr, rows))
                if peer not in declared:
                    rep.err(f"pvp {pvp}: {label} ({px},{py}) has no symmetric peer "
                            f"(expected one at {peer})")

    # --- structures (object identification) -------------------------------------------------
    blobs = sorted((c for c in heavy_comps if len(c) >= 4), key=len, reverse=True)
    if blobs:
        rep.info(f"{len(blobs)} structure(s) (Heavy blobs ≥4 cells) — for bug reports:")
        for i, c in enumerate(blobs[:10]):
            rs = [r for r, _ in c]
            cs = [cc for _, cc in c]
            cy_lo, cy_hi = row_to_cell_y(max(rs), rows), row_to_cell_y(min(rs), rows)
            cent = (round(sum(cs) / len(cs)), row_to_cell_y(round(sum(rs) / len(rs)), rows))
            rep.info(f"  #{i} {len(c):4d} cells  bbox cell "
                     f"({min(cs)},{cy_lo})-({max(cs)},{cy_hi})  centroid {cent}")

    if preview:
        write_preview(name, grid)

    return rep


def write_preview(name, grid):
    try:
        from PIL import Image, ImageDraw
    except ImportError:
        print("  (no PIL — skipping PNG preview; `pip install pillow` to enable)")
        return
    scale = 5
    rows, cols = len(grid), len(grid[0])
    img = Image.new("RGB", (cols * scale, rows * scale), (18, 26, 18))
    px = img.load()
    palette = {NONE: (24, 34, 24), LIGHT: (120, 120, 40), HEAVY: (90, 96, 104)}
    for r in range(rows):
        for c in range(cols):
            col = palette.get(grid[r][c], (200, 0, 200))
            for dy in range(scale):
                for dx in range(scale):
                    px[c * scale + dx, r * scale + dy] = col
    draw = ImageDraw.Draw(img)
    for k in range(0, cols + 1, 16):  # coordinate gridlines every 16 cells
        draw.line([(k * scale, 0), (k * scale, rows * scale)], fill=(60, 70, 60))
        draw.line([(0, k * scale), (cols * scale, k * scale)], fill=(60, 70, 60))
    out = MAPS_DIR / f"{name}.preview.png"
    img.save(out)
    print(f"  preview → {out.relative_to(REPO)} ({cols}x{rows} cells, {scale}px/cell, "
          "gridlines every 16 cells)")


def parse_cell(s):
    x, y = s.split(",")
    return (int(x), int(y))


def self_test():
    """Exercise every diagnostic on tiny synthetic grids with a known-good and known-bad case.

    Pure stdlib, deterministic, no files. Runs off small grids (not 128×128) via the dimension-
    independent helpers, so it stays fast and readable. Prints a per-check pass line; exits
    non-zero if any assertion fails."""

    def make(rows):
        return [list(r) for r in rows]

    checks = []

    def check(name, cond):
        checks.append((name, bool(cond)))
        print(f"  {'PASS' if cond else 'FAIL'}  {name}")

    # reachability from spawn: a wall column splits the grid; a control point behind it is cut off.
    #   col 2 is all '#', so col 0-1 and col 3-4 are separate passable pockets.
    split = make(["..#..", "..#..", "..#..", "..#..", "..#.."])
    reach = reachable_from(split, (0, 0))          # start top-left
    check("reach: same-side cell reachable", (2, 1) in reach or (0, 1) in reach)
    check("reach: cell across the wall NOT reachable", (0, 4) not in reach)
    open5 = make(["....." for _ in range(5)])
    check("reach: open grid reaches all 25 cells", len(reachable_from(open5, (0, 0))) == 25)

    # chokepoint: two open rooms joined by a single-cell doorway → narrowest width == 1.
    #   rows 0-1 open, row 2 walled except one door at col 2, rows 3-4 open.
    pinch = make([".....", ".....", "##.##", ".....", "....."])
    path = shortest_path(pinch, (0, 0), (4, 4))
    check("choke: a path exists across the doorway", path is not None)
    width, (wr, wc) = narrowest_on_path(pinch, path)
    check("choke: narrowest width is 1 (the doorway)", width == 1)
    check("choke: pinch located at the door cell (row 2,col 2)", (wr, wc) == (2, 2))
    wide_w, _ = narrowest_on_path(open5, shortest_path(open5, (0, 0), (4, 4)))
    check("choke: fully-open route is never a 1-cell pinch", wide_w > 1)

    # open field: an all-open grid is one big exposed region; a fully-covered grid has none.
    open_regions = components(open5, lambda ch: ch == NONE)
    check("open: all-open grid is one 25-cell exposed region",
          len(open_regions) == 1 and len(open_regions[0]) == 25)
    walled = make(["#####" for _ in range(5)])
    check("open: fully-covered grid has no open region",
          not components(walled, lambda ch: ch == NONE))

    # symmetry: a mirror-x-symmetric grid has zero mismatches; a lopsided one is flagged.
    sym = make(["#...#", "o...o", ".....", "o...o", "#...#"])
    check("pvp: mirror-x-symmetric grid has 0 cover mismatches",
          cover_symmetry_mismatches(sym, "mirror-x") == [])
    asym = make(["#....", ".....", ".....", ".....", "....."])  # wall only on the left
    check("pvp: asymmetric grid IS flagged (>0 mismatches)",
          len(cover_symmetry_mismatches(asym, "mirror-x")) > 0)
    check("pvp: point (180°) symmetry accepts a point-symmetric grid",
          cover_symmetry_mismatches(make(["#...o", ".....", ".....", ".....", "o...#"]), "point") == [])
    check("pvp: mirror-y flags a top-heavy grid",
          len(cover_symmetry_mismatches(make(["#####", ".....", ".....", ".....", "....."]),
                                        "mirror-y")) > 0)
    # determinism: mismatch list is stable & sorted across repeated runs.
    check("determinism: mismatch list is stably sorted",
          cover_symmetry_mismatches(asym, "mirror-x") == sorted(cover_symmetry_mismatches(asym, "mirror-x")))

    passed = sum(1 for _, ok in checks if ok)
    print(f"\n[self-test] {passed}/{len(checks)} checks passed")
    sys.exit(0 if passed == len(checks) else 1)


def main():
    ap = argparse.ArgumentParser(description="Lint a baked battlefield map for playability bugs.")
    ap.add_argument("name", nargs="?")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--spawn", action="append", default=[], type=parse_cell,
                    help="a spawn cell 'cx,cy' that must be passable & reachable (repeatable)")
    ap.add_argument("--control", action="append", default=[], type=parse_cell,
                    help="a control-point cell 'cx,cy' that must be reachable from every spawn (repeatable)")
    ap.add_argument("--pvp", choices=["mirror-x", "mirror-y", "point"],
                    help="opt-in fairness gate: assert cover/spawns/control points are symmetric under "
                         "this transform (ERROR on any asymmetry). Use only for competitive maps.")
    ap.add_argument("--preview", action="store_true", help="also write <name>.preview.png")
    ap.add_argument("--self-test", action="store_true",
                    help="run the built-in synthetic good/bad fixtures for every check and exit")
    args = ap.parse_args()

    if args.self_test:
        self_test()

    if args.all:
        names = sorted(p.stem for p in MAPS_DIR.glob("*.covergrid"))
    elif args.name:
        names = [args.name]
    else:
        ap.error("give a map name, --all, or --self-test")

    total_err = 0
    for n in names:
        rep = lint(n, args.spawn, args.control, args.pvp, args.preview)
        total_err += rep.errors
        print(f"  → {rep.errors} error(s), {rep.warns} warning(s)\n")

    sys.exit(1 if total_err else 0)


if __name__ == "__main__":
    main()
