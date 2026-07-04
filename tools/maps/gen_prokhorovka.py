#!/usr/bin/env python3
"""Synthesize the "Prokhorovka, Kursk" battlefield cover grid — a large, EVEN steppe map.

Script-not-binary (decisions.md D41/D46): the committed record is this generator + the
git-diffable `.covergrid` + a `.meta.json` sidecar (sha256'd). No opaque binary blob. This is
the offline sibling of `bake.py` (real GIS) and `generate.py` (abstract procedural): it hand-
shapes a *named real place* — the July 1943 tank battle on the open steppe around Prokhorovka —
without a network fetch, exactly the `synthetic_source` path Pointe du Hoc already ships.

WHY PROKHOROVKA, WHY EVEN
  The design ask was a *real place where a battle was fought, on an EVEN playing field* — a map
  that plays big and gives neither commander a structural edge. Prokhorovka fits: the largest
  tank clash of WWII was decided on flat, open ground where both sides met in the open. So the
  whole field is built on the WEST half and MIRRORED across x (col c -> 127-c). That makes the
  map EXACTLY symmetric under mirror-x by construction — `tools/maps/lint.py --pvp mirror-x`
  passes, and neither spawn inherits better cover, sightlines, or approach. The recognizable
  features (two settlements, the rail line, shelterbelts) appear as mirrored pairs, which also
  reads historically: two opposing forces across the steppe.

OFFLINE TOOLING, NOT SIM CODE (CLAUDE.md invariant #1/#2):
  * Never imports/touches `core` or the sim. Its ONLY entropy is `random.Random(SEED)` — a
    single seeded stream, drawn in a fixed code order, west-half only. Same seed -> byte-
    identical output. No wall-clock, no global `random`, no unseeded draws.
  * Output is an integer, byte-stable cover grid in the SAME format `bake.py`/`generate.py`
    emit, so it lints with `tools/maps/lint.py` unchanged and `core::terrain::apply_cover_grid`
    decodes it directly.

Output format (identical to bake.py / generate.py):
    128 lines of 128 chars, one Cover level per cell, NORTH-FIRST (row 0 = highest cy):
        '.' = Cover::None       open steppe
        'o' = Cover::Light      shelterbelts / scrub / rail embankment (concealment, sight passes)
        '#' = Cover::Impassable village buildings (block movement AND sight; D92)
    Trailing newline.

Usage:
    python3 tools/maps/gen_prokhorovka.py            # write covergrid + meta.json
    python3 tools/maps/gen_prokhorovka.py --verify   # generate twice, assert byte-identical
"""

import argparse
import hashlib
import json
import random
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
OUT_DIR = REPO / "assets" / "maps"
NAME = "prokhorovka"
MAP_ID = 2  # == core::terrain::Terrain::PROKHOROVKA_MAP_ID
SEED = 1943  # the battle year — a fixed, documented seed (only the sparse scrub scatter uses it)

GRID = 128  # == core::flow_field::GRID; lint.py requires exactly this, so it is fixed.
AXIS = GRID // 2  # mirror axis sits between col 63 and 64 (GRID even -> no cell is self-mirror)

NONE, LIGHT, HEAVY = ".", "o", "#"


def in_bounds(cx, cy):
    return 0 <= cx < GRID and 0 <= cy < GRID


def fill_rect(cells, cx0, cy0, cx1, cy1, ch):
    """Set an inclusive rectangle [cx0..cx1] x [cy0..cy1] (west-half cells only expected)."""
    for cy in range(min(cy0, cy1), max(cy0, cy1) + 1):
        for cx in range(min(cx0, cx1), max(cx0, cx1) + 1):
            if in_bounds(cx, cy):
                cells[cy][cx] = ch


def building(cells, cx0, cy0, cx1, cy1):
    """A solid building footprint (HEAVY). Kept small and street-gapped by the caller so the
    settlement never walls off a lane — the open steppe around it stays connected."""
    fill_rect(cells, cx0, cy0, cx1, cy1, HEAVY)


def build_west_half(seed):
    """Paint every feature on the WEST half (cols 0..AXIS-1). The east half is a pure reflection,
    added later, so ALL fairness lives here: whatever advantage a feature gives, the mirror gives
    the other side identically."""
    cells = [[NONE] * GRID for _ in range(GRID)]
    rng = random.Random(seed)

    # 1. The Prokhorovka rail line — a north-south embankment (LIGHT: concealment, passable). Sits
    #    at cx 49 (mirrors to cx 78), leaving a wide-open central corridor between the two lines for
    #    the armour to cross — the open ground the battle was actually fought on.
    for cy in range(6, GRID - 6):
        fill_rect(cells, 48, cy, 49, cy, LIGHT)

    # 2. "Oktyabrsky State Farm" — the north settlement. A handful of street-gapped building blocks
    #    (HEAVY) around cx 16..30, cy 74..92; open ground on every side keeps it fully traversable.
    farm_blocks = [
        (16, 74, 19, 77), (23, 74, 26, 77), (30, 74, 32, 76),
        (16, 81, 18, 84), (22, 81, 25, 84), (29, 81, 31, 84),
        (18, 88, 21, 91), (25, 88, 28, 91),
    ]
    for b in farm_blocks:
        building(cells, *b)
    # An orchard belt fringing the farm (LIGHT).
    fill_rect(cells, 14, 71, 33, 72, LIGHT)

    # 3. A southern hamlet — a smaller mirrored settlement around cx 20..30, cy 32..42.
    hamlet_blocks = [
        (20, 34, 22, 37), (26, 34, 29, 37),
        (20, 40, 23, 42), (27, 40, 29, 42),
    ]
    for b in hamlet_blocks:
        building(cells, *b)

    # 4. Shelterbelts — the windbreak treelines that stripe Russian steppe farmland (LIGHT). Two
    #    belts, well clear of the central corridor so they shape flank fights, not the main lane.
    fill_rect(cells, 2, 58, 40, 59, LIGHT)     # long east-west belt, mid-field
    fill_rect(cells, 6, 20, 7, 52, LIGHT)      # north-south belt on the far (west) flank

    # 5. Mid-field cover at the central objective (cx 60, mirrors to cx 67) — a copse of scrub with a
    #    couple of burnt-out wrecks (HEAVY hard cover), so the central post is contestable rather than
    #    a killing floor. Kept west of the axis so cols 62..65 stay an open crossing lane.
    fill_rect(cells, 54, 60, 61, 68, LIGHT)    # copse around the central post
    building(cells, 57, 63, 58, 64)            # a wreck: two cells of hard cover
    building(cells, 59, 66, 60, 66)            # a second wreck, offset

    # 6. Sparse scrub scatter across the open steppe (LIGHT) — the ONLY randomized step, seeded and
    #    west-half only so the mirror stays exact. Skips any cell already featured.
    for _ in range(140):
        cx = rng.randint(2, AXIS - 3)
        cy = rng.randint(4, GRID - 5)
        if cells[cy][cx] == NONE:
            cells[cy][cx] = LIGHT

    return cells


def mirror_x(cells):
    """Reflect the painted west half onto the east half: col c -> col (GRID-1-c). Exact by
    construction -> the finished grid is symmetric under mirror-x (the fairness guarantee)."""
    for cy in range(GRID):
        for cx in range(AXIS):
            cells[cy][GRID - 1 - cx] = cells[cy][cx]
    return cells


def render(cells):
    """Serialize to the covergrid text: NORTH-FIRST (row 0 = highest cy), trailing newline."""
    lines = ["".join(cells[cy]) for cy in range(GRID - 1, -1, -1)]
    return "\n".join(lines) + "\n"


def generate(seed=SEED):
    cells = build_west_half(seed)
    mirror_x(cells)
    return render(cells)


def counts(text):
    flat = text.replace("\n", "")
    return {
        "none": flat.count(NONE),
        "light": flat.count(LIGHT),
        "heavy": flat.count(HEAVY),
    }


def assert_symmetric(text):
    """Belt-and-suspenders: prove the emitted grid really is mirror-x symmetric before we ship it."""
    rows = text.splitlines()
    for r, row in enumerate(rows):
        for c in range(AXIS):
            assert row[c] == row[GRID - 1 - c], f"mirror mismatch at row {r}, col {c}"


def main():
    ap = argparse.ArgumentParser(description="Synthesize the Prokhorovka (Kursk) cover grid.")
    ap.add_argument("--verify", action="store_true",
                    help="generate twice and assert byte-identical (determinism check), no write")
    args = ap.parse_args()

    text = generate()
    assert_symmetric(text)

    if args.verify:
        again = generate()
        assert text == again, "non-deterministic output!"
        assert_symmetric(again)
        print("OK: deterministic and mirror-x symmetric")
        return

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    covergrid_path = OUT_DIR / f"{NAME}.covergrid"
    covergrid_path.write_text(text)
    sha = hashlib.sha256(text.encode()).hexdigest()

    c = counts(text)
    meta = {
        "name": NAME,
        "title": "Prokhorovka, Kursk",
        "map_id": MAP_ID,
        "mode": "inspired",
        "fidelity": "material",
        "era": "1943-07",
        "generator": "tools/maps/gen_prokhorovka.py",
        "seed": SEED,
        "symmetry": "mirror-x",
        "synthetic_source": True,
        "grid": GRID,
        "cell_world_units": 1,
        "cover_cells": c,
        "cover_density": round((c["light"] + c["heavy"]) / (GRID * GRID), 4),
        "covergrid_file": f"{NAME}.covergrid",
        "covergrid_sha256": sha,
        "notes": (
            "July 1943 tank battle on open steppe. Built on the west half and mirrored across x "
            "(mirror-x) so the field is EXACTLY even — neither commander gets a structural edge. "
            "Features (rail line, two settlements, shelterbelts) appear as mirrored pairs. "
            "Synthetic source (no live GIS fetch), same as pointe-du-hoc; elevation is not modeled "
            "(the steppe is flat — the point of the location)."
        ),
        "license": "synthetic (generator-authored); no third-party GIS data",
        "lint_cmd": (
            f"python3 tools/maps/lint.py {NAME} "
            "--spawn 11,64 --spawn 116,64 "
            "--control 60,64 --control 67,64 --control 38,96 --control 89,96 "
            "--control 38,32 --control 89,32 --pvp mirror-x"
        ),
    }
    (OUT_DIR / f"{NAME}.meta.json").write_text(json.dumps(meta, indent=2) + "\n")

    print(f"wrote {covergrid_path.relative_to(REPO)}  sha256={sha[:16]}…")
    print(f"  cover: none={c['none']} light={c['light']} heavy={c['heavy']} "
          f"density={meta['cover_density']}")
    print(f"  lint:  {meta['lint_cmd']}")


if __name__ == "__main__":
    main()
