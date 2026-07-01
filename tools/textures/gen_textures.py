#!/usr/bin/env python3
"""Generate the ground **detail textures** — the seamless noise that breaks up the flat floor.

Script-not-binary (decisions.md D41/D46): this generator + the manifest entry are the committed
source of record; the raw textures are regenerable artifacts (sha256 pinned below). It bakes small,
**seamlessly tiling** grayscale detail maps with ImageMagick and emits, for each:

  * assets/textures/<name>.png    — the texture (8-bit grey; for inspection / diffing)
  * assets/textures/<name>.gray   — raw R8 bytes (SIZE*SIZE), what render/ include_bytes!s so the
                                    render crate needs NO png-decode dependency (stays wgpu +
                                    bytemuck only, the same rule as the D74 font atlas)
  * assets/textures/manifest.json — provenance (source / license / sha256 / dims / channels)

Currently one texture is baked: `ground` — a **multi-octave seamless HEIGHTFIELD** sampled by the
embodied first-person floor shader (`render/src/world.wgsl`). The shader treats the R8 value as a
terrain *height*: it tiles the map at several world scales for albedo tonal variation, and
reconstructs a per-pixel surface normal by finite-differencing the height so a dim key light gives
the floor real relief instead of a flat slate. It tiles seamlessly so it can be repeated across the
world plane without visible seams.

## The seamless trick

ImageMagick `+noise Random` is NOT tiling on its own. The fix: blur it with `-virtual-pixel tile`,
so the blur convolution **wraps around the edges** — the blurred (and therefore the final) result
tiles with no seam. We stack FIVE wrap-blurred octaves plus a broad tonal *mottle*:

  * **macro** swell — the large-scale terrain silhouette;
  * **meso** undulation — the metre-scale relief the shader finite-differences into a normal;
  * **detail** — sub-metre variation the near floor reads as texture;
  * **micro** grit — the finest near-field speckle;
  * **mottle** — a very-broad low-frequency drift that keeps large regions of floor from all
    settling to the same average tone (patches of damp/dry ground).

Each octave gets its own `-seed` (uncorrelated noise) and is folded into the running result with a
`-compose blend`; the coarse octaves dominate the silhouette, the finer ones only dust detail on
top. After the blend we `-normalize` for full contrast, then a **gentle** `-sigmoidal-contrast`
firms the mid-tones into believable relief WITHOUT crushing the finite-difference gradients the
normal reconstruction depends on. `-seed` makes `+noise` reproducible, so the bytes (and the
sha256 below) are stable for a given ImageMagick build.

Render-only (invariants #1/#4): the texture is a pure presentation derivation — it never touches
the sim, carries no fog/intel (invariant #6), and can never move the per-tick checksum.

Run: `pnpm assets:textures` (or `python3 tools/textures/gen_textures.py`). Requires ImageMagick on
PATH.
"""

import hashlib
import json
import subprocess
import sys
from pathlib import Path

# ---- The contract with render::world (the GROUND_TEX_SIZE const there must match) ----------------
SIZE = 256  # square, power-of-two so GPU mips/tiling are clean
SEED = 1337  # makes +noise reproducible → stable bytes / sha256 for a given ImageMagick build
LICENSE = "CC0-1.0"  # procedurally synthesised from a seed — no third-party asset, public domain

OUT_DIR = Path(__file__).resolve().parents[2] / "assets" / "textures"


def bake_ground(png_path: Path, gray_path: Path) -> bytes:
    """Bake the seamless multi-octave ground HEIGHTFIELD → PNG (inspection) + raw R8 (engine)."""
    # Five wrap-blurred noise octaves + a broad mottle, blended stepwise, then normalised and given a
    # gentle sigmoidal firm-up. `-virtual-pixel tile` makes every blur wrap at the edges, so the result
    # tiles seamlessly. Each `-compose blend -composite` folds the next octave into the running result;
    # the coarse octaves own the silhouette, the finer ones only dust detail on top:
    #   (macro) (meso) 56/44 → (detail) 80/20 → (micro) 90/10 → (mottle) 86/14 → normalize → sigmoidal
    subprocess.run(
        [
            "magick",
            # macro octave — broad soft swells (the large-scale terrain silhouette)
            "(",
            "-size", f"{SIZE}x{SIZE}", "xc:",
            "-seed", str(SEED), "+noise", "Random",
            "-colorspace", "Gray",
            "-virtual-pixel", "tile", "-blur", "0x22",
            "-auto-level",
            ")",
            # meso octave — metre-scale undulation (what the shader lights as relief), own seed
            "(",
            "-size", f"{SIZE}x{SIZE}", "xc:",
            "-seed", str(SEED + 1), "+noise", "Random",
            "-colorspace", "Gray",
            "-virtual-pixel", "tile", "-blur", "0x9",
            "-auto-level",
            ")",
            # fold meso into macro (macro sets the silhouette, meso carries the relief)
            "-define", "compose:args=56,44",
            "-compose", "blend", "-composite",
            # detail octave — sub-metre variation the near floor reads as texture
            "(",
            "-size", f"{SIZE}x{SIZE}", "xc:",
            "-seed", str(SEED + 2), "+noise", "Random",
            "-colorspace", "Gray",
            "-virtual-pixel", "tile", "-blur", "0x3.4",
            "-auto-level",
            ")",
            "-define", "compose:args=80,20",
            "-compose", "blend", "-composite",
            # micro octave — the finest near-field grit, a fourth uncorrelated seed
            "(",
            "-size", f"{SIZE}x{SIZE}", "xc:",
            "-seed", str(SEED + 3), "+noise", "Random",
            "-colorspace", "Gray",
            "-virtual-pixel", "tile", "-blur", "0x1.3",
            "-auto-level",
            ")",
            "-define", "compose:args=90,10",
            "-compose", "blend", "-composite",
            # mottle — a very-broad tonal drift so large patches of floor aren't all one average
            # tone (damp vs. dry ground); the macro height already handles the albedo split, this
            # just breaks up flatness at the map scale.
            "(",
            "-size", f"{SIZE}x{SIZE}", "xc:",
            "-seed", str(SEED + 4), "+noise", "Random",
            "-colorspace", "Gray",
            "-virtual-pixel", "tile", "-blur", "0x44",
            "-auto-level",
            ")",
            "-define", "compose:args=86,14",
            "-compose", "blend", "-composite",
            # stretch to full contrast, then a GENTLE sigmoidal firm-up of the mid-tones. Kept light
            # (2.5 about the mid-point) so relief reads without clipping the finite-difference
            # gradients the shader's normal reconstruction depends on.
            "-normalize",
            "-sigmoidal-contrast", "2.5,50%",
            "-depth", "8",
            str(png_path),
        ],
        check=True,
        capture_output=True,
    )
    # Flatten to a single-channel 8-bit grey of the exact size, then dump raw R8 bytes.
    subprocess.run(
        [
            "magick", str(png_path),
            "-colorspace", "Gray",
            "-resize", f"{SIZE}x{SIZE}!",
            "-depth", "8",
            f"gray:{gray_path}",
        ],
        check=True,
        capture_output=True,
    )
    raw = gray_path.read_bytes()
    expected = SIZE * SIZE
    if len(raw) != expected:
        raise SystemExit(f"raw ground is {len(raw)} bytes, expected {expected}")
    return raw


def main() -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    ground_png = OUT_DIR / "ground.png"
    ground_gray = OUT_DIR / "ground.gray"
    raw = bake_ground(ground_png, ground_gray)

    manifest = {
        "note": (
            "Ground detail textures, generated by tools/textures/gen_textures.py "
            "(decisions.md D41/D46). Seamlessly-tiling grayscale noise sampled by the render crate "
            "(render/src/world.wgsl) to break up the flat embodied floor. Each <name>.gray is raw "
            "R8 bytes (SIZE*SIZE) include_bytes!d straight in, so the render crate stays "
            "wgpu+bytemuck only (no png-decode dep). Render-only; regenerate with "
            "`pnpm assets:textures`."
        ),
        "source": "ImageMagick (+noise Random, wrap-blurred for seamless tiling)",
        "license": LICENSE,
        "author": "procedurally synthesised (seed-based) via ImageMagick",
        "seed": SEED,
        "textures": [
            {
                "name": "ground",
                "size": SIZE,
                "channels": 1,
                "format": "R8",
                "png_bytes": ground_png.stat().st_size,
                "png_sha256": hashlib.sha256(ground_png.read_bytes()).hexdigest(),
                "gray_bytes": len(raw),
                "gray_sha256": hashlib.sha256(raw).hexdigest(),
            }
        ],
    }
    (OUT_DIR / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

    print(f"ground {SIZE}x{SIZE} R8  {len(raw)} raw bytes")
    print(f"gray sha256 {manifest['textures'][0]['gray_sha256']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
