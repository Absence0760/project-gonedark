#!/usr/bin/env python3
# Going Dark — placeholder model generator (decisions.md D41).
#
# Builds the game's greybox/low-tier placeholder models from primitives in Blender and
# exports, per object, into a category subfolder under ../../assets/models/ (units/, structures/,
# weapons/, props/, fx/ — see CATEGORY), as a small LOD chain:
#   - LOD0 (full detail) — one `.glb` (interchange / source-of-record, two-view harness §4)
#     and one `.mesh` (the COOKED runtime format the engine actually loads, decisions.md D44)
#   - LOD1/LOD2 (decimated) — `<name>.lod1.glb`/`.lod1.mesh` (and `.lod2.*`), produced by
#     running `gltfpack -si … -sa` over the glb to simplify the geometry, then re-importing
#     the simplified glb and running the SAME `.mesh` cook on it — so every tier is the
#     identical GDM1 format with freshly recomputed flat normals. The renderer picks a tier
#     by on-screen size/distance (top-down tokens use a low tier; the embodied view uses LOD0).
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
import shutil
import subprocess

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


def icosphere(radius, loc, subdivisions=1):
    """A faceted icosphere — deterministic, run-to-run reproducible (unlike the UV sphere whose
    pole/seam tessellation wobbles between runs). Its triangular facets flat-shade into crisp,
    chunky chips, so it's the right primitive for hard greybox scenery (rock) and rounded helmets."""
    bpy.ops.mesh.primitive_ico_sphere_add(
        radius=radius, location=loc, subdivisions=subdivisions
    )
    return bpy.context.active_object


def cone(base, top, depth, loc, rot=(0, 0, 0), verts=8):
    """A (truncated) cone — `top` > 0 gives a frustum, `top` = 0 a point. Deterministic. Used for
    stylized conifer tiers and tapered trunks; a low `verts` keeps the facet read chunky and the
    triangle count lean for the mobile / 200-unit budget."""
    bpy.ops.mesh.primitive_cone_add(
        radius1=base, radius2=top, depth=depth, location=loc, rotation=rot, vertices=verts
    )
    return bpy.context.active_object


def pyramid(base, height, loc, rot=(0, 0, 0)):
    # A 4-vertex cone is a square pyramid; rotate 45° in Z to square it to the walls.
    bpy.ops.mesh.primitive_cone_add(
        radius1=base, radius2=0.0, depth=height, location=loc,
        rotation=(rot[0], rot[1], rot[2] + math.radians(45)), vertices=4,
    )
    return bpy.context.active_object


def chamfer(obj, width, segments=1, angle_deg=40.0):
    """Apply an angle-limited bevel modifier so the model's hard silhouette edges read as a
    deliberate machined/cast chamfer instead of a raw razor-sharp primitive edge — the single
    cheapest lift from "stack of cubes" to "intentional greybox".

    The `ANGLE` limit means only edges sharper than `angle_deg` get beveled, so the flat coplanar
    faces of a box are left untouched (no wasted geometry, no shading seams) while every corner
    and silhouette crease is softened. `clamp_overlap` caps the width per-edge to half the shortest
    adjacent edge, so a thin part (a rifle barrel, a track guard) auto-shrinks its chamfer instead
    of self-intersecting — one global `width` is therefore safe across very different part scales.
    `segments=1` keeps it a single flat chamfer face (faceted, on-aesthetic, and tri-cheap). The
    modifier is applied immediately so `export_mesh` sees real geometry and recomputes flat
    normals from it."""
    if width <= 0.0:
        return obj
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    m = obj.modifiers.new("chamfer", "BEVEL")
    m.width = width
    m.segments = segments
    m.limit_method = "ANGLE"
    m.angle_limit = math.radians(angle_deg)
    m.use_clamp_overlap = True  # Blender 5.x name (was `clamp_overlap` pre-4.x)
    bpy.ops.object.modifier_apply(modifier=m.name)
    return obj


def weld(name, parts, material, bevel=0.0):
    """Apply each part's transform, join into one mesh, assign a single material. `bevel` (metres)
    applies an angle-limited `chamfer` to the welded result — soft silhouette edges per model."""
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
    chamfer(obj, bevel)
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


def mesh_tris(path):
    """Triangle count of a cooked `.mesh` — read straight from the GDM1 header. The soup is
    3 verts per triangle (one corner each, no dedup), so `tris == v_count / 3`."""
    with open(path, "rb") as f:
        head = f.read(12)
    assert head[0:4] == MESH_MAGIC, f"{path} is not a GDM1 mesh"
    v_count = struct.unpack("<I", head[4:8])[0]
    return v_count // 3


# --- LOD chain (gltfpack decimation → re-import → re-cook) -------------------------------
# The full-detail tier (LOD0) is the unchanged `.glb`+`.mesh` pair above. Each decimated tier
# is gltfpack-simplified geometry re-cooked back through `export_mesh`, so every tier lands in
# the identical GDM1 format. We run gltfpack with:
#   -sa  : aggressively hit the target ratio *across attribute discontinuities*. Our `.mesh`
#          (and the exported glb) is a flat-shaded soup — adjacent faces don't share normals,
#          so a plain `-si` finds almost no collapsible edges and reduces nothing. `-sa` welds
#          across those seams to actually decimate (quality is secondary for a distance LOD).
#   -noq : emit plain float glTF (no KHR_mesh_quantization / meshopt extension), so the
#          re-import is trivially lossless to read back — and we recompute our own flat normals
#          on the cook anyway, so gltfpack's normals are irrelevant.
# LOD2 is chained off LOD1's glb (not the source) so the pyramid is monotone by construction —
# simplification never *adds* triangles, so tris(LOD2) <= tris(LOD1) <= tris(LOD0) always.
GLTFPACK = shutil.which("gltfpack") or "gltfpack"

# (level, filename suffix, ratio passed to gltfpack for THIS step, cumulative ratio vs LOD0)
LOD_TIERS = [
    (1, ".lod1", 0.5, 0.5),
    (2, ".lod2", 0.5, 0.25),
]


def run_gltfpack(src_filename, dst_filename, ratio):
    """Simplify `src_filename` → `dst_filename` (both under OUT_DIR) at triangle ratio `ratio`."""
    src = os.path.join(OUT_DIR, src_filename)
    dst = os.path.join(OUT_DIR, dst_filename)
    subprocess.run(
        [GLTFPACK, "-i", src, "-o", dst, "-si", str(ratio), "-sa", "-noq"],
        check=True, capture_output=True, text=True,
    )
    return dst


def import_glb(filename):
    """Import a (gltfpack-simplified) glb and return one welded, Z-up mesh object.

    Blender's glTF importer carries glTF's +Y-up convention as an *object rotation* rather than
    baking it into the vertex data, so we `transform_apply` it down — that restores the same
    Z-up coordinates the LOD0 cook used, keeping every tier in one convention."""
    path = os.path.join(OUT_DIR, filename)
    bpy.ops.import_scene.gltf(filepath=path)
    meshes = [o for o in bpy.context.selected_objects if o.type == "MESH"]
    assert meshes, f"no mesh imported from {filename}"
    bpy.ops.object.select_all(action="DESELECT")
    for o in meshes:
        o.select_set(True)
    bpy.context.view_layer.objects.active = meshes[0]
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    if len(meshes) > 1:  # join() warns "No mesh data to join" on a single object
        bpy.ops.object.join()
    return bpy.context.active_object


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
    "tank": (0.18, 0.22, 0.14),        # dark green armour (hull)
    "tank_turret": (0.18, 0.22, 0.14), # dark green armour (turret — matches the hull)
    "camp_hq": (0.45, 0.40, 0.30),     # tan structure
    "weapon_rifle": (0.12, 0.12, 0.13),  # gunmetal
    "crate": (0.40, 0.28, 0.16),       # wood cover prop
    "turret": (0.22, 0.24, 0.26),      # steel defensive emplacement
    "tree": (0.16, 0.30, 0.16),        # foliage greybox
    "rock": (0.40, 0.40, 0.42),        # grey boulder
    "barricade": (0.34, 0.30, 0.22),   # sandbag berm cover
    "tracer": (1.00, 0.60, 0.20),      # hot orange shell tracer (renderer adds the glow)
    # --- Faction cosmetic silhouettes (factions-plan WS-C, D68). Presentation-only: per-army
    # silhouettes/names never reach `core` and add no checksum surface. The geometry carries the
    # read; the tint reinforces it. US = NATO olive/CARC grey-green; FR = French army green. ---
    "trooper_us": (0.30, 0.34, 0.18),       # US infantry — olive (OCP era)
    "trooper_fr": (0.27, 0.31, 0.20),       # FR infantry — French army green
    "tank_us": (0.30, 0.31, 0.24),          # M1 Abrams — CARC tan/grey-green hull
    "tank_turret_us": (0.30, 0.31, 0.24),   # Abrams turret — matches the hull
    "tank_fr": (0.22, 0.27, 0.18),          # Leclerc — darker French green hull
    "tank_turret_fr": (0.22, 0.27, 0.18),   # Leclerc turret — matches the hull
    "weapon_rifle_us": (0.12, 0.12, 0.13),  # M4 carbine — gunmetal
    "weapon_rifle_fr": (0.13, 0.13, 0.12),  # FAMAS bullpup — warmer gunmetal
}


# Category subfolder each model is written into under assets/models/ — keeps the asset tree
# browsable by role instead of one flat dump. The renderer's `include_bytes!` paths in
# `render/src/mesh.rs` and the manifest's per-tier `file`/`cooked` paths both carry this prefix,
# so a model's category here is its on-disk home everywhere. Adding a model? Give it a category.
CATEGORY = {
    "trooper": "units",
    "tank": "units",
    "tank_turret": "units",
    "camp_hq": "structures",
    "turret": "structures",
    "barricade": "structures",
    "weapon_rifle": "weapons",
    "crate": "props",
    "tree": "props",
    "rock": "props",
    "tracer": "fx",
    # Faction cosmetic silhouettes (WS-C) — same role-based categories as their shared kin.
    "trooper_us": "units",
    "trooper_fr": "units",
    "tank_us": "units",
    "tank_turret_us": "units",
    "tank_fr": "units",
    "tank_turret_fr": "units",
    "weapon_rifle_us": "weapons",
    "weapon_rifle_fr": "weapons",
}


def relpath(stem, suffix):
    """Category-relative path for a model file, e.g. ('trooper', '.lod1.glb') → 'units/trooper.lod1.glb'.
    Always forward-slashed so the strings written into manifest.json are stable across platforms."""
    return CATEGORY[stem] + "/" + stem + suffix


def rgba(name):
    r, g, b = COLORS[name]
    return (r, g, b, 1.0)


def build_trooper():
    mat = make_material("trooper", rgba("trooper"))  # olive
    # Boxy humanoid, but with the silhouette tells that read as a soldier even at eye level: a
    # rounded combat helmet capping the head, a chest-plate slab over the torso, and a backpack
    # behind. Limbs taper (shoulders > forearms, thighs > calves) so it stops reading as a coat-rack.
    parts = [
        box((0.40, 0.24, 0.20), (0, 0, 0.75)),                 # hips
        box((0.45, 0.26, 0.46), (0, 0, 0.98)),                 # lower torso
        box((0.48, 0.30, 0.30), (0, -0.01, 1.30)),             # chest (plate carrier — bulkier, deeper)
        box((0.36, 0.14, 0.30), (0, 0.20, 1.28)),              # front plate slab
        box((0.30, 0.20, 0.40), (0, -0.20, 1.18)),             # backpack
        sphere(0.15, (0, 0, 1.56), segments=10, rings=6),      # head
        icosphere(0.18, (0, 0, 1.64), subdivisions=1),         # rounded combat helmet (faceted dome)
        box((0.34, 0.30, 0.06), (0, 0.02, 1.71)),              # helmet brow / NVG-mount slab
        cyl(0.10, 0.40, (0.12, 0, 0.55), verts=10),            # thigh R
        cyl(0.10, 0.40, (-0.12, 0, 0.55), verts=10),           # thigh L
        cyl(0.08, 0.40, (0.12, 0, 0.18), verts=10),            # calf R
        cyl(0.08, 0.40, (-0.12, 0, 0.18), verts=10),           # calf L
        box((0.16, 0.18, 0.10), (0.12, 0.04, 0.02)),           # boot R
        box((0.16, 0.18, 0.10), (-0.12, 0.04, 0.02)),          # boot L
        cyl(0.08, 0.34, (0.30, 0, 1.30), verts=10),            # upper arm R
        cyl(0.08, 0.34, (-0.30, 0, 1.30), verts=10),           # upper arm L
        cyl(0.06, 0.36, (0.30, 0.04, 0.96), rot=(math.radians(14), 0, 0), verts=10),   # forearm R
        cyl(0.06, 0.36, (-0.30, 0.04, 0.96), rot=(math.radians(14), 0, 0), verts=10),  # forearm L
    ]
    return weld("trooper", parts, mat, bevel=0.02)


def build_tank():
    # The tank HULL (chassis + tracks) only — the turret is a SEPARATE model
    # (`build_tank_turret`) so the renderer can slew it independently of the hull (tank
    # embodiment P7, D55). Both keep the dark-green armour tint. The turret-ring pivot sits at
    # the hull's local origin (x=0, y=0), so a turret drawn at the same world (x, y) and lifted
    # to z≈hull-top rotates about that ring exactly.
    mat = make_material("tank", rgba("tank"))  # dark green
    parts = [
        box((3.0, 1.6, 0.55), (0, 0, 0.62)),                   # upper hull
        box((0.9, 1.6, 0.34), (1.35, 0, 0.52), rot=(0, math.radians(22), 0)),  # sloped front glacis
        box((3.2, 0.45, 0.50), (0, 0.85, 0.35)),               # track R
        box((3.2, 0.45, 0.50), (0, -0.85, 0.35)),              # track L
        box((3.3, 0.50, 0.12), (0, 0.85, 0.62)),               # track guard / fender R
        box((3.3, 0.50, 0.12), (0, -0.85, 0.62)),              # track guard / fender L
    ]
    # Road wheels: faceted drums proud of each track — break the slab side into a running-gear read.
    for side in (0.85, -0.85):
        for i, x in enumerate((-1.1, -0.55, 0.0, 0.55, 1.1)):
            parts.append(cyl(0.22, 0.12, (x, side, 0.24),
                             rot=(math.radians(90), 0, 0), verts=10))
    return weld("tank", parts, mat, bevel=0.05)


def build_tank_turret():
    # The tank TURRET (gun mantlet + barrel) as its own model so it can yaw independently of the
    # hull (P7). Modelled in the hull's local frame: the turret-ring pivot is the local origin
    # (x=0, y=0) about which the renderer rotates by `turret_yaw`, and the geometry keeps its real
    # height (z≈1.05, sitting on the hull top at z≈0.95). Drawing it at the hull's world (x, y) with
    # yaw = turret_yaw therefore slews it about the ring. Barrel points +X (turret_yaw 0 == hull 0).
    mat = make_material("tank_turret", rgba("tank_turret"))  # dark green (matches the hull)
    parts = [
        box((1.4, 1.2, 0.50), (-0.2, 0, 1.05)),                # turret box (centred behind the ring)
        box((0.55, 1.0, 0.34), (0.45, 0, 1.02), rot=(0, math.radians(-14), 0)),  # sloped gun mantlet
        box((0.9, 1.3, 0.22), (-0.45, 0, 0.98)),               # rear stowage bustle (overhangs)
        cyl(0.22, 0.16, (-0.45, 0.0, 1.38), verts=12),         # commander's cupola
        cyl(0.10, 1.60, (1.2, 0, 1.05), rot=(0, math.radians(90), 0)),  # barrel, forward along +X
        cyl(0.13, 0.20, (0.55, 0, 1.05), rot=(0, math.radians(90), 0), verts=12),  # bore-evacuator collar
    ]
    return weld("tank_turret", parts, mat, bevel=0.04)


def build_tracer():
    # A tank-shell tracer: a small bolt elongated along +X (its travel axis), centred on the local
    # origin so the renderer can place it at the shell's (x, y, height) and yaw it by the velocity
    # heading (tank embodiment P7, D55). Deliberately tiny — it reads as a glowing round in flight,
    # not a model; the renderer drives a hot emissive tint per-instance, so the base colour is only a
    # fallback.
    mat = make_material("tracer", rgba("tracer"))  # hot orange
    # A short body with a pointed nose cone along +X (the travel axis) — reads as a round in flight
    # rather than a brick. The renderer drives the emissive glow; geometry just needs the heading.
    parts = [
        box((0.42, 0.12, 0.12), (-0.09, 0, 0)),                          # body
        cone(0.085, 0.0, 0.24, (0.24, 0, 0), rot=(0, math.radians(90), 0), verts=8),  # nose cone (+X)
    ]
    return weld("tracer", parts, mat, bevel=0.015)


def build_camp_hq():
    mat = make_material("camp_hq", rgba("camp_hq"))  # tan
    parts = [
        box((3.5, 3.0, 1.8), (0, 0, 0.90)),                    # walls
        box((3.7, 3.2, 0.18), (0, 0, 1.80)),                   # eave / cornice band (roofline lip)
        pyramid(2.6, 1.2, (0, 0, 2.40)),                       # roof
        box((1.0, 0.20, 1.10), (0, 1.50, 0.55)),               # door frame (front face)
        box((0.70, 0.10, 0.55), (-1.0, 1.50, 1.10)),           # window slab (front face)
        box((0.70, 0.10, 0.55), (1.0, 1.50, 1.10)),            # window slab (front face)
        box((0.40, 0.40, 0.50), (0, 0, 3.00)),                 # rooftop vent housing under the mast
        cyl(0.04, 1.40, (1.2, 1.0, 3.50)),                     # antenna
        cyl(0.10, 0.30, (1.2, 1.0, 2.95), verts=8),            # antenna base
    ]
    return weld("camp_hq", parts, mat, bevel=0.06)


def build_weapon_rifle():
    mat = make_material("weapon_rifle", rgba("weapon_rifle"))  # gunmetal
    # The eye-level hero prop — gets the most silhouette care: flat-top rail, ribbed handguard,
    # front sight post, a canted magazine and a real grip+stock. Receiver at origin, barrel +X.
    parts = [
        box((0.46, 0.06, 0.11), (0.0, 0, 0)),                  # upper/lower receiver
        box((0.40, 0.05, 0.035), (0.0, 0, 0.082)),             # flat-top picatinny rail
        cyl(0.03, 0.20, (0.26, 0, -0.01), rot=(0, math.radians(90), 0), verts=10),  # handguard
        cyl(0.018, 0.46, (0.42, 0, 0), rot=(0, math.radians(90), 0), verts=10),  # barrel
        box((0.02, 0.03, 0.07), (0.40, 0, 0.05)),              # front sight post
        cyl(0.035, 0.06, (0.64, 0, 0), rot=(0, math.radians(90), 0), verts=10),  # muzzle device
        box((0.07, 0.05, 0.20), (-0.02, 0, -0.14)),            # magazine (curved STANAG read)
        box((0.20, 0.05, 0.085), (-0.32, 0, 0.0)),             # collapsible stock
        box((0.06, 0.045, 0.05), (-0.20, 0, 0.05)),            # cheek riser
        box((0.06, 0.05, 0.14), (-0.10, 0, -0.10), rot=(0, math.radians(-14), 0)),  # grip
    ]
    return weld("weapon_rifle", parts, mat, bevel=0.006)


def build_crate():
    mat = make_material("crate", rgba("crate"))  # wood — low cover prop
    # Slatted shipping crate: a core box, four proud corner posts, and a mid-height banding course
    # so it reads as built planks instead of a featureless 1 m cube. Bevel chamfers every edge.
    parts = [box((0.94, 0.94, 1.0), (0, 0, 0.50))]            # core
    for sx in (-1, 1):
        for sy in (-1, 1):
            parts.append(box((0.10, 0.10, 1.0), (sx * 0.47, sy * 0.47, 0.50)))  # corner post
    parts += [
        box((1.02, 1.02, 0.10), (0, 0, 0.30)),                 # lower banding course
        box((1.02, 1.02, 0.10), (0, 0, 0.70)),                 # upper banding course
        box((1.0, 1.0, 0.08), (0, 0, 1.0)),                    # lid rim
    ]
    return weld("crate", parts, mat, bevel=0.03)


def build_turret():
    mat = make_material("turret", rgba("turret"))  # steel defensive emplacement
    parts = [
        box((1.6, 1.6, 0.40), (0, 0, 0.20)),                   # base pad
        box((1.2, 1.2, 0.14), (0, 0, 0.47)),                   # bolted ring plate on the pad
        cyl(0.55, 0.70, (0, 0, 0.70), verts=12),               # rotating drum
        box((0.70, 0.80, 0.45), (0, 0, 1.15)),                 # gun housing
        box((0.55, 0.95, 0.50), (0.42, 0, 1.18)),              # gun shield (faces +X with the barrel)
        box((0.34, 0.30, 0.26), (-0.28, 0.34, 1.20)),          # ammo box (side)
        cyl(0.07, 1.20, (0.75, 0, 1.15), rot=(0, math.radians(90), 0), verts=10),  # barrel
        cyl(0.10, 0.18, (0.30, 0, 1.15), rot=(0, math.radians(90), 0), verts=10),  # barrel shroud
    ]
    return weld("turret", parts, mat, bevel=0.03)


def build_tree():
    mat = make_material("tree", rgba("tree"))  # foliage greybox (single material)
    # A stylized low-poly conifer: a tapered trunk plus three stacked cone tiers of decreasing
    # radius. Cones/cylinders are deterministic (the old two-UV-sphere canopy varied run-to-run);
    # the stacked tiers give a far more intentional, readable silhouette than two blobs.
    parts = [
        cone(0.20, 0.12, 1.40, (0, 0, 0.70), verts=8),         # tapered trunk
        cone(1.00, 0.0, 1.30, (0, 0, 1.70), verts=10),         # lower skirt tier
        cone(0.78, 0.0, 1.20, (0, 0, 2.35), verts=10),         # mid tier
        cone(0.52, 0.0, 1.00, (0, 0, 3.00), verts=10),         # crown tier
    ]
    return weld("tree", parts, mat, bevel=0.0)


def build_rock():
    mat = make_material("rock", rgba("rock"))  # grey boulder
    # A cluster of two faceted icospheres (deterministic — the old UV sphere tessellated differently
    # each run), each squashed and offset so the silhouette is an irregular boulder, not a ball.
    # Flat-shaded triangular facets read as cleaved stone.
    main = icosphere(0.90, (0, 0, 0.55), subdivisions=1)
    main.dimensions = (1.80, 1.50, 1.05)               # squash to a boulder, base near z=0
    spur = icosphere(0.55, (0.55, -0.30, 0.40), subdivisions=1)
    spur.dimensions = (0.95, 0.80, 0.70)               # smaller offset lobe — breaks the symmetry
    return weld("rock", [main, spur], mat)


def build_barricade():
    mat = make_material("barricade", rgba("barricade"))  # sandbag berm cover
    # A stacked sandbag berm: discrete bags laid in two offset (running-bond) courses instead of two
    # smooth slabs, so the silhouette reads as a wall of bags. A heavy chamfer rounds each bag.
    parts = []
    lower_xs = [-1.0, -0.5, 0.0, 0.5, 1.0]
    upper_xs = [-0.75, -0.25, 0.25, 0.75]
    for x in lower_xs:
        parts.append(box((0.46, 0.70, 0.30), (x, 0, 0.15)))    # lower course bag
    for x in upper_xs:
        parts.append(box((0.46, 0.60, 0.28), (x, 0, 0.44)))    # upper course bag (offset = running bond)
    return weld("barricade", parts, mat, bevel=0.09)


# --- Faction cosmetic silhouettes (factions-plan WS-C, D68) -----------------------------------
# Per-army, presentation-only variants of the headline archetypes. The renderer maps
# `(Army, kind) → ModelKind` (render/src/lib.rs::model_for_unit) exactly as it maps bare `UnitKind`,
# so a US-side rifleman draws `trooper_us` and a French tank draws `tank_fr` (+ `tank_turret_fr`).
# These NEVER reach `core` — silhouettes/names add zero checksum surface (invariant #1/#7 untouched).
# Geometry stays in the SAME local frame as the shared kin it replaces (Z-up, base z≈0; tank barrel
# +X with the turret-ring pivot at the hull origin; rifle receiver at origin, barrel +X) so the
# existing placement/token math (`token_meshes`, `weapon_view_model`) works unchanged.


def build_trooper_us():
    # US infantry: ACH/ECH rounded helmet, plate-carrier bulked torso. Boxy greybox, olive.
    mat = make_material("trooper_us", rgba("trooper_us"))
    parts = [
        box((0.42, 0.26, 0.20), (0, 0, 0.75)),                 # hips
        box((0.50, 0.30, 0.72), (0, 0, 1.10)),                 # torso (plate carrier — bulkier)
        box((0.38, 0.16, 0.34), (0, 0.20, 1.22)),              # front plate slab
        box((0.32, 0.22, 0.42), (0, -0.22, 1.16)),             # backpack
        sphere(0.15, (0, 0, 1.58), segments=10, rings=6),      # head
        icosphere(0.19, (0, 0, 1.64), subdivisions=1),         # rounded ACH/ECH combat helmet
        box((0.40, 0.30, 0.07), (0, 0.02, 1.72)),              # helmet brow / NVG-mount slab
        cyl(0.10, 0.70, (0.13, 0, 0.35)),                      # leg R
        cyl(0.10, 0.70, (-0.13, 0, 0.35)),                     # leg L
        box((0.17, 0.20, 0.10), (0.13, 0.04, 0.02)),           # boot R
        box((0.17, 0.20, 0.10), (-0.13, 0.04, 0.02)),          # boot L
        cyl(0.08, 0.60, (0.30, 0, 1.10), rot=(math.radians(8), 0, 0)),   # arm R
        cyl(0.08, 0.60, (-0.30, 0, 1.10), rot=(math.radians(8), 0, 0)),  # arm L
    ]
    return weld("trooper_us", parts, mat, bevel=0.02)


def build_trooper_fr():
    # French infantry (FELIN): SPECTRA helmet (flatter, brimmed), slimmer profile, French green.
    mat = make_material("trooper_fr", rgba("trooper_fr"))
    parts = [
        box((0.40, 0.24, 0.20), (0, 0, 0.75)),                 # hips
        box((0.44, 0.26, 0.70), (0, 0, 1.10)),                 # torso (slimmer)
        box((0.30, 0.14, 0.32), (0, 0.18, 1.20)),              # front plate slab (slimmer)
        box((0.28, 0.18, 0.38), (0, -0.18, 1.16)),             # backpack
        sphere(0.15, (0, 0, 1.58), segments=10, rings=6),      # head
        cyl(0.18, 0.16, (0, 0, 1.66), verts=12),               # flatter SPECTRA helmet dome
        box((0.40, 0.18, 0.05), (0.0, 0.10, 1.62)),            # brim accent (forward)
        cyl(0.09, 0.70, (0.12, 0, 0.35)),                      # leg R
        cyl(0.09, 0.70, (-0.12, 0, 0.35)),                     # leg L
        box((0.16, 0.20, 0.10), (0.12, 0.04, 0.02)),           # boot R
        box((0.16, 0.20, 0.10), (-0.12, 0.04, 0.02)),          # boot L
        cyl(0.07, 0.62, (0.27, 0, 1.10), rot=(math.radians(8), 0, 0)),   # arm R
        cyl(0.07, 0.62, (-0.27, 0, 1.10), rot=(math.radians(8), 0, 0)),  # arm L
    ]
    return weld("trooper_fr", parts, mat, bevel=0.02)


def build_tank_us():
    # M1 Abrams HULL: long, low, flat — a broad chassis with a flat front glacis. Turret is the
    # separate `tank_us_turret` model (slews independently, P7). Pivot at local origin like `tank`.
    mat = make_material("tank_us", rgba("tank_us"))
    parts = [
        box((3.6, 1.9, 0.55), (0, 0, 0.55)),                   # long flat hull
        box((1.0, 1.9, 0.30), (1.55, 0, 0.45), rot=(0, math.radians(18), 0)),  # sloped front glacis
        box((3.7, 0.50, 0.55), (0, 1.00, 0.35)),               # track R (long)
        box((3.7, 0.50, 0.55), (0, -1.00, 0.35)),              # track L
        box((3.8, 0.56, 0.12), (0, 1.00, 0.62)),               # track guard / fender R
        box((3.8, 0.56, 0.12), (0, -1.00, 0.62)),              # track guard / fender L
    ]
    for side in (1.00, -1.00):
        for x in (-1.3, -0.78, -0.26, 0.26, 0.78, 1.3):
            parts.append(cyl(0.23, 0.12, (x, side, 0.24),
                             rot=(math.radians(90), 0, 0), verts=10))  # road wheel
    return weld("tank_us", parts, mat, bevel=0.05)


def build_tank_turret_us():
    # M1 Abrams TURRET: large, low, wide, angular slab with a long 120 mm gun. Pivot at hull origin,
    # barrel +X (turret_yaw 0 == hull 0); seated on the hull top (z≈0.85).
    mat = make_material("tank_turret_us", rgba("tank_turret_us"))
    parts = [
        box((2.0, 1.7, 0.55), (-0.15, 0, 1.05)),               # broad flat turret
        box((0.7, 1.7, 0.30), (1.05, 0, 1.05), rot=(0, math.radians(-12), 0)),  # sloped gun mantlet
        cyl(0.10, 2.10, (1.5, 0, 1.05), rot=(0, math.radians(90), 0)),  # long 120mm barrel, +X
        box((0.5, 0.5, 0.18), (-0.9, 0.5, 1.40)),              # commander's cupola/CITV
        cyl(0.13, 0.20, (0.45, 0, 1.05), rot=(0, math.radians(90), 0), verts=12),  # bore-evacuator collar
    ]
    return weld("tank_turret_us", parts, mat, bevel=0.04)


def build_tank_fr():
    # Leclerc HULL: more compact than the Abrams, cleaner sloped front. Separate turret model.
    mat = make_material("tank_fr", rgba("tank_fr"))
    parts = [
        box((3.0, 1.7, 0.60), (0, 0, 0.58)),                   # compact hull
        box((0.9, 1.7, 0.34), (1.35, 0, 0.50), rot=(0, math.radians(24), 0)),  # steeper glacis
        box((3.1, 0.46, 0.58), (0, 0.88, 0.36)),               # track R
        box((3.1, 0.46, 0.58), (0, -0.88, 0.36)),              # track L
        box((3.2, 0.52, 0.12), (0, 0.88, 0.64)),               # track guard / fender R
        box((3.2, 0.52, 0.12), (0, -0.88, 0.64)),              # track guard / fender L
    ]
    for side in (0.88, -0.88):
        for x in (-1.1, -0.55, 0.0, 0.55, 1.1):
            parts.append(cyl(0.22, 0.12, (x, side, 0.25),
                             rot=(math.radians(90), 0, 0), verts=10))  # road wheel
    return weld("tank_fr", parts, mat, bevel=0.05)


def build_tank_turret_fr():
    # Leclerc TURRET: cleaner, taller box with a prominent rear bustle (autoloader) — a distinctly
    # different silhouette from the Abrams' broad flat turret. Pivot at hull origin, barrel +X.
    mat = make_material("tank_turret_fr", rgba("tank_turret_fr"))
    parts = [
        box((1.5, 1.3, 0.62), (-0.10, 0, 1.10)),               # main turret box (taller)
        box((1.0, 1.4, 0.55), (-1.05, 0, 1.05)),               # rear bustle (autoloader) — overhangs
        cyl(0.09, 1.90, (1.35, 0, 1.12), rot=(0, math.radians(90), 0)),  # 120mm barrel, +X
        box((0.4, 0.4, 0.40), (-0.3, 0.45, 1.55)),             # roof sight mast
        cyl(0.12, 0.20, (0.42, 0, 1.12), rot=(0, math.radians(90), 0), verts=12),  # bore-evacuator collar
    ]
    return weld("tank_turret_fr", parts, mat, bevel=0.04)


def build_weapon_rifle_us():
    # M4 carbine viewmodel: conventional layout — receiver at origin, barrel forward (+X), magazine
    # BELOW/forward of the grip, collapsible stock to the rear, flat-top rail on top. Modelled in the
    # same frame as `weapon_rifle` so `weapon_view_model` re-bases +X→forward unchanged.
    mat = make_material("weapon_rifle_us", rgba("weapon_rifle_us"))
    parts = [
        box((0.46, 0.06, 0.11), (0.0, 0, 0)),                  # upper/lower receiver
        box((0.40, 0.05, 0.04), (0.0, 0, 0.085)),              # flat-top picatinny rail
        cyl(0.018, 0.46, (0.42, 0, 0), rot=(0, math.radians(90), 0)),  # barrel (forward)
        cyl(0.03, 0.18, (0.30, 0, -0.02), rot=(0, math.radians(90), 0)),  # handguard
        box((0.06, 0.05, 0.20), (0.02, 0, -0.14)),             # STANAG magazine (curved, forward of grip)
        box((0.07, 0.05, 0.14), (-0.10, 0, -0.10), rot=(0, math.radians(-14), 0)),  # pistol grip
        box((0.22, 0.05, 0.10), (-0.34, 0, 0.0)),              # collapsible stock
        box((0.02, 0.03, 0.06), (0.40, 0, 0.05)),              # front sight post
    ]
    return weld("weapon_rifle_us", parts, mat, bevel=0.006)


def build_weapon_rifle_fr():
    # FAMAS bullpup viewmodel: the headline-distinct rifle silhouette — magazine BEHIND the grip
    # (toward the stock), a tall full-length carry handle on top, short overall. Receiver at origin,
    # barrel +X, same frame as `weapon_rifle`.
    mat = make_material("weapon_rifle_fr", rgba("weapon_rifle_fr"))
    parts = [
        box((0.50, 0.07, 0.15), (-0.05, 0, 0)),                # bullpup body (action sits at the rear)
        box((0.34, 0.03, 0.10), (-0.02, 0, 0.16)),             # tall full-length carry handle (the FAMAS tell)
        box((0.02, 0.03, 0.10), (0.15, 0, 0.11)),              # front handle post
        box((0.02, 0.03, 0.10), (-0.19, 0, 0.11)),             # rear handle post
        cyl(0.016, 0.34, (0.38, 0, 0.0), rot=(0, math.radians(90), 0)),  # thin barrel (forward)
        box((0.05, 0.05, 0.16), (-0.20, 0, -0.12)),            # magazine BEHIND the grip (bullpup)
        box((0.06, 0.05, 0.13), (0.02, 0, -0.10), rot=(0, math.radians(-10), 0)),  # pistol grip (forward of mag)
    ]
    return weld("weapon_rifle_fr", parts, mat, bevel=0.006)


MODELS = [
    ("trooper", build_trooper,
     "Greybox infantry unit — boxy humanoid (hips/torso/head/limbs)."),
    ("tank", build_tank,
     "Greybox vehicle hull — chassis + tracks (turret is a separate model so it slews independently)."),
    ("tank_turret", build_tank_turret,
     "Greybox tank turret — gun mantlet + barrel, pivoting about the hull's turret ring (P7)."),
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
    ("tracer", build_tracer,
     "Tank-shell tracer — a small +X-elongated bolt, placed at the shell and yawed by velocity (P7)."),
    # Faction cosmetic silhouettes (WS-C, D68) — presentation-only per-army variants.
    ("trooper_us", build_trooper_us,
     "US Army infantry silhouette — rounded combat helmet, plate-carrier torso (WS-C)."),
    ("trooper_fr", build_trooper_fr,
     "French Army infantry silhouette — flatter brimmed SPECTRA helmet, slimmer profile (WS-C)."),
    ("tank_us", build_tank_us,
     "US M1 Abrams hull — long, low, flat chassis with a sloped front glacis (WS-C)."),
    ("tank_turret_us", build_tank_turret_us,
     "US M1 Abrams turret — broad flat turret + long 120mm gun, pivoting about the hull ring (WS-C/P7)."),
    ("tank_fr", build_tank_fr,
     "French Leclerc hull — compact chassis with a steeper sloped glacis (WS-C)."),
    ("tank_turret_fr", build_tank_turret_fr,
     "French Leclerc turret — taller box with a rear autoloader bustle, pivoting about the hull ring (WS-C/P7)."),
    ("weapon_rifle_us", build_weapon_rifle_us,
     "US M4 carbine viewmodel — conventional layout, magazine forward of the grip, flat-top rail (WS-C)."),
    ("weapon_rifle_fr", build_weapon_rifle_fr,
     "French FAMAS bullpup viewmodel — magazine behind the grip, full-length carry handle (WS-C)."),
]


def tier_record(level, glb_name, mesh_name, ratio):
    """Manifest record for one LOD tier — geometry stats for both the glb and the cooked mesh."""
    glb_path = os.path.join(OUT_DIR, glb_name)
    mesh_path = os.path.join(OUT_DIR, mesh_name)
    return {
        "level": level,
        "file": glb_name,
        "cooked": mesh_name,
        "simplify_ratio": ratio,
        "tri_count": mesh_tris(mesh_path),
        "bytes": os.path.getsize(glb_path),
        "sha256": sha256(glb_path),
        "cooked_bytes": os.path.getsize(mesh_path),
        "cooked_sha256": sha256(mesh_path),
    }


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    entries = []
    for stem, builder, description in MODELS:
        os.makedirs(os.path.join(OUT_DIR, CATEGORY[stem]), exist_ok=True)
        reset_scene()
        obj = builder()
        # LOD0 (full detail): `.glb` (interchange source) + `.mesh` (cooked runtime format the
        # engine loads, D44). Files land in the model's category subfolder (CATEGORY); the LOD0
        # `.mesh` bytes stay identical — only the path moved.
        glb_path = export_glb(obj, relpath(stem, ".glb"))
        mesh_path = export_mesh(obj, relpath(stem, ".mesh"))
        lods = [tier_record(0, relpath(stem, ".glb"), relpath(stem, ".mesh"), 1.0)]

        # Decimated tiers: gltfpack simplifies the glb, then we re-import it and run the SAME
        # `export_mesh` cook so the tier lands in the identical GDM1 format with recomputed flat
        # normals. LOD2 chains off LOD1's glb (monotone pyramid; see LOD_TIERS notes).
        prev_glb = relpath(stem, ".glb")
        for level, suffix, ratio_arg, cum_ratio in LOD_TIERS:
            lod_glb = relpath(stem, suffix + ".glb")
            lod_mesh = relpath(stem, suffix + ".mesh")
            run_gltfpack(prev_glb, lod_glb, ratio_arg)
            reset_scene()
            imp = import_glb(lod_glb)
            export_mesh(imp, lod_mesh)
            lods.append(tier_record(level, lod_glb, lod_mesh, cum_ratio))
            prev_glb = lod_glb

        entries.append({
            "name": stem,
            "category": CATEGORY[stem],
            "file": relpath(stem, ".glb"),
            "cooked": relpath(stem, ".mesh"),
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
            "lods": lods,
        })
        tiers = " → ".join(f"L{t['level']}:{t['tri_count']}t" for t in lods)
        print(f"  wrote {stem} [{tiers}]  (LOD0 {entries[-1]['cooked_bytes']} B)")

    manifest = {
        "note": (
            "Placeholder greybox models, generated by tools/models/gen_models.py "
            "(decisions.md D41). Each model ships a full-detail tier — a `.glb` (interchange "
            "source) and a cooked `.mesh` the engine loads directly (decisions.md D44) — plus a "
            "gltfpack-decimated LOD chain (`<name>.lod1.*`, `<name>.lod2.*`): gltfpack simplifies "
            "the glb (`-si … -sa`), which is then re-imported and re-cooked so every tier is the "
            "identical GDM1 `.mesh` format with recomputed flat normals. Per-tier stats live in "
            "each asset's `lods` array; the renderer selects a tier by on-screen size/distance. "
            "Render-only; regenerate with `pnpm assets:models`. License-clean by construction — "
            "code-authored primitives, CC0-1.0 (content-pipeline.md §3). Honest weak axis: "
            "eye-level FPS credibility (§4)."
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
