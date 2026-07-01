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
  * wall specks       WARN  — isolated single-cell Heavy blocks (often ingest noise).
  * symmetry          WARN  — left/right mirror mismatch (info for the balance pass; a faithful
                      real map is expected to be asymmetric, so this never fails).
  * structures        INFO  — enumerates connected Heavy blobs as objects with bbox + centroid in
                      CELL coordinates, so a bug report can say "building at (26,32)-(34,38)".

Usage:
    python3 tools/maps/lint.py pointe-du-hoc
    python3 tools/maps/lint.py pointe-du-hoc --spawn 20,20 --spawn 100,100
    python3 tools/maps/lint.py --all
    python3 tools/maps/lint.py pointe-du-hoc --preview   # also write <name>.preview.png

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


def lint(name, spawns, preview):
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

    # --- isolated wall specks ---------------------------------------------------------------
    heavy_comps = components(grid, lambda ch: ch == HEAVY)
    specks = [c for c in heavy_comps if len(c) == 1]
    if specks:
        rep.warn(f"{len(specks)} isolated single-cell wall(s) — often ingest noise, e.g. cell "
                 + ", ".join(f"({c[0][1]},{row_to_cell_y(c[0][0], rows)})" for c in specks[:5])
                 + (" ..." if len(specks) > 5 else ""))

    # --- symmetry (info for the balance pass; never an error) -------------------------------
    mism = sum(
        1
        for r in range(rows)
        for c in range(GRID // 2)
        if passable(grid[r][c]) != passable(grid[r][GRID - 1 - c])
    )
    rep.info(f"left/right mirror mismatch: {round(mism / (rows * (GRID // 2)), 3)} "
             "(0 = symmetric; high is fine for a faithful real map, flags a balance target)")

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


def parse_spawn(s):
    x, y = s.split(",")
    return (int(x), int(y))


def main():
    ap = argparse.ArgumentParser(description="Lint a baked battlefield map for playability bugs.")
    ap.add_argument("name", nargs="?")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--spawn", action="append", default=[], type=parse_spawn,
                    help="a spawn cell 'cx,cy' that must be passable & reachable (repeatable)")
    ap.add_argument("--preview", action="store_true", help="also write <name>.preview.png")
    args = ap.parse_args()

    if args.all:
        names = sorted(p.stem for p in MAPS_DIR.glob("*.covergrid"))
    elif args.name:
        names = [args.name]
    else:
        ap.error("give a map name or --all")

    total_err = 0
    for n in names:
        rep = lint(n, args.spawn, args.preview)
        total_err += rep.errors
        print(f"  → {rep.errors} error(s), {rep.warns} warning(s)\n")

    sys.exit(1 if total_err else 0)


if __name__ == "__main__":
    main()
