#!/usr/bin/env python3
"""Ingest real-world GIS data for a battlefield map — stage 1 of the map pipeline.

Script-not-binary (decisions.md D41/D46): this generator + the cached extract it writes are
the committed record; anyone can re-run it to refresh from live sources.

Given a config (`tools/maps/config/<name>.json`) that names a lon/lat bounding box, this pulls
the two source layers a real map is built from:

  * ELEVATION  — a Digital Elevation Model (Copernicus DEM GLO-30 / SRTM / USGS 3DEP) resampled
                 onto the 128x128 sim grid. Written as raw little-endian f32 metres
                 (`<name>.height.f32`). RENDER-ONLY today: the sim has no elevation layer yet
                 (core::terrain is cover-only), so this feeds the render mesh (terrain_mesh.py)
                 and waits on the sim-elevation decision. See README.
  * FEATURES   — vector features from OpenStreetMap (buildings, walls, hedges, forest, water),
                 normalised into bbox-relative [0,1] coordinates and written as
                 `<name>.features.json`. This is what bake.py rasterises into a cover grid.

The heavy GIS libraries (osmnx, rasterio, GDAL) are NOT assumed present on this box. When they
are installed the real fetch path runs; otherwise a DETERMINISTIC synthetic extract is written
(seeded from the bbox) so the rest of the pipeline runs end-to-end offline with zero deps. The
synthetic path is clearly stamped `"synthetic": true` in the output so no one mistakes a
placeholder for a real import.

Usage:
    python3 tools/maps/ingest.py pointe-du-hoc          # by config name
    python3 tools/maps/ingest.py --all                  # every config in config/

Output (committed as the cached source extract for a reproducible offline bake):
    assets/maps/<name>.features.json
    assets/maps/<name>.height.f32
"""

import argparse
import array
import hashlib
import json
import math
import random
import struct
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CONFIG_DIR = Path(__file__).resolve().parent / "config"
OUT_DIR = REPO / "assets" / "maps"
GRID = 128  # must equal core::flow_field::GRID

# Optional real-fetch backends. Absent on this workstation → synthetic path.
try:
    import osmnx  # noqa: F401
    import rasterio  # noqa: F401

    HAVE_GIS = True
except Exception:
    HAVE_GIS = False


def seed_for(bbox):
    """A stable integer seed derived from the bbox so a synthetic map is reproducible."""
    h = hashlib.sha256(json.dumps(bbox).encode()).digest()
    return int.from_bytes(h[:8], "big")


# --------------------------------------------------------------------------------------------
# Real fetch path (runs only where osmnx + rasterio are installed). Documented + wired, but
# this workstation has no GIS stack, so it is exercised on CI/dev boxes that install it.
# --------------------------------------------------------------------------------------------
def fetch_real(cfg):
    """Pull live OSM features + a DEM tile and normalise to the pipeline's intermediate format.

    Intentionally thin: the point of the spike is the deterministic BAKE, not GIS plumbing.
    Real-fetch specifics (Overpass tags → feature kinds, DEM reprojection to the bbox grid)
    are left as clearly-marked TODOs so this fails loudly rather than fabricating data.
    """
    raise NotImplementedError(
        "Real GIS fetch not implemented in the spike. Install osmnx+rasterio and fill in:\n"
        "  1. osmnx.features_from_bbox(bbox, tags={building, barrier, natural, landuse, waterway})\n"
        "  2. project each geometry to bbox-relative [0,1] and classify to a feature 'kind'\n"
        "  3. read a DEM tile (Copernicus GLO-30) with rasterio, resample to 128x128, write f32.\n"
        "Until then run without a GIS stack to get the deterministic synthetic extract."
    )


# --------------------------------------------------------------------------------------------
# Synthetic path — deterministic placeholder standing in for a live import so the pipeline runs
# offline. Produces a plausible clifftop-battery layout: a sea/cliff edge, casemates, craters,
# hedgerows. NOT real geography — stamped synthetic in the output.
# --------------------------------------------------------------------------------------------
def synth_features(cfg):
    rng = random.Random(seed_for(cfg["bbox"]))
    feats = []

    def poly(kind, cx, cy, w, h):
        feats.append(
            {
                "id": f"{kind}-{len(feats)}",
                "kind": kind,
                "geom": "polygon",
                "coords": [
                    [cx - w / 2, cy - h / 2],
                    [cx + w / 2, cy - h / 2],
                    [cx + w / 2, cy + h / 2],
                    [cx - w / 2, cy + h / 2],
                ],
            }
        )

    def line(kind, pts):
        feats.append({"id": f"{kind}-{len(feats)}", "kind": kind, "geom": "line", "coords": pts})

    # The sea below the cliff: the bottom strip is water (impassable — becomes Heavy today).
    feats.append(
        {
            "id": "water-sea",
            "kind": "water",
            "geom": "polygon",
            "coords": [[0.0, 0.0], [1.0, 0.0], [1.0, 0.18], [0.0, 0.18]],
        }
    )
    # The cliff edge just inland of the water (a hedge/berm line for the spike).
    line("hedge", [[0.0, 0.22], [0.35, 0.24], [0.65, 0.21], [1.0, 0.23]])

    # A cluster of concrete casemates / bunkers (Heavy) across the headland.
    for _ in range(5):
        cx = rng.uniform(0.2, 0.85)
        cy = rng.uniform(0.35, 0.85)
        poly("building", cx, cy, rng.uniform(0.05, 0.09), rng.uniform(0.04, 0.07))

    # Bomb craters — cleared ground (None); represented as absence, so nothing to emit, but a
    # couple of scrub rings (Light) around lips add texture.
    for _ in range(6):
        cx = rng.uniform(0.15, 0.9)
        cy = rng.uniform(0.3, 0.9)
        poly("scrub", cx, cy, rng.uniform(0.03, 0.05), rng.uniform(0.03, 0.05))

    # Hedgerows / bocage lines (Light).
    for _ in range(4):
        y = rng.uniform(0.4, 0.9)
        line("hedge", [[rng.uniform(0.0, 0.3), y], [rng.uniform(0.7, 1.0), y + rng.uniform(-0.1, 0.1)]])

    return feats


def synth_heightgrid(cfg):
    """A synthetic DEM: high, flat headland dropping to sea level over the cliff edge (y<0.22).

    Metres. Render-only — the sim ignores this until the elevation decision lands.
    """
    rng = random.Random(seed_for(cfg["bbox"]) ^ 0x9E3779B9)
    h = array.array("f", [0.0] * (GRID * GRID))
    for cy in range(GRID):
        ny = (cy + 0.5) / GRID
        for cx in range(GRID):
            if ny < 0.18:
                base = 0.0  # sea
            elif ny < 0.24:
                base = 30.0 * (ny - 0.18) / 0.06  # cliff face rising to the headland
            else:
                base = 30.0 + 4.0 * math.sin(cx * 0.15) + 3.0 * math.cos(cy * 0.11)
            base += rng.uniform(-0.6, 0.6)  # micro-relief
            h[cy * GRID + cx] = base
    return h


def build(cfg):
    name = cfg["name"]
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    if HAVE_GIS:
        feats, height = fetch_real(cfg)
        synthetic = False
    else:
        feats = synth_features(cfg)
        height = synth_heightgrid(cfg)
        synthetic = True

    features_doc = {
        "name": name,
        "bbox": cfg["bbox"],
        "grid": GRID,
        "synthetic": synthetic,
        "source": cfg.get("sources", {}).get("features", {}),
        "note": (
            "Bbox-relative [0,1] coords, origin bottom-left (x=lon frac, y=lat frac). "
            "kind ∈ {building, wall, hedge, scrub, forest, water}. Consumed by bake.py. "
            + ("SYNTHETIC placeholder — regenerate from live OSM once a GIS stack is installed." if synthetic else "")
        ),
        "features": feats,
    }
    fpath = OUT_DIR / f"{name}.features.json"
    fpath.write_text(json.dumps(features_doc, indent=2) + "\n")

    hpath = OUT_DIR / f"{name}.height.f32"
    with open(hpath, "wb") as f:
        f.write(struct.pack("<I", GRID))  # header: grid dim, then GRID*GRID LE f32 metres
        f.write(height.tobytes() if sys.byteorder == "little" else _le_bytes(height))

    print(f"[ingest] {name}: {'SYNTHETIC' if synthetic else 'real'}  "
          f"{len(feats)} features → {fpath.relative_to(REPO)}")
    print(f"[ingest] {name}: heightgrid {GRID}x{GRID} → {hpath.relative_to(REPO)} (render-only)")
    return fpath


def _le_bytes(arr):
    return b"".join(struct.pack("<f", v) for v in arr)


def load_cfg(name):
    return json.loads((CONFIG_DIR / f"{name}.json").read_text())


def main():
    ap = argparse.ArgumentParser(description="Ingest GIS data for a battlefield map.")
    ap.add_argument("name", nargs="?", help="config name (without .json)")
    ap.add_argument("--all", action="store_true", help="ingest every config in config/")
    args = ap.parse_args()

    if args.all:
        cfgs = [load_cfg(p.stem) for p in sorted(CONFIG_DIR.glob("*.json"))]
    elif args.name:
        cfgs = [load_cfg(args.name)]
    else:
        ap.error("give a config name or --all")

    for cfg in cfgs:
        build(cfg)


if __name__ == "__main__":
    main()
