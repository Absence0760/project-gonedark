#!/usr/bin/env python3
# Going Dark — placeholder model generator (decisions.md D41).
#
# Builds the game's greybox/low-tier placeholder models from primitives in Blender and
# exports, per object, into ../../assets/models/:
#   - one `.glb`  — the standard interchange / source-of-record (two-view harness §4, tools)
#   - one `.mesh` — the COOKED runtime format the engine actually loads (decisions.md D44)
# plus a license manifest. These are deliberately blocky, intentional-looking placeholders —
# the "Claude can generate procedural & greybox content" lane of content-pipeline.md §6, NOT
# final/hero art.
#
# The `.mesh` is the cook step of content-pipeline.md §1 reduced to its greybox essentials: a
# trivially-parseable, Z-up, flat-shaded triangle soup (position + face normal) that the
# `gonedark-render` crate `include_bytes!`s and uploads straight to the GPU — no glTF/JSON parser
# on-device, no extra crate dependency. Format is documented in `render/src/mesh.rs`. The `.glb`
# stays the thing "we are using" (Blender source); the `.mesh` is its cooked runtime sibling. Both
# are committed under assets/models/ (the greybox tier is committed, per D41) — `/assets/cooked/`
# is reserved for the future heavyweight per-device ASTC/atlas/pak cook.
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
import struct
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
    if getattr(m, "node_tree", None) is None:  # 5.x materials already node-backed
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
    if len(parts) > 1:  # join() warns "No mesh data to join" on a single object
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


# --- cooked runtime mesh (.mesh) --------------------------------------------------------
# A dead-simple, little-endian, Z-up, flat-shaded triangle soup the engine loads directly
# (decisions.md D44). One vertex per triangle corner (no dedup) so each face carries its own
# flat normal — exactly the faceted greybox look we want, and the simplest possible parser.
#
#   magic   : 4 bytes  b"GDM1"
#   v_count : u32       number of vertices  (== 3 * triangle count)
#   i_count : u32       number of indices   (sequential 0..v_count for the soup)
#   verts   : v_count × [px,py,pz, nx,ny,nz]  f32  (24 bytes each)
#   indices : i_count × u32
#
# Coords are Z-up world metres with the base at z≈0 — matching the game's ground plane
# (`render/shader.wgsl` puts world XY on z=0, Z up). NOTE: the `.glb` exporter rewrites to
# glTF's +Y-up convention; the `.mesh` deliberately keeps Blender/​game Z-up. They describe the
# same geometry in each format's native up-axis. Keep this layout in lockstep with the parser
# in `render/src/mesh.rs` (`parse_mesh`) and its golden test.
MESH_MAGIC = b"GDM1"


def export_mesh(obj, filename):
    from mathutils import Vector

    mesh = obj.data
    mesh.calc_loop_triangles()
    verts = []  # flat f32 list: px,py,pz,nx,ny,nz per corner
    for tri in mesh.loop_triangles:
        # Flat shading: compute each triangle's own geometric normal from its vertices (the
        # CCW cross product) and share it across all three corners, so edges read as crisp
        # facets (the greybox aesthetic). Computing it here — rather than reading Blender's
        # cached polygon normal — guarantees a unit, perpendicular normal even after the
        # non-uniform `dimensions` scale bakes a skewed normal into that cache.
        co = [mesh.vertices[vi].co for vi in tri.vertices]
        n = (co[1] - co[0]).cross(co[2] - co[0])
        n = n.normalized() if n.length > 1e-9 else Vector((0.0, 0.0, 1.0))
        for c in co:
            verts.extend((c.x, c.y, c.z, n.x, n.y, n.z))
    v_count = len(mesh.loop_triangles) * 3
    assert v_count * 6 == len(verts), "expected 6 floats per vertex"

    path = os.path.join(OUT_DIR, filename)
    with open(path, "wb") as f:
        f.write(MESH_MAGIC)
        f.write(struct.pack("<II", v_count, v_count))  # i_count == v_count (sequential soup)
        f.write(struct.pack("<%df" % len(verts), *verts))
        f.write(struct.pack("<%dI" % v_count, *range(v_count)))
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
#
# Base colours are the single source of truth for each model's greybox tint. The `.mesh` is
# geometry-only (no colour), so the render crate mirrors these in `mesh.rs`'s `ModelKind` base
# colours — they are echoed into the manifest here so that mirror is auditable. A unit token's
# faction colour can still override its model tint at draw time (player blue / enemy red).
COLORS = {
    "trooper": (0.30, 0.34, 0.18),     # olive infantry
    "tank": (0.18, 0.22, 0.14),        # dark green armour
    "camp_hq": (0.45, 0.40, 0.30),     # tan structure
    "weapon_rifle": (0.12, 0.12, 0.13),  # gunmetal
    "crate": (0.40, 0.28, 0.16),       # wood cover prop
    "turret": (0.22, 0.24, 0.26),      # steel defensive emplacement
    "tree": (0.16, 0.30, 0.16),        # foliage greybox
    "rock": (0.40, 0.40, 0.42),        # grey boulder
    "barricade": (0.34, 0.30, 0.22),   # sandbag berm cover
}


def rgba(name):
    r, g, b = COLORS[name]
    return (r, g, b, 1.0)


def build_trooper():
    mat = make_material("trooper", rgba("trooper"))  # olive
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
    mat = make_material("tank", rgba("tank"))  # dark green
    parts = [
        box((3.0, 1.6, 0.70), (0, 0, 0.60)),                   # hull
        box((3.2, 0.45, 0.50), (0, 0.85, 0.35)),               # track R
        box((3.2, 0.45, 0.50), (0, -0.85, 0.35)),              # track L
        box((1.4, 1.2, 0.50), (-0.2, 0, 1.05)),                # turret
        cyl(0.10, 1.60, (1.2, 0, 1.05), rot=(0, math.radians(90), 0)),  # barrel
    ]
    return weld("tank", parts, mat)


def build_camp_hq():
    mat = make_material("camp_hq", rgba("camp_hq"))  # tan
    parts = [
        box((3.5, 3.0, 1.8), (0, 0, 0.90)),                    # walls
        pyramid(2.6, 1.2, (0, 0, 2.40)),                       # roof
        cyl(0.04, 1.40, (1.2, 1.0, 3.50)),                     # antenna
    ]
    return weld("camp_hq", parts, mat)


def build_weapon_rifle():
    mat = make_material("weapon_rifle", rgba("weapon_rifle"))  # gunmetal
    parts = [
        box((0.50, 0.06, 0.12), (0, 0, 0)),                    # receiver/body
        cyl(0.02, 0.40, (0.35, 0, 0), rot=(0, math.radians(90), 0)),  # barrel
        box((0.06, 0.05, 0.18), (-0.02, 0, -0.13)),            # magazine
        box((0.18, 0.05, 0.10), (-0.32, 0, 0.0)),              # stock
        box((0.06, 0.05, 0.14), (-0.10, 0, -0.10), rot=(0, math.radians(-12), 0)),  # grip
    ]
    return weld("weapon_rifle", parts, mat)


def build_crate():
    mat = make_material("crate", rgba("crate"))  # wood — low cover prop
    return weld("crate", [box((1.0, 1.0, 1.0), (0, 0, 0.50))], mat)


def build_turret():
    mat = make_material("turret", rgba("turret"))  # steel defensive emplacement
    parts = [
        box((1.6, 1.6, 0.40), (0, 0, 0.20)),                   # base pad
        cyl(0.55, 0.70, (0, 0, 0.70)),                         # rotating drum
        box((0.70, 0.70, 0.45), (0, 0, 1.15)),                 # gun housing
        cyl(0.07, 1.20, (0.75, 0, 1.15), rot=(0, math.radians(90), 0)),  # barrel
    ]
    return weld("turret", parts, mat)


def build_tree():
    mat = make_material("tree", rgba("tree"))  # foliage greybox (single material)
    parts = [
        cyl(0.16, 1.40, (0, 0, 0.70), verts=8),                # trunk
        sphere(0.95, (0, 0, 1.90), segments=10, rings=6),      # lower canopy
        sphere(0.65, (0, 0, 2.70), segments=10, rings=6),      # upper canopy
    ]
    return weld("tree", parts, mat)


def build_rock():
    mat = make_material("rock", rgba("rock"))  # grey boulder
    # A low-poly sphere squashed and faceted into a boulder — flat-shaded facets read as stone.
    o = sphere(0.90, (0, 0, 0.55), segments=10, rings=6)
    o.dimensions = (1.80, 1.50, 1.10)  # squash to a boulder, base near z=0
    return weld("rock", [o], mat)


def build_barricade():
    mat = make_material("barricade", rgba("barricade"))  # sandbag berm cover
    # A stepped sandbag berm: a wide low course with a narrower course stacked on top.
    parts = [
        box((2.40, 0.70, 0.45), (0, 0, 0.225)),                # lower course
        box((2.00, 0.55, 0.40), (0, 0, 0.625)),                # upper course
    ]
    return weld("barricade", parts, mat)


MODELS = [
    ("trooper", build_trooper,
     "Greybox infantry unit — boxy humanoid (hips/torso/head/limbs)."),
    ("tank", build_tank,
     "Greybox vehicle unit — hull, tracks, turret, barrel."),
    ("camp_hq", build_camp_hq,
     "Greybox structure — walled building with a pyramid roof + antenna."),
    ("weapon_rifle", build_weapon_rifle,
     "First-person weapon viewmodel — receiver, barrel, magazine, stock, grip."),
    ("crate", build_crate,
     "Cover prop — a 1m crate."),
    ("turret", build_turret,
     "Defensive structure — base pad, rotating drum, gun housing + barrel."),
    ("tree", build_tree,
     "Scenery / soft cover — trunk with a two-tier canopy."),
    ("rock", build_rock,
     "Scenery / hard cover — a faceted low-poly boulder."),
    ("barricade", build_barricade,
     "Cover prop — a stepped two-course sandbag berm."),
]


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    entries = []
    for stem, builder, description in MODELS:
        reset_scene()
        obj = builder()
        # `.glb` (interchange source) + `.mesh` (cooked runtime format the engine loads, D44).
        glb_path = export_glb(obj, stem + ".glb")
        mesh_path = export_mesh(obj, stem + ".mesh")
        entries.append({
            "name": stem,
            "file": stem + ".glb",
            "cooked": stem + ".mesh",
            "description": description,
            "base_color": [round(c, 4) for c in COLORS[stem]],
            "source": "procedural (Blender bpy — tools/models/gen_models.py)",
            "generator": bpy.app.version_string,
            "author": AUTHOR,
            "license": LICENSE,
            "url": "",
            "bytes": os.path.getsize(glb_path),
            "sha256": sha256(glb_path),
            "cooked_bytes": os.path.getsize(mesh_path),
            "cooked_sha256": sha256(mesh_path),
        })
        print(f"  wrote {stem}.glb ({entries[-1]['bytes']} B) "
              f"+ {stem}.mesh ({entries[-1]['cooked_bytes']} B)")

    manifest = {
        "note": (
            "Placeholder greybox models, generated by tools/models/gen_models.py "
            "(decisions.md D41). Each ships a `.glb` (interchange source) and a cooked `.mesh` "
            "the engine loads directly (decisions.md D44). Render-only; regenerate with "
            "`pnpm assets:models`. License-clean by construction — code-authored primitives, "
            "CC0-1.0 (content-pipeline.md §3). Honest weak axis: eye-level FPS credibility (§4)."
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
