#!/usr/bin/env python3
# Going Dark — placeholder model generator (decisions.md D41).
#
# Builds the game's greybox/low-tier placeholder models from primitives in Blender and
# exports one `.glb` per object into ../../assets/models/, plus a license manifest. These
# are deliberately blocky, intentional-looking placeholders — the "Claude can generate
# procedural & greybox content" lane of content-pipeline.md §6, NOT final/hero art.
#
# This file is a Blender Python (`bpy`) script — it is NOT importable as plain CPython.
# Run it headless:
#
#     blender --background --python tools/models/gen_models.py
#     # or:  pnpm assets:models
#
# Output is license-clean by construction: code-authored geometry from primitives has no
# third-party tool terms to vet, so every manifest entry is CC0-1.0 (content-pipeline.md §3).

import bpy
import os
import json
import math
import hashlib

# --- where to write -------------------------------------------------------------------
try:
    HERE = os.path.dirname(os.path.abspath(__file__))
except NameError:  # pragma: no cover — __file__ is set when run via --python
    HERE = os.getcwd()
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
OUT_DIR = os.path.join(REPO, "assets", "models")

AUTHOR = os.environ.get("GONEDARK_ASSET_AUTHOR", "Jared Howard")
LICENSE = "CC0-1.0"


# --- scene + primitive helpers --------------------------------------------------------
def reset_scene():
    """Empty the scene and purge orphan mesh/material data so runs are repeatable."""
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for block in (bpy.data.meshes, bpy.data.materials, bpy.data.objects):
        for item in list(block):
            if getattr(item, "users", 0) == 0:
                block.remove(item)


def make_material(name, rgba):
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = rgba
    bsdf.inputs["Roughness"].default_value = 0.85
    return m


def box(dims, loc, rot=(0, 0, 0)):
    bpy.ops.mesh.primitive_cube_add(size=1.0, location=loc, rotation=rot)
    o = bpy.context.active_object
    o.dimensions = dims  # sets scale to hit this bounding box
    return o


def cyl(radius, depth, loc, rot=(0, 0, 0), verts=16):
    bpy.ops.mesh.primitive_cylinder_add(
        radius=radius, depth=depth, location=loc, rotation=rot, vertices=verts
    )
    return bpy.context.active_object


def sphere(radius, loc, segments=16, rings=8):
    bpy.ops.mesh.primitive_uv_sphere_add(
        radius=radius, location=loc, segments=segments, ring_count=rings
    )
    return bpy.context.active_object


def pyramid(base, height, loc, rot=(0, 0, 0)):
    # A 4-vertex cone is a square pyramid; rotate 45° in Z to square it to the walls.
    bpy.ops.mesh.primitive_cone_add(
        radius1=base, radius2=0.0, depth=height, location=loc,
        rotation=(rot[0], rot[1], rot[2] + math.radians(45)), vertices=4,
    )
    return bpy.context.active_object


def weld(name, parts, material):
    """Apply each part's transform, join into one mesh, assign a single material."""
    for o in parts:
        bpy.ops.object.select_all(action="DESELECT")
        o.select_set(True)
        bpy.context.view_layer.objects.active = o
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    bpy.ops.object.select_all(action="DESELECT")
    for o in parts:
        o.select_set(True)
    bpy.context.view_layer.objects.active = parts[0]
    bpy.ops.object.join()
    obj = bpy.context.active_object
    obj.name = name
    obj.data.materials.clear()
    obj.data.materials.append(material)
    return obj


def export_glb(obj, filename):
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    path = os.path.join(OUT_DIR, filename)
    bpy.ops.export_scene.gltf(
        filepath=path, export_format="GLB", use_selection=True, export_apply=True
    )
    return path


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


# --- the models -----------------------------------------------------------------------
# Conventions: Z-up, feet/base at z≈0, sizes roughly in metres. Each builder returns a
# single welded object; `description` feeds the manifest + the two-view filter (§4) notes.
def build_trooper():
    mat = make_material("trooper", (0.30, 0.34, 0.18, 1.0))  # olive
    parts = [
        box((0.40, 0.24, 0.20), (0, 0, 0.75)),                 # hips
        box((0.45, 0.25, 0.70), (0, 0, 1.10)),                 # torso
        sphere(0.16, (0, 0, 1.58)),                            # head
        cyl(0.09, 0.70, (0.12, 0, 0.35)),                      # leg R
        cyl(0.09, 0.70, (-0.12, 0, 0.35)),                     # leg L
        cyl(0.07, 0.60, (0.28, 0, 1.10), rot=(math.radians(8), 0, 0)),   # arm R
        cyl(0.07, 0.60, (-0.28, 0, 1.10), rot=(math.radians(8), 0, 0)),  # arm L
    ]
    return weld("trooper", parts, mat)


def build_tank():
    mat = make_material("tank", (0.18, 0.22, 0.14, 1.0))  # dark green
    parts = [
        box((3.0, 1.6, 0.70), (0, 0, 0.60)),                   # hull
        box((3.2, 0.45, 0.50), (0, 0.85, 0.35)),               # track R
        box((3.2, 0.45, 0.50), (0, -0.85, 0.35)),              # track L
        box((1.4, 1.2, 0.50), (-0.2, 0, 1.05)),                # turret
        cyl(0.10, 1.60, (1.2, 0, 1.05), rot=(0, math.radians(90), 0)),  # barrel
    ]
    return weld("tank", parts, mat)


def build_camp_hq():
    mat = make_material("camp_hq", (0.45, 0.40, 0.30, 1.0))  # tan
    parts = [
        box((3.5, 3.0, 1.8), (0, 0, 0.90)),                    # walls
        pyramid(2.6, 1.2, (0, 0, 2.40)),                       # roof
        cyl(0.04, 1.40, (1.2, 1.0, 3.50)),                     # antenna
    ]
    return weld("camp_hq", parts, mat)


def build_weapon_rifle():
    mat = make_material("weapon_rifle", (0.12, 0.12, 0.13, 1.0))  # gunmetal
    parts = [
        box((0.50, 0.06, 0.12), (0, 0, 0)),                    # receiver/body
        cyl(0.02, 0.40, (0.35, 0, 0), rot=(0, math.radians(90), 0)),  # barrel
        box((0.06, 0.05, 0.18), (-0.02, 0, -0.13)),            # magazine
        box((0.18, 0.05, 0.10), (-0.32, 0, 0.0)),              # stock
        box((0.06, 0.05, 0.14), (-0.10, 0, -0.10), rot=(0, math.radians(-12), 0)),  # grip
    ]
    return weld("weapon_rifle", parts, mat)


def build_crate():
    mat = make_material("crate", (0.40, 0.28, 0.16, 1.0))  # wood — low cover prop
    return weld("crate", [box((1.0, 1.0, 1.0), (0, 0, 0.50))], mat)


MODELS = [
    ("trooper.glb", build_trooper,
     "Greybox infantry unit — boxy humanoid (hips/torso/head/limbs)."),
    ("tank.glb", build_tank,
     "Greybox vehicle unit — hull, tracks, turret, barrel."),
    ("camp_hq.glb", build_camp_hq,
     "Greybox structure — walled building with a pyramid roof + antenna."),
    ("weapon_rifle.glb", build_weapon_rifle,
     "First-person weapon viewmodel — receiver, barrel, magazine, stock, grip."),
    ("crate.glb", build_crate,
     "Cover prop — a 1m crate."),
]


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    entries = []
    for filename, builder, description in MODELS:
        reset_scene()
        obj = builder()
        path = export_glb(obj, filename)
        entries.append({
            "file": filename,
            "description": description,
            "source": "procedural (Blender bpy — tools/models/gen_models.py)",
            "generator": bpy.app.version_string,
            "author": AUTHOR,
            "license": LICENSE,
            "url": "",
            "bytes": os.path.getsize(path),
            "sha256": sha256(path),
        })
        print(f"  wrote {filename}  ({entries[-1]['bytes']} bytes)")

    manifest = {
        "note": (
            "Placeholder greybox models, generated by tools/models/gen_models.py "
            "(decisions.md D41). Render-only; regenerate with `pnpm assets:models`. "
            "License-clean by construction — code-authored primitives, CC0-1.0 "
            "(content-pipeline.md §3). Honest weak axis: eye-level FPS credibility (§4)."
        ),
        "license_default": LICENSE,
        "assets": entries,
    }
    manifest_path = os.path.join(OUT_DIR, "manifest.json")
    with open(manifest_path, "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"  wrote manifest.json  ({len(entries)} assets)")


if __name__ == "__main__":
    main()
