#!/usr/bin/env python3
"""Generate the equirectangular Earth land mask the globe backdrop samples (D103).

Script-not-binary (decisions.md D41/D46): this generator + the manifest entry are the committed
source of record; the mask is a regenerable artifact. It downloads the **Natural Earth 1:50m
land** polygons (public domain) and rasterizes them — pure-Python scanline fill, no GIS deps —
into an equirectangular 8-bit mask (255 = land, 0 = sea), and emits:

  * assets/earth/landmask.gray  — raw R8 bytes (MASK_W * MASK_H, row 0 = lat +90°), what
                                  render/ include_bytes!s as a GPU texture so the render crate
                                  needs NO decode dependency (stays wgpu + bytemuck only — the
                                  same contract as assets/fonts/hud_atlas.gray)
  * assets/earth/landmask.png   — preview (via ImageMagick) for inspection / diffing
  * assets/earth/manifest.json  — provenance (source / license / url / sha256), the auditable
                                  record (content-pipeline.md §3)

The mask dimensions are the contract with `render::globe_backdrop` — the MASK_* consts there MUST
match. Longitude spans −180..+180 left→right; latitude +90..−90 top→bottom (plate carrée), so a
shader maps a unit-sphere normal to UV with plain atan2/asin.

Run: `python3 tools/earth/gen_landmask.py`. Requires network (first run) + ImageMagick on PATH.
A cached copy of the GeoJSON next to this script is reused when present, so regeneration is
deterministic offline once the source is fetched.
"""

import hashlib
import json
import subprocess
import sys
import urllib.request
from pathlib import Path

# ---- The contract with render::globe_backdrop (MASK_* consts there must match) ------------------
# 1440x720 @ 1:50m (was 720x360 @ 1:110m): the D106 battlefield overview zooms the globe onto a
# single war, and at that framing the 1:110m data simply omits the smaller shipped battlegrounds
# (Gotland, Espiritu Santo) — no resolution bump fixes missing source polygons. 0.25°/texel keeps
# the embedded R8 at ~1 MB (1,036,800 bytes), the accepted cost of islands existing.
MASK_W = 1440  # 0.25° per texel in longitude
MASK_H = 720  # 0.25° per texel in latitude

SOURCE_NAME = "Natural Earth 1:50m Land (ne_50m_land)"
SOURCE_LICENSE = "Public Domain (Natural Earth terms of use)"
SOURCE_URL = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/"
    "geojson/ne_50m_land.geojson"
)

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "assets" / "earth"
CACHE = Path(__file__).resolve().parent / "ne_50m_land.geojson"


def fetch_geojson() -> dict:
    """Load the land polygons — from the committed cache if present, else the network (and cache)."""
    if not CACHE.exists():
        print(f"fetching {SOURCE_URL}")
        with urllib.request.urlopen(SOURCE_URL, timeout=60) as resp:
            CACHE.write_bytes(resp.read())
    return json.loads(CACHE.read_text())


def rings_of(feature) -> list:
    """Every outer/inner ring of a (Multi)Polygon feature, as [[lon, lat], ...] lists."""
    geom = feature["geometry"]
    if geom["type"] == "Polygon":
        return list(geom["coordinates"])
    if geom["type"] == "MultiPolygon":
        return [ring for poly in geom["coordinates"] for ring in poly]
    return []


def rasterize(rings: list) -> bytearray:
    """Even-odd scanline fill of every ring into the MASK_W x MASK_H grid.

    Even-odd across ALL rings at once makes holes (inner rings) subtract naturally — the classic
    polygon-fill rule, deterministic and dependency-free. Each pixel row samples at its latitude
    centre; crossings are computed per edge and filled between sorted pairs.
    """
    mask = bytearray(MASK_W * MASK_H)
    # Pre-flatten edges: (lon0, lat0, lon1, lat1), skipping degenerate horizontals.
    edges = []
    for ring in rings:
        for (lon0, lat0), (lon1, lat1) in zip(ring, ring[1:] + ring[:1]):
            if lat0 != lat1:
                edges.append((lon0, lat0, lon1, lat1))
    for row in range(MASK_H):
        lat = 90.0 - (row + 0.5) * (180.0 / MASK_H)  # row centre latitude
        xs = []
        for lon0, lat0, lon1, lat1 in edges:
            # Half-open rule [min, max): each vertex counts for exactly one of its two edges.
            if (lat0 <= lat < lat1) or (lat1 <= lat < lat0):
                t = (lat - lat0) / (lat1 - lat0)
                xs.append(lon0 + t * (lon1 - lon0))
        xs.sort()
        for i in range(0, len(xs) - 1, 2):
            # Fill the span [xs[i], xs[i+1]] at this latitude, clamped into the grid.
            px0 = int((xs[i] + 180.0) / 360.0 * MASK_W)
            px1 = int((xs[i + 1] + 180.0) / 360.0 * MASK_W)
            px0 = max(px0, 0)
            px1 = min(px1, MASK_W - 1)
            base = row * MASK_W
            for px in range(px0, px1 + 1):
                mask[base + px] = 255
    return mask


def main() -> int:
    geo = fetch_geojson()
    rings = [ring for feature in geo["features"] for ring in rings_of(feature)]
    print(f"rasterizing {len(rings)} rings at {MASK_W}x{MASK_H}")
    mask = rasterize(rings)
    land_pct = 100.0 * sum(1 for b in mask if b) / len(mask)
    # Sanity: land is ~29% of Earth's surface; equirectangular over-weights the poles a little
    # (Antarctica), so accept a generous band and fail loudly outside it.
    if not 20.0 <= land_pct <= 40.0:
        print(f"land fraction {land_pct:.1f}% is implausible — refusing to write", file=sys.stderr)
        return 1

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    raw = OUT_DIR / "landmask.gray"
    raw.write_bytes(bytes(mask))
    subprocess.run(
        [
            "magick",
            "-size",
            f"{MASK_W}x{MASK_H}",
            "-depth",
            "8",
            f"gray:{raw}",
            str(OUT_DIR / "landmask.png"),
        ],
        check=True,
    )
    manifest = {
        "landmask.gray": {
            "source": SOURCE_NAME,
            "license": SOURCE_LICENSE,
            "url": SOURCE_URL,
            "generator": "tools/earth/gen_landmask.py",
            "width": MASK_W,
            "height": MASK_H,
            "format": "R8 (255=land, 0=sea), equirectangular, row 0 = lat +90",
            "land_pct": round(land_pct, 2),
            "sha256": hashlib.sha256(bytes(mask)).hexdigest(),
        }
    }
    (OUT_DIR / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"wrote {raw} ({len(mask)} bytes, {land_pct:.1f}% land)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
