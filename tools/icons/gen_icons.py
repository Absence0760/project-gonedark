#!/usr/bin/env python3
"""Generate the command-view HUD **icon atlas** — the small tactical glyphs that sit beside the
text-only command-bar / readout labels so the RTS chrome reads as *designed*, not debug.

Script-not-binary (decisions.md D41/D46): this generator + the per-icon `.svg` sources it writes +
the manifest entry are the committed source of record; the atlas is a regenerable artifact. It

  1. authors a small set of clean, CLI-defined **SVG** icons (white shapes on a transparent ground),
     writing each to `assets/icons/svg/<name>.svg` (committed source),
  2. rasterises each with **Inkscape** headless into a fixed CELL×CELL RGBA PNG tile
     (`inkscape --export-type=png --export-filename=… -w CELL -h CELL in.svg`),
  3. montages the tiles into a COLS×ROWS grid with **ImageMagick** (transparent gutters), and
  4. dumps:
       * assets/icons/icons_atlas.png   — the packed atlas (RGBA; for inspection / diffing)
       * assets/icons/icons_atlas.rgba  — raw RGBA8 bytes (ATLAS_W*ATLAS_H*4), what render/ include_bytes!s
                                          so the render crate stays wgpu + bytemuck only (NO png-decode)
       * assets/icons/manifest.json     — provenance (source / license CC0 / sha256), the auditable record

The grid metrics below are the contract with `render::icon` — the `ICON_*` consts there MUST match
the `grid` block of the manifest, and `ICONS` MUST match `render::icon::IconKind`'s order (the atlas
index of each icon is its position in this list, laid out row-major across COLS columns).

Run: `pnpm assets:icons` (or `python3 tools/icons/gen_icons.py`). Requires Inkscape + ImageMagick.
"""

import hashlib
import json
import subprocess
import sys
from pathlib import Path

# ---- The contract with render::icon (ICON_* consts + IconKind order there must match) -----------
COLS = 4
CELL = 64  # one cell's pixel size (square)
# Atlas index == position in this list (row-major). Order MUST match render::icon::IconKind.
ICONS = [
    "infantry",   # 0 — a foot-soldier token (train Rifleman)
    "armor",      # 1 — a tank/armor token (train Heavy)
    "build",      # 2 — a hammer (build / construct)
    "upgrade",    # 3 — a double chevron (upgrade a tier)
    "resources",  # 4 — a credits crystal (banked resources)
    "objective",  # 5 — a flag (mission objective / control point)
    "move",       # 6 — an arrow (the move order)
    "attack",     # 7 — a crosshair (the attack order)
    "hold",       # 8 — a shield (the hold-position stance)
]
ICON_COUNT = len(ICONS)
ROWS = (ICON_COUNT + COLS - 1) // COLS  # 3
ATLAS_W = COLS * CELL  # 256
ATLAS_H = ROWS * CELL  # 192

LICENSE = "CC0-1.0"  # Original CLI-authored geometry — public domain, redistribution-clean.

OUT_DIR = Path(__file__).resolve().parents[2] / "assets" / "icons"
SVG_DIR = OUT_DIR / "svg"

# ---- The icon geometry — clean, bold, legible-at-small-size shapes on a 64×64 viewBox. ----------
# White (#ffffff) shapes on a transparent ground; the render pass tints them per draw (text-style
# coverage), so authoring them white keeps every icon recolourable from the theme palette.
_HEAD = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">'
_FILL = 'fill="#ffffff"'
_STROKE = 'fill="none" stroke="#ffffff" stroke-linecap="round" stroke-linejoin="round"'


def svg_for(name: str) -> str:
    """Return the SVG document for one icon name (white-on-transparent, 64×64 viewBox).

    Design language: a coherent tactical glyph set. Solid white silhouettes for *things*
    (units/structures/resources/objective), open white strokes for *orders/actions*
    (upgrade/attack). Stroke-based icons share one weight (_SW) for a consistent line; all
    detailing reads through **negative space** (transparent seams) so every shape stays pure
    white + alpha and tints cleanly per draw — no painted-in dark strokes (which the alpha-
    coverage tint would just flatten into solid fill).
    """
    _SW = "6.5"  # one shared stroke weight for the open/line icons (upgrade, attack)
    if name == "infantry":
        # A soldier bust — domed helmet head over square-set shoulders (rifleman token).
        body = (
            f'<circle cx="32" cy="18" r="8.6" {_FILL}/>'
            f'<path d="M14,55 L14,43 C14,35 22,30 32,30 C42,30 50,35 50,43 '
            f'L50,55 Z" {_FILL}/>'
        )
    elif name == "armor":
        # Side-on tank: sloped hull + turret + barrel, sitting on a track bar with a thin
        # transparent seam between hull and track so the running gear reads as separate.
        body = (
            f'<rect x="6" y="44" width="52" height="12" rx="6" {_FILL}/>'
            f'<path d="M10,42 L15,31 L49,31 L54,42 Z" {_FILL}/>'
            f'<rect x="22" y="21" width="17" height="11" rx="3" {_FILL}/>'
            f'<rect x="38" y="24.5" width="22" height="4.5" rx="2.2" {_FILL}/>'
        )
    elif name == "build":
        # A hammer — head bar over a centred handle (build / construct).
        body = (
            f'<rect x="18" y="11" width="28" height="10" rx="3" {_FILL}/>'
            f'<rect x="28.5" y="20" width="7" height="34" rx="3" {_FILL}/>'
        )
    elif name == "upgrade":
        # Two stacked up-chevrons — "advance a tier".
        body = (
            f'<path d="M14,30 L32,15 L50,30" {_STROKE} stroke-width="{_SW}"/>'
            f'<path d="M14,48 L32,33 L50,48" {_STROKE} stroke-width="{_SW}"/>'
        )
    elif name == "resources":
        # A cut credits gem — four white facets (two crown, two pavilion) split by thin
        # transparent seams (the dark HUD shows through), so it reads faceted while staying
        # pure white + alpha.
        body = (
            f'<path d="M31,10 L12,28 L31,28 Z" {_FILL}/>'   # crown left
            f'<path d="M33,10 L52,28 L33,28 Z" {_FILL}/>'   # crown right
            f'<path d="M12,30 L31,30 L32,57 Z" {_FILL}/>'   # pavilion left
            f'<path d="M52,30 L33,30 L32,57 Z" {_FILL}/>'   # pavilion right
        )
    elif name == "objective":
        # A pennant on a pole — objective / control point.
        body = (
            f'<rect x="16.5" y="8" width="4.5" height="48" rx="2" {_FILL}/>'
            f'<path d="M21,10 L50,17 L21,24 Z" {_FILL}/>'
        )
    elif name == "move":
        # An upward arrow — the move order.
        body = (
            f'<path d="M32,8 L51,29 L41,29 L41,55 L23,55 L23,29 L13,29 Z" {_FILL}/>'
        )
    elif name == "attack":
        # A crosshair — ring + four ticks + a center dot.
        body = (
            f'<circle cx="32" cy="32" r="16" {_STROKE} stroke-width="{_SW}"/>'
            f'<path d="M32,5 L32,15 M32,49 L32,59 M5,32 L15,32 M49,32 L59,32" '
            f'{_STROKE} stroke-width="{_SW}"/>'
            f'<circle cx="32" cy="32" r="3.6" {_FILL}/>'
        )
    elif name == "hold":
        # A shield — the hold-position stance.
        body = (
            f'<path d="M32,7 L53,15 L53,32 C53,46 43,54 32,59 '
            f'C21,54 11,46 11,32 L11,15 Z" {_FILL}/>'
        )
    else:
        raise ValueError(f"no geometry for icon {name!r}")
    return _HEAD + body + "</svg>"


def rasterize(svg_path: Path, out_png: Path) -> None:
    """Rasterise one SVG into a CELL×CELL RGBA PNG with Inkscape (argv — no shell)."""
    subprocess.run(
        [
            "inkscape",
            "--export-type=png",
            f"--export-filename={out_png}",
            "-w", str(CELL),
            "-h", str(CELL),
            str(svg_path),
        ],
        check=True,
        capture_output=True,
    )


def main() -> int:
    for tool in ("inkscape", "magick", "montage"):
        if subprocess.run(["which", tool], capture_output=True).returncode != 0:
            print(f"required tool not found on PATH: {tool}", file=sys.stderr)
            return 1
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    SVG_DIR.mkdir(parents=True, exist_ok=True)
    tiles_dir = OUT_DIR / "_tiles"
    tiles_dir.mkdir(exist_ok=True)

    # 1+2. Author each SVG (committed source) and rasterise it to a tile named by atlas index so the
    # montage order is exact.
    tile_paths = []
    for idx, name in enumerate(ICONS):
        svg_path = SVG_DIR / f"{name}.svg"
        svg_path.write_text(svg_for(name) + "\n")
        tile = tiles_dir / f"i{idx:03d}.png"
        rasterize(svg_path, tile)
        tile_paths.append(str(tile))

    # 3. Pack tiles into a COLS×ROWS grid, zero spacing/border, transparent gutters → exact CELL grid.
    atlas_png = OUT_DIR / "icons_atlas.png"
    subprocess.run(
        [
            "montage", *tile_paths,
            "-tile", f"{COLS}x{ROWS}",
            "-geometry", f"{CELL}x{CELL}+0+0",
            "-background", "none",
            str(atlas_png),
        ],
        check=True,
        capture_output=True,
    )

    # 4. Force the exact atlas size and dump raw straight-alpha RGBA8 bytes (what render/ maps as a
    # texture). `-background none -alpha on` keeps the transparent ground; no premultiply so the
    # white shapes' edges stay white and only the alpha ramps (clean tinting in the shader).
    atlas_rgba = OUT_DIR / "icons_atlas.rgba"
    subprocess.run(
        [
            "magick", str(atlas_png),
            "-background", "none", "-alpha", "on",
            "-resize", f"{ATLAS_W}x{ATLAS_H}!",
            "-depth", "8",
            f"RGBA:{atlas_rgba}",
        ],
        check=True,
        capture_output=True,
    )

    raw = atlas_rgba.read_bytes()
    expected = ATLAS_W * ATLAS_H * 4
    if len(raw) != expected:
        print(f"raw atlas is {len(raw)} bytes, expected {expected}", file=sys.stderr)
        return 1

    # Clean up the per-icon tiles (the atlas + raw + manifest + SVG sources are the committed record).
    for t in tile_paths:
        Path(t).unlink()
    tiles_dir.rmdir()

    manifest = {
        "note": (
            "Command-view HUD icon atlas, generated by tools/icons/gen_icons.py (decisions.md "
            "D41/D46). Fixed-cell RGBA icons; render::icon uploads icons_atlas.rgba as an RGBA8 "
            "texture and samples one cell per icon, tinting it per draw. The ICON_* metrics + "
            "IconKind order in render::icon MUST match the grid + icons fields below. Render-only; "
            "regenerate with `pnpm assets:icons`."
        ),
        "source": "CLI-authored SVG geometry (tools/icons/gen_icons.py), rasterised via Inkscape",
        "license": LICENSE,
        "author": "project-gonedark (original geometry; Inkscape + ImageMagick pipeline)",
        "icons": ICONS,
        "icon_count": ICON_COUNT,
        "grid": {
            "cols": COLS,
            "rows": ROWS,
            "cell": CELL,
            "atlas_w": ATLAS_W,
            "atlas_h": ATLAS_H,
            "channels": 4,
        },
        "png_bytes": atlas_png.stat().st_size,
        "png_sha256": hashlib.sha256(atlas_png.read_bytes()).hexdigest(),
        "rgba_bytes": len(raw),
        "rgba_sha256": hashlib.sha256(raw).hexdigest(),
    }
    (OUT_DIR / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

    print(f"atlas {ATLAS_W}x{ATLAS_H}  {ICON_COUNT} icons  {len(raw)} raw RGBA bytes")
    print(f"rgba sha256 {manifest['rgba_sha256']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
