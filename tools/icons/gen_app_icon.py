#!/usr/bin/env python3
"""Generate the **"Going Dark" app launcher icon** — the home-screen mark for the Android app.

This is the brand mark, not the in-HUD glyph atlas (that's `gen_icons.py`). The concept encodes the
whole pitch in one shape: a tactical **aperture going dark**. A ring of amber ticks circles a bright
central pip; the ticks are full-bright on the upper-left and **fade to darkness toward the lower-right**
— strategic vision dimming — while the centre pip (the one unit you inhabit) stays lit. Command vision
closing in around a single point of presence: *going dark*.

Script-not-binary (decisions.md D41/D46): this generator + the SVG sources it writes (`assets/app-icon/svg/`)
+ the manifest entry are the committed source of record; every PNG under `android/.../res/mipmap-*` is a
regenerable artifact. Palette is pulled straight from `render::theme` (INK / AMBER / AVATAR / HAIRLINE)
so the launcher icon matches the game's art direction exactly.

Pipeline (matches the rest of `tools/`):
  1. author the layers as clean, CLI-defined **SVG** (background grid + foreground aperture mark),
  2. rasterise each with **Inkscape** headless at every Android density,
  3. composite the legacy (pre-API-26) square + round launcher PNGs with **ImageMagick**, and
  4. emit an **adaptive-icon** XML (foreground + background layers) for API 26+ so the launcher masks
     the mark to the device's icon shape, plus a 512² Play-Store master and a provenance manifest.

Run: `pnpm assets:app-icon` (or `python3 tools/icons/gen_app_icon.py`). Requires Inkscape + ImageMagick.
"""

import hashlib
import json
import math
import subprocess
import sys
from pathlib import Path

# ---- Palette (mirrors render::theme; keep in sync if the theme retunes) --------------------------
INK = "#07090C"        # theme::INK   — deepest background
INK_LIFT = "#0C111A"   # a hair above INK for the vignette centre
AMBER = "#E0791F"      # theme::AMBER — the warm signal accent / aperture ticks
AVATAR = "#FFD13D"     # theme::AVATAR — the embodied unit; the bright centre pip
HAIRLINE = "#1A2129"   # theme::HAIRLINE — the faint command-grid lines
RIM = "#28323F"        # theme::RIM — a touch brighter hairline for the aperture rims

LICENSE = "CC0-1.0"    # Original CLI-authored geometry — public domain, redistribution-clean.

# Authoring canvas: the adaptive-icon spec is 108dp; we author at 432 (= 108dp @ xxxhdpi) so the
# largest density rasterises 1:1 and everything else scales down cleanly.
VB = 432
C = VB / 2  # centre

REPO = Path(__file__).resolve().parents[2]
SVG_DIR = REPO / "assets" / "app-icon" / "svg"
MANIFEST = REPO / "assets" / "app-icon" / "manifest.json"
RES = REPO / "android" / "app" / "src" / "main" / "res"

# Android density buckets: (mipmap dir suffix, dpi). Adaptive-layer px = 108dp * dpi/160; legacy
# launcher px = 48dp * dpi/160.
DENSITIES = [
    ("mdpi", 160),
    ("hdpi", 240),
    ("xhdpi", 320),
    ("xxhdpi", 480),
    ("xxxhdpi", 640),
]


def _aperture_mark(r_outer: float) -> str:
    """The amber aperture: a ring of tapered ticks fading to dark on the lower-right, a faint rim,
    and the bright centre pip. `r_outer` sizes the whole mark (tick tips sit at this radius)."""
    r_inner = r_outer * 0.62
    w = r_outer * 0.075          # tick width
    rim_r = r_outer * 1.04       # outer structural rim
    inner_rim_r = r_inner * 0.92 # inner structural rim
    pip_r = r_outer * 0.135      # the embodied-unit centre pip
    n = 24                       # ticks around the ring

    parts = []
    # Outer + inner structural rims — faint, just enough to bind the ticks into one aperture.
    parts.append(
        f'<circle cx="{C}" cy="{C}" r="{rim_r:.2f}" fill="none" '
        f'stroke="{RIM}" stroke-width="2.2" stroke-opacity="0.55"/>'
    )
    parts.append(
        f'<circle cx="{C}" cy="{C}" r="{inner_rim_r:.2f}" fill="none" '
        f'stroke="{AMBER}" stroke-width="2.0" stroke-opacity="0.30"/>'
    )
    # The ticks. Angle 0 = top, increasing clockwise. The "dark" sector is centred on the lower-right
    # (150° clockwise from top); opacity ramps from ~0.12 there up to full on the opposite arc.
    dark_center = 150.0
    for i in range(n):
        ang = i * 360.0 / n
        d = abs(((ang - dark_center + 180.0) % 360.0) - 180.0)  # angular distance from the dark axis
        opacity = 0.12 + 0.88 * (d / 180.0)
        x = C - w / 2
        y = C - r_outer
        h = r_outer - r_inner
        parts.append(
            f'<rect x="{x:.2f}" y="{y:.2f}" width="{w:.2f}" height="{h:.2f}" '
            f'rx="{w / 2:.2f}" fill="{AMBER}" fill-opacity="{opacity:.3f}" '
            f'transform="rotate({ang:.2f} {C} {C})"/>'
        )
    # The centre pip — the one unit you inhabit. A bright avatar dot inside a soft amber halo.
    parts.append(f'<circle cx="{C}" cy="{C}" r="{pip_r * 1.55:.2f}" fill="{AMBER}" fill-opacity="0.16"/>')
    parts.append(f'<circle cx="{C}" cy="{C}" r="{pip_r:.2f}" fill="{AVATAR}"/>')
    return "".join(parts)


def _command_grid() -> str:
    """The background: ink fill + a faint concentric command-map grid (recedes; reads as 'tactical')."""
    parts = [f'<rect x="0" y="0" width="{VB}" height="{VB}" fill="{INK}"/>']
    # A subtle centre lift so the flat ink doesn't read dead.
    parts.append(
        f'<radialGradient id="vig" cx="50%" cy="44%" r="62%">'
        f'<stop offset="0%" stop-color="{INK_LIFT}"/>'
        f'<stop offset="100%" stop-color="{INK}"/></radialGradient>'
        f'<rect x="0" y="0" width="{VB}" height="{VB}" fill="url(#vig)"/>'
    )
    # Concentric range rings + crosshair lines, very low opacity.
    for rr in (0.30, 0.46, 0.62):
        parts.append(
            f'<circle cx="{C}" cy="{C}" r="{VB * rr:.1f}" fill="none" '
            f'stroke="{HAIRLINE}" stroke-width="1.6" stroke-opacity="0.6"/>'
        )
    parts.append(
        f'<path d="M{C},36 L{C},{VB - 36} M36,{C} L{VB - 36},{C}" '
        f'stroke="{HAIRLINE}" stroke-width="1.4" stroke-opacity="0.5"/>'
    )
    return "".join(parts)


def _svg(body: str) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {VB} {VB}" '
        f'width="{VB}" height="{VB}">{body}</svg>'
    )


def build_svgs() -> dict[str, Path]:
    """Author and write the three SVG sources; return {name: path}."""
    SVG_DIR.mkdir(parents=True, exist_ok=True)
    docs = {
        # Adaptive foreground: transparent ground, mark sized to the central 66% safe zone (the
        # launcher may mask/crop the outer 33%), so r_outer ≈ 0.66*VB/2 minus a little breathing room.
        "foreground": _svg(_aperture_mark(r_outer=VB * 0.30)),
        # Adaptive background: the command grid, full-bleed.
        "background": _svg(_command_grid()),
        # Legacy (pre-API-26) composite: grid + a larger mark filling the square frame.
        "legacy": _svg(_command_grid() + _aperture_mark(r_outer=VB * 0.40)),
    }
    paths = {}
    for name, doc in docs.items():
        p = SVG_DIR / f"{name}.svg"
        p.write_text(doc, encoding="utf-8")
        paths[name] = p
    return paths


def rasterize(svg: Path, png: Path, px: int) -> None:
    png.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["inkscape", "--export-type=png", f"--export-filename={png}",
         "-w", str(px), "-h", str(px), str(svg)],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )


def round_mask(src: Path, dst: Path, px: int) -> None:
    """Composite `src` through a centred circular mask into `dst` (the legacy round launcher icon)."""
    subprocess.run(
        ["magick", str(src), "(", "+clone", "-alpha", "extract",
         "-fill", "black", "-colorize", "100",
         "-fill", "white", "-draw", f"circle {px/2},{px/2} {px/2},2", ")",
         "-alpha", "off", "-compose", "CopyOpacity", "-composite", str(dst)],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )


def sha256(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


ADAPTIVE_XML = """<?xml version="1.0" encoding="utf-8"?>
<!-- GENERATED by tools/icons/gen_app_icon.py — do not hand-edit. API 26+ adaptive launcher icon:
     the launcher masks these two layers to the device's icon shape (circle/squircle/…). -->
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@mipmap/ic_launcher_background" />
    <foreground android:drawable="@mipmap/ic_launcher_foreground" />
</adaptive-icon>
"""


def main() -> int:
    for tool in ("inkscape", "magick"):
        if subprocess.run(["which", tool], stdout=subprocess.DEVNULL).returncode:
            print(f"error: {tool!r} not found on PATH", file=sys.stderr)
            return 1

    svgs = build_svgs()
    artifacts: list[Path] = []

    # Per-density rasterisation.
    for suffix, dpi in DENSITIES:
        mip = RES / f"mipmap-{suffix}"
        adaptive_px = round(108 * dpi / 160)
        legacy_px = round(48 * dpi / 160)

        fg = mip / "ic_launcher_foreground.png"
        bg = mip / "ic_launcher_background.png"
        sq = mip / "ic_launcher.png"
        rd = mip / "ic_launcher_round.png"

        rasterize(svgs["foreground"], fg, adaptive_px)
        rasterize(svgs["background"], bg, adaptive_px)
        rasterize(svgs["legacy"], sq, legacy_px)
        round_mask(sq, rd, legacy_px)
        artifacts += [fg, bg, sq, rd]

    # Adaptive-icon descriptors (one set, density-independent).
    anydpi = RES / "mipmap-anydpi-v26"
    anydpi.mkdir(parents=True, exist_ok=True)
    for name in ("ic_launcher.xml", "ic_launcher_round.xml"):
        p = anydpi / name
        p.write_text(ADAPTIVE_XML, encoding="utf-8")
        artifacts.append(p)

    # 512² Play-Store master (square composite).
    store = REPO / "assets" / "app-icon" / "ic_launcher-playstore.png"
    rasterize(svgs["legacy"], store, 512)
    artifacts.append(store)

    # Provenance manifest.
    MANIFEST.parent.mkdir(parents=True, exist_ok=True)
    manifest = {
        "name": "Going Dark — app launcher icon",
        "generator": "tools/icons/gen_app_icon.py",
        "source": "Original CLI-authored SVG geometry (no third-party art).",
        "license": LICENSE,
        "palette": {"ink": INK, "amber": AMBER, "avatar": AVATAR, "hairline": HAIRLINE},
        "svg_sources": {n: str(p.relative_to(REPO)) for n, p in svgs.items()},
        "artifacts": [
            {"path": str(a.relative_to(REPO)), "sha256": sha256(a)}
            for a in sorted(artifacts)
        ],
    }
    MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    print(f"app icon: {len(artifacts)} artifacts across {len(DENSITIES)} densities")
    print(f"  svg sources : {SVG_DIR.relative_to(REPO)}/")
    print(f"  android res : {RES.relative_to(REPO)}/mipmap-*/")
    print(f"  play store  : {store.relative_to(REPO)}")
    print(f"  manifest    : {MANIFEST.relative_to(REPO)}")
    print("\nNext: AndroidManifest <application> needs android:icon=\"@mipmap/ic_launcher\" "
          "android:roundIcon=\"@mipmap/ic_launcher_round\".")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
