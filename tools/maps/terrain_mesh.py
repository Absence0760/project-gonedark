#!/usr/bin/env python3
"""Build a RENDER-ONLY terrain mesh from an ingested heightgrid — stage 3 (Blender).

Run under Blender's Python:
    blender --background --python tools/maps/terrain_mesh.py -- pointe-du-hoc

Script-not-binary (D41/D46): mirrors tools/models/gen_models.py — commit this generator + a
manifest entry, regenerate the glTF on demand.

Invariant boundary (READ THIS): this mesh is RENDER-ONLY. It carries real floating-point
elevation and NEVER enters the sim. The sim's terrain is the integer cover grid from bake.py
(core::terrain), which today is flat (no height). Displaying a cliff here while the sim treats
the field as flat is fine and expected under invariant #4 (sim/render decoupled) — until the
sim-elevation decision lands, elevation is a visual layer only. Do not wire this mesh's z into
any sim query.

Pipeline: read `<name>.height.f32` (LE f32 metres, GRID header) → build a GRID-resolution
displaced plane spanning the sim's [-64, 64) world extent → decimate to the mobile triangle
budget → export glTF → (optionally) gltfpack-compress, exactly like the model pipeline.

Outputs:
    assets/maps/<name>.terrain.glb      interchange mesh
    assets/maps/<name>.terrain.cooked   gltfpack-compressed (if gltfpack on PATH)
"""

import struct
import sys
from pathlib import Path

try:
    import bpy
except ImportError:
    sys.exit("Run under Blender: blender --background --python tools/maps/terrain_mesh.py -- <name>")

REPO = Path(__file__).resolve().parents[2]
OUT_DIR = REPO / "assets" / "maps"
HALF_EXTENT = 64.0  # == core::flow_field::HALF_EXTENT (world units); mesh spans [-64, 64)
TARGET_TRIS = 6000  # mobile budget for one static terrain mesh (tunable)
Z_SCALE = 0.1       # metres → world-unit vertical exaggeration for readability (render taste)


def arg_name():
    argv = sys.argv
    return argv[argv.index("--") + 1] if "--" in argv else "pointe-du-hoc"


def read_heightgrid(path):
    data = path.read_bytes()
    (grid,) = struct.unpack_from("<I", data, 0)
    vals = struct.unpack_from(f"<{grid * grid}f", data, 4)
    return grid, vals


def build_mesh(name):
    grid, h = read_heightgrid(OUT_DIR / f"{name}.height.f32")

    # Fresh scene.
    bpy.ops.wm.read_factory_settings(use_empty=True)

    verts = []
    for cy in range(grid):
        wy = -HALF_EXTENT + (cy + 0.5) / grid * (2 * HALF_EXTENT)
        for cx in range(grid):
            wx = -HALF_EXTENT + (cx + 0.5) / grid * (2 * HALF_EXTENT)
            verts.append((wx, wy, h[cy * grid + cx] * Z_SCALE))

    faces = []
    for cy in range(grid - 1):
        for cx in range(grid - 1):
            a = cy * grid + cx
            b = a + 1
            c = a + grid
            d = c + 1
            faces.append((a, b, d, c))

    mesh = bpy.data.meshes.new(f"{name}_terrain")
    mesh.from_pydata(verts, [], faces)
    mesh.update()
    obj = bpy.data.objects.new(f"{name}_terrain", mesh)
    bpy.context.collection.objects.link(obj)

    # Decimate to the mobile budget.
    tri_count = (grid - 1) * (grid - 1) * 2
    if tri_count > TARGET_TRIS:
        bpy.context.view_layer.objects.active = obj
        mod = obj.modifiers.new("decimate", "DECIMATE")
        mod.ratio = TARGET_TRIS / tri_count
        bpy.ops.object.modifier_apply(modifier="decimate")

    glb = OUT_DIR / f"{name}.terrain.glb"
    bpy.ops.export_scene.gltf(filepath=str(glb), export_format="GLB", use_selection=False)
    print(f"[terrain] {name}: {grid}x{grid} heightgrid → {glb.relative_to(REPO)} "
          f"(decimated {tri_count}→~{min(tri_count, TARGET_TRIS)} tris)")
    print("[terrain] RENDER-ONLY: this elevation never enters the sim (invariant #4).")


if __name__ == "__main__":
    build_mesh(arg_name())
