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
import bmesh
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


# Maps a rounded albedo RGB → its team-tint mask. The `.glb` exporter drops Base-Color *alpha* for
# opaque materials (so the mask can't ride the LOD gltfpack round-trip in the alpha channel), but it
# round-trips the RGB *factor* exactly. So we key the mask off the albedo RGB and reconstruct it in
# `export_mesh` for every tier — LOD0 and the re-imported LODs alike. Palette colours are distinct.
MASK_BY_RGB = {}


def _rgb_key(rgb):
    return (round(rgb[0], 3), round(rgb[1], 3), round(rgb[2], 3))


def make_material(name, rgba, mask=1.0):
    """A greybox material. `rgba` is the part albedo; `mask` is the team-tint fraction in [0,1] the
    runtime shader blends the per-instance team colour over this part (see `export_mesh` + the
    `.mesh` format doc in `render/src/mesh.rs`): 1.0 = fully team-coloured (buildings/vehicles),
    ~0.5 = a team-hued uniform, 0.0 = keeps its own colour (skin, rifle, boots). The mask is keyed
    off the albedo RGB (see `MASK_BY_RGB`) so it survives the LOD glTF round-trip."""
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    if getattr(m, "node_tree", None) is None:  # 5.x materials already node-backed
        m.use_nodes = True
    bsdf = m.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = (rgba[0], rgba[1], rgba[2], 1.0)
    bsdf.inputs["Roughness"].default_value = 0.85
    m.diffuse_color = (rgba[0], rgba[1], rgba[2], 1.0)  # viewport-only; export reads the BSDF node
    MASK_BY_RGB[_rgb_key(rgba)] = float(mask)
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


def dome(radius, loc, zsquash=1.0, xyscale=(1.0, 1.0), cut=-1.0e9, subdivisions=2):
    """A helmet shell: an icosphere squashed in LOCAL space with every vertex below `cut` (local z,
    post-squash) deleted, so the kept cap reads as a rounded dome that hugs the crown and sides while
    leaving the face open below the front edge. The cut runs in bmesh in local coordinates — the
    `bpy.ops.mesh.bisect` operator's plane space is scale-dependent and unreliable once the object
    carries a squash, which silhouetted the old helmet as a floating bucket-hat brim. Deterministic
    (icosphere tessellation is stable run-to-run, unlike a UV sphere's poles)."""
    bpy.ops.mesh.primitive_ico_sphere_add(radius=radius, location=loc, subdivisions=subdivisions)
    o = bpy.context.active_object
    me = o.data
    bm = bmesh.new()
    bm.from_mesh(me)
    for v in bm.verts:
        v.co.x *= xyscale[0]
        v.co.y *= xyscale[1]
        v.co.z *= zsquash
    dead = [v for v in bm.verts if v.co.z < cut]
    bmesh.ops.delete(bm, geom=dead, context="VERTS")
    bm.to_mesh(me)
    bm.free()
    return o


def skinned_body(joints, edges, radii, root="pelvis", name="body"):
    """An organic low-poly body from a vertex 'stick figure' + Blender's Skin modifier + one subsurf
    level — the single biggest lift from 'stack of primitives' to 'reads as a human'. Box-stacked
    troopers hit a silhouette ceiling (a slab torso, a golf-ball head, no arms); skinning a jointed
    skeleton clears it in ~the same tri budget.

    `joints` maps a joint name → (x, y, z) in the game frame (Z-up, feet at z≈0). `edges` are
    (name, name) 'bones'. `radii` maps a joint name → (x, y) skin radius (the limb thickness there).
    `root` is the joint the Skin modifier grows the skin from. Returns ONE welded mesh object with no
    material assigned — the caller pairs it with the fatigue material in `weld`. Deterministic: the
    skin+subsurf output is a pure function of the input skeleton, so `pnpm assets:models` regenerates
    bit-identical geometry (invariant of the content pipeline)."""
    keys = list(joints.keys())
    idx = {k: i for i, k in enumerate(keys)}
    me = bpy.data.meshes.new(name)
    me.from_pydata([joints[k] for k in keys], [(idx[a], idx[b]) for a, b in edges], [])
    me.update()
    o = bpy.data.objects.new(name, me)
    bpy.context.scene.collection.objects.link(o)
    bpy.context.view_layer.objects.active = o
    o.select_set(True)
    o.modifiers.new("skin", "SKIN")
    skin_data = o.data.skin_vertices[0].data
    for k in keys:
        skin_data[idx[k]].radius = radii[k]
    o.data.skin_vertices[0].data[idx[root]].use_root = True
    sub = o.modifiers.new("sub", "SUBSURF")
    sub.levels = 1
    sub.render_levels = 1
    bpy.ops.object.modifier_apply(modifier="skin")
    bpy.ops.object.modifier_apply(modifier="sub")
    return o


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


def boolean_cut(target, cutters):
    """Subtract each `cutters` object from `target` with the EXACT boolean solver, applying the
    modifier immediately and deleting the spent cutter. This is the WS-F "boolean cuts for real
    sloped/inset detail" lever (visual-design-plan §WS-F): a Picatinny rail's transverse slots, a
    magazine well's inset, a skeletonized stock's lightening cut — geometry a box-stack can only fake.
    The exact solver is a pure function of its input geometry, so `pnpm assets:models` regenerates
    bit-identical (content-pipeline determinism). New faces inherit `target`'s material_index 0, i.e.
    the part's own gunmetal — the cut walls stay the same colour, no extra material slot. Returns the
    cut `target`."""
    for c in cutters:
        bpy.ops.object.select_all(action="DESELECT")
        target.select_set(True)
        bpy.context.view_layer.objects.active = target
        m = target.modifiers.new("bool", "BOOLEAN")
        m.operation = "DIFFERENCE"
        m.solver = "EXACT"
        m.object = c
        bpy.ops.object.modifier_apply(modifier=m.name)
    for c in cutters:
        bpy.data.objects.remove(c, do_unlink=True)
    return target


def weld(name, parts, material=None, bevel=0.0):
    """Apply each part's transform, assign its material, then join into one mesh. `parts` is a list
    of either a bare object (uses the default `material`) or an `(object, material)` tuple — so a
    model can carry MULTIPLE materials (fatigues / helmet / skin / rifle…), which `export_mesh`
    bakes into per-vertex colours. Joining merges the parts' material slots and remaps each face's
    `material_index`, so per-part colour survives the weld. `bevel` (metres) applies an
    angle-limited `chamfer` to the welded result — soft silhouette edges per model."""
    objs = []
    for p in parts:
        o, m = p if isinstance(p, tuple) else (p, material)
        o.data.materials.clear()
        o.data.materials.append(m)
        objs.append(o)
    for o in objs:
        bpy.ops.object.select_all(action="DESELECT")
        o.select_set(True)
        bpy.context.view_layer.objects.active = o
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    bpy.ops.object.select_all(action="DESELECT")
    for o in objs:
        o.select_set(True)
    bpy.context.view_layer.objects.active = objs[0]
    if len(objs) > 1:  # join() warns "No mesh data to join" on a single object
        bpy.ops.object.join()
    obj = bpy.context.active_object
    obj.name = name
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
#   magic   : 4 bytes  b"GDM2"
#   v_count : u32       number of vertices  (== 3 * triangle count)
#   i_count : u32       number of indices   (sequential 0..v_count for the soup)
#   verts   : v_count × [px,py,pz, nx,ny,nz, cr,cg,cb, cm]  f32  (40 bytes each)
#   indices : i_count × u32
#
# `cr,cg,cb` is the face's material albedo and `cm` its team-tint mask [0,1] (read from the part's
# material — `diffuse_color` + the `team_mask` custom prop). Coords are Z-up world metres with the
# base at z≈0 — matching the game's ground plane (`render/shader.wgsl` puts world XY on z=0, Z up).
# NOTE: the `.glb` exporter rewrites to glTF's +Y-up convention; the `.mesh` deliberately keeps
# Blender/​game Z-up. They describe the same geometry in each format's native up-axis. Keep this
# layout in lockstep with the parser in `render/src/mesh.rs` and its golden test. GDM2 added the
# per-vertex colour to GDM1 (position+normal only).
MESH_MAGIC = b"GDM2"


def export_mesh(obj, filename):
    from mathutils import Vector

    mesh = obj.data
    mesh.calc_loop_triangles()
    # Per-material-slot (albedo rgb, team-mask), indexed by a face's `material_index`. Read straight
    # from the Principled BSDF Base Color (rgb = albedo, alpha = mask) — the one encoding that
    # survives both the LOD0 cook and the gltfpack→re-import LOD round-trip. Missing/degenerate slots
    # fall back to a neutral fully-team-tinted grey (matches the old flat look).
    def slot_color(m):
        bsdf = m.node_tree.nodes.get("Principled BSDF") if (m and m.node_tree) else None
        if bsdf is None:
            return (0.5, 0.5, 0.5, 1.0)
        bc = bsdf.inputs["Base Color"].default_value
        mask = MASK_BY_RGB.get(_rgb_key(bc), 1.0)  # RGB survives glTF; reconstruct the mask from it
        return (bc[0], bc[1], bc[2], mask)

    slot_colors = [slot_color(m) for m in mesh.materials] or [(0.5, 0.5, 0.5, 1.0)]

    verts = []  # flat f32 list: px,py,pz,nx,ny,nz,cr,cg,cb,cm per corner
    for tri in mesh.loop_triangles:
        # Flat shading: compute each triangle's own geometric normal from its vertices (the
        # CCW cross product) and share it across all three corners, so edges read as crisp
        # facets (the greybox aesthetic). Computing it here — rather than reading Blender's
        # cached polygon normal — guarantees a unit, perpendicular normal even after the
        # non-uniform `dimensions` scale bakes a skewed normal into that cache.
        co = [mesh.vertices[vi].co for vi in tri.vertices]
        n = (co[1] - co[0]).cross(co[2] - co[0])
        n = n.normalized() if n.length > 1e-9 else Vector((0.0, 0.0, 1.0))
        col = slot_colors[tri.material_index if tri.material_index < len(slot_colors) else 0]
        for c in co:
            verts.extend((c.x, c.y, c.z, n.x, n.y, n.z, col[0], col[1], col[2], col[3]))
    v_count = len(mesh.loop_triangles) * 3
    assert v_count * 10 == len(verts), "expected 10 floats per vertex"

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
# Each model's representative/dominant greybox tint, mirrored in `mesh.rs`'s `ModelKind::base_color`
# and echoed into the manifest so that mirror is auditable. Since GDM2 the `.mesh` also carries a
# per-VERTEX material colour + team-tint mask (troopers set several via `infantry_palette`; simpler
# models are a single material = this colour at mask 1.0), so a soldier renders as a coloured
# soldier and the per-instance faction colour tints only the masked parts + the silhouette rim
# (player blue / enemy red) rather than flooding the whole body.
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
    "turret_us": (0.30, 0.31, 0.24),        # US emplacement — CARC grey-green (matches Abrams hull)
    "turret_fr": (0.22, 0.27, 0.18),        # FR emplacement — darker French green (matches Leclerc hull)
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
    "turret_us": "structures",
    "turret_fr": "structures",
}


def relpath(stem, suffix):
    """Category-relative path for a model file, e.g. ('trooper', '.lod1.glb') → 'units/trooper.lod1.glb'.
    Always forward-slashed so the strings written into manifest.json are stable across platforms."""
    return CATEGORY[stem] + "/" + stem + suffix


def rgba(name):
    r, g, b = COLORS[name]
    return (r, g, b, 1.0)


def infantry_palette(prefix, fatigue, helmet):
    """The per-part material set every trooper is built from, so a soldier renders as a *coloured*
    soldier — olive/green fatigues that take a team tint, a darker helmet, tan skin, a near-black
    rifle, dark boots — instead of one flat team-coloured blob. Only the uniform + helmet carry a
    team-tint mask; skin/rifle/boots keep their own colour and the silhouette rim carries the team
    read (see `render/src/mesh.wgsl`). `fatigue`/`helmet` vary per army; the rest are shared."""
    return {
        "fatigue": make_material(prefix + "_fatigue", fatigue, mask=0.55),
        "helmet": make_material(prefix + "_helmet", helmet, mask=0.42),
        "skin": make_material(prefix + "_skin", (0.60, 0.45, 0.33), mask=0.0),
        "gun": make_material(prefix + "_gun", (0.08, 0.08, 0.09), mask=0.0),
        "boots": make_material(prefix + "_boots", (0.11, 0.10, 0.08), mask=0.0),
        "web": make_material(prefix + "_web", (0.19, 0.20, 0.14), mask=0.12),
        "pack": make_material(prefix + "_pack", (0.24, 0.26, 0.16), mask=0.30),
    }


def soldier_parts(P, bulk=1.0, helmet="pot"):
    """The shared skinned-infantryman, ready for `weld`. An organic body (`skinned_body`) in fatigues,
    a skin head + hands, a helmet (`dome`), an M1956 pistol belt with ammo pouches, jungle boots, and
    an M16 held at the ready. Coordinate frame agrees with the engine's `+X = forward` convention
    (renderer's `model_matrix` rotates local +X onto the unit's heading; the tank nose/barrel are +X
    too): Z-up, feet at z≈0; boot toes point +X; face/pouches/rifle-muzzle front at +X; pack at −X;
    left/right are ±Y. The rifle barrel runs along +X so it points where the trooper faces — the sim
    now turns riflemen to FACE their target, so the gun must agree (was authored +Y-forward with the
    rifle laid across the chest, which read as the gun sticking out sideways). Per-part materials
    (`infantry_palette`) so it reads as a coloured soldier — only the uniform + helmet carry a team tint.

    `bulk` widens the shoulders/chest (US heavier, FR slimmer — the fairness-bounded WS-C silhouette
    tell). `helmet` picks the shell: "pot" = the M1 steel pot (rounded dome + subtle rolled brim),
    "spectra" = the flatter, front-brimmed French shell. Each builder appends army-specific kit
    (rucksack, grenade, bedroll) before welding."""
    sh = 0.20 * bulk        # shoulder half-width (±Y)
    chest_r = 0.185 * bulk  # chest skin radius
    arm_r = 0.09 * bulk
    # Body faces +X. Legs/shoulders straddle ±Y. The arms are RE-POSED into an aimed/ready grip: both
    # hands come onto a rifle held just off the centreline in front of the chest, barrel forward (+X)
    # — the support (left) arm reaches out to the handguard, the firing (right) arm tucks the grip in.
    joints = {
        "pelvis": (0.0, 0.0, 0.94), "spine": (-0.01, 0.0, 1.14), "chest": (-0.02, 0.0, 1.37),
        "neck": (-0.015, 0.0, 1.505),
        "shL": (-0.01, -sh, 1.45), "elbL": (0.16, -0.12, 1.34), "haL": (0.37, 0.03, 1.29),
        "shR": (-0.01, sh, 1.45), "elbR": (-0.05, sh, 1.25), "haR": (0.06, 0.09, 1.23),
        "hipL": (0.0, -0.09, 0.90), "kneeL": (0.01, -0.11, 0.50), "ankL": (0.0, -0.11, 0.10),
        "hipR": (0.0, 0.09, 0.90), "kneeR": (0.01, 0.11, 0.50), "ankR": (0.0, 0.11, 0.10),
    }
    edges = [
        ("pelvis", "spine"), ("spine", "chest"), ("chest", "neck"),
        ("chest", "shL"), ("shL", "elbL"), ("elbL", "haL"),
        ("chest", "shR"), ("shR", "elbR"), ("elbR", "haR"),
        ("pelvis", "hipL"), ("hipL", "kneeL"), ("kneeL", "ankL"),
        ("pelvis", "hipR"), ("hipR", "kneeR"), ("kneeR", "ankR"),
    ]
    radii = {
        "pelvis": (0.155, 0.12), "spine": (0.15, 0.11), "chest": (chest_r, 0.125), "neck": (0.062, 0.062),
        "shL": (arm_r, arm_r), "elbL": (0.058, 0.058), "haL": (0.05, 0.05),
        "shR": (arm_r, arm_r), "elbR": (0.058, 0.058), "haR": (0.05, 0.05),
        "hipL": (0.105, 0.105), "kneeL": (0.075, 0.075), "ankL": (0.052, 0.062),
        "hipR": (0.105, 0.105), "kneeR": (0.075, 0.075), "ankR": (0.052, 0.062),
    }
    body = skinned_body(joints, edges, radii)
    parts = [
        (body, P["fatigue"]),                                                 # organic skinned body
        (icosphere(0.108, (0.01, 0.0, 1.585), subdivisions=2), P["skin"]),    # head (face shows below the brim, +X)
        (icosphere(0.055, (0.37, 0.03, 1.29), subdivisions=1), P["skin"]),    # hand L (support, on the handguard)
        (icosphere(0.055, (0.06, 0.09, 1.23), subdivisions=1), P["skin"]),    # hand R (firing, on the grip)
        (box((0.27, 0.15, 0.12), (0.06, -0.11, 0.06)), P["boots"]),           # jungle boot R (toe forward, +X)
        (box((0.27, 0.15, 0.12), (0.06, 0.11, 0.06)), P["boots"]),            # jungle boot L
        # M1956 web gear: a pistol belt hugging the waist + two compact front (+X) ammo pouches.
        (box((0.24, 0.345, 0.085), (0, 0, 1.00)), P["web"]),                  # pistol belt (wraps ±Y)
        (box((0.09, 0.10, 0.12), (0.135, -0.13, 1.00)), P["web"]),            # ammo pouch R (front)
        (box((0.09, 0.10, 0.12), (0.135, 0.13, 1.00)), P["web"]),             # ammo pouch L (front)
        # M16 held at the ready: receiver/barrel along +X (points where the trooper faces), tucked in
        # front of the chest just off centre — carry handle proud on top, mag hanging below. Legible
        # from the front AND the top-down command view as an aimed weapon, not a bar across the chest.
        (box((0.60, 0.06, 0.07), (0.16, 0.05, 1.30)), P["gun"]),              # receiver/barrel (+X)
        (box((0.14, 0.055, 0.045), (0.11, 0.05, 1.36)), P["gun"]),            # carry handle (atop)
        (box((0.05, 0.07, 0.16), (0.15, 0.05, 1.18)), P["gun"]),              # magazine (hangs below)
    ]
    if helmet == "spectra":
        # French SPECTRA: a flatter shell with a short forward (+X) brim (the FR silhouette tell).
        parts.append((dome(0.15, (-0.01, 0.0, 1.60), zsquash=0.80, xyscale=(1.02, 0.98), cut=-0.030), P["helmet"]))
        parts.append((box((0.14, 0.28, 0.045), (0.13, 0.0, 1.585)), P["helmet"]))   # front brim accent (+X)
    else:
        # M1 steel pot: a rounded dome hugging crown + sides down past the ears, with a subtle rolled
        # brim lip — the face pokes out below the front edge. NOT a floating sombrero (the old failure).
        parts.append((dome(0.155, (-0.01, 0.0, 1.60), zsquash=0.92, xyscale=(1.0, 0.95), cut=-0.028), P["helmet"]))
        parts.append((cyl(0.152, 0.028, (-0.01, 0.0, 1.575), verts=18), P["helmet"]))  # rolled brim lip
    return parts


def build_trooper():
    # The Neutral infantryman: an organic skinned soldier (skeleton + Skin modifier via `soldier_parts`)
    # in jungle fatigues, cradling an M16 at patrol-ready under an M1 steel-pot helmet, with a tropical
    # rucksack + bedroll. This replaced the old box-stack (slab torso / golf-ball head / no arms) — same
    # silhouette intent, finally readable as a human. Per-part materials; only uniform + helmet tint.
    P = infantry_palette("gd", (0.30, 0.34, 0.20), (0.20, 0.23, 0.15))
    parts = soldier_parts(P, bulk=1.0, helmet="pot")
    parts += [
        (box((0.15, 0.26, 0.34), (-0.185, 0.0, 1.25)), P["pack"]),            # tropical rucksack (tucked to the back, −X)
        (cyl(0.05, 0.26, (-0.20, 0.0, 1.44), rot=(math.radians(90), 0, 0), verts=8), P["pack"]),  # bedroll lashed across the shoulders (±Y)
    ]
    return weld("trooper", parts, bevel=0.0)


def running_gear(track_y, wheel_z, wheels, track_dims, track_z, fender_dims, fender_z,
                 wheel_depth=0.14):
    """One side's track run + a distinct road-wheel line, then mirrored to the other side. The WS-F
    tier-2 "distinct road-wheel read" lever: the road wheels are cylinders whose OUTER disc face sits
    proud of the track slab's outer face (not buried inside it, the old failure that made the running
    gear invisible), so each side reads as a row of circles. `wheels` is a list of (x, radius) — a
    larger idler/sprocket at each end reads the gear front-to-back. verts=10 keeps the facet chunky
    without tripping the 40° chamfer limit (at 8 the 45° inter-facet angle bevels every rim edge)."""
    parts = []
    for side in (1.0, -1.0):
        y = side * track_y
        parts.append(box(track_dims, (0, y, track_z)))                     # track run (shoe belt)
        parts.append(box(fender_dims, (0, y, fender_z)))                   # track guard / fender
        outer = y + side * (track_dims[1] * 0.5 - wheel_depth * 0.35)      # disc proud of the track face
        for x, r in wheels:
            parts.append(cyl(r, wheel_depth, (x, outer, wheel_z),
                             rot=(math.radians(90), 0, 0), verts=10))
    return parts


def build_tank():
    # The tank HULL (chassis + tracks) only — the turret is a SEPARATE model
    # (`build_tank_turret`) so the renderer can slew it independently of the hull (tank
    # embodiment P7, D55). Both keep the dark-green armour tint. The turret-ring pivot sits at
    # the hull's local origin (x=0, y=0), so a turret drawn at the same world (x, y) and lifted
    # to z≈hull-top rotates about that ring exactly.
    #
    # WS-F tier-2 lift (visual-design-plan §WS-F): the old hull read "lumpy road gear, melty slopes".
    # Fixes — (1) a BOOLEAN glacis: the sloped front is milled straight into the upper hull block as
    # one crisp integral plane instead of a separate rounded plate floating off the nose; (2) a
    # BOOLEAN sponson undercut so the upper hull visibly overhangs the tracks (a real shadow line, not
    # a melty blob); (3) a distinct road-wheel read (`running_gear`) with the wheels proud of the track
    # face; (4) a tight bevel (0.018, was 0.05) so the armour plates stay crisp — booleans + facets
    # carry the detail, no soap-bar over-rounding.
    mat = make_material("tank", rgba("tank"))  # dark green
    lower = box((2.9, 1.42, 0.44), (0, 0, 0.42))               # lower hull tub (between the tracks)
    upper = box((2.95, 1.9, 0.42), (0, 0, 0.78))               # upper hull / sponson (overhangs tracks)
    # Glacis: slice the top-front wedge off the upper hull along a ~34° plane → one crisp sloped nose.
    # The cutter is a large half-space (its underside IS the glacis plane, its bulk sits up-and-forward
    # of the hull) so only that one face touches the block — a smaller box let a stray edge clip the
    # top deck.
    boolean_cut(upper, [box((5.0, 3.0, 4.0), (2.59, 0, 2.23), rot=(0, math.radians(34), 0))])
    # Sponson undercut: notch the underside of each overhang so the hull reads as sitting proud of the
    # tracks with a defined shadow line (not a slab that melts into the running gear).
    boolean_cut(upper, [box((2.6, 0.34, 0.20), (0, 0.86, 0.60)),
                        box((2.6, 0.34, 0.20), (0, -0.86, 0.60))])
    parts = [
        lower, upper,
        box((0.5, 1.42, 0.34), (-1.42, 0, 0.60), rot=(0, math.radians(-18), 0)),  # rear hull plate (sloped)
    ]
    parts += running_gear(
        track_y=0.86, wheel_z=0.30,
        wheels=((1.30, 0.28), (0.70, 0.22), (0.15, 0.22), (-0.40, 0.22), (-0.95, 0.22), (-1.42, 0.28)),
        track_dims=(3.15, 0.42, 0.42), track_z=0.30,
        fender_dims=(3.25, 0.48, 0.10), fender_z=0.60,
    )
    return weld("tank", parts, mat, bevel=0.018)


def build_tank_turret():
    # The tank TURRET (gun mantlet + barrel) as its own model so it can yaw independently of the
    # hull (P7). Modelled in the hull's local frame: the turret-ring pivot is the local origin
    # (x=0, y=0) about which the renderer rotates by `turret_yaw`, and the geometry keeps its real
    # height (z≈1.05, sitting on the hull top at z≈0.95). Drawing it at the hull's world (x, y) with
    # yaw = turret_yaw therefore slews it about the ring. Barrel points +X (turret_yaw 0 == hull 0).
    mat = make_material("tank_turret", rgba("tank_turret"))  # dark green (matches the hull)
    # WS-F tier-2: a crisper turret to match the tightened hull. A boolean sloped face gives a real
    # cast-mantlet cheek instead of a melty box, and the bevel drops 0.04→0.022 so the edges stay sharp.
    turret = box((1.5, 1.2, 0.50), (-0.2, 0, 1.05))            # turret box (centred behind the ring)
    boolean_cut(turret, [box((1.2, 1.6, 0.9), (0.95, 0, 1.55), rot=(0, math.radians(38), 0))])  # sloped front cheek
    mantlet = box((0.5, 1.0, 0.40), (0.5, 0, 1.02))            # gun mantlet block
    boolean_cut(mantlet, [box((0.6, 0.34, 0.16), (0.55, 0, 1.20)),        # recessed sight ports flanking the gun
                          box((0.6, 0.34, 0.16), (0.55, 0, 0.86))])
    parts = [
        cyl(0.58, 0.12, (0, 0, 0.86), verts=12),               # ring base (drops into the hull socket)
        turret, mantlet,
        box((0.9, 1.3, 0.22), (-0.55, 0, 0.98)),               # rear stowage bustle (overhangs)
        cyl(0.22, 0.16, (-0.45, 0.0, 1.38), verts=12),         # commander's cupola
        cyl(0.10, 0.22, (-0.10, 0.40, 1.34), verts=8),         # coaxial / loader's MG mount
        cyl(0.09, 1.65, (1.25, 0, 1.05), rot=(0, math.radians(90), 0)),  # barrel, forward along +X
        cyl(0.13, 0.20, (0.60, 0, 1.05), rot=(0, math.radians(90), 0), verts=12),  # bore-evacuator collar
    ]
    return weld("tank_turret", parts, mat, bevel=0.022)


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
    # A real command building. WS-F tier-3 lift (visual-design-plan §WS-F): kill the melty base and
    # sharpen the facade with BOOLEAN inset detail instead of proud slabs.
    #   (1) a crisp foundation PLINTH, proud of the walls, gives a hard groundline (was: the walls
    #       melting straight into the ground under the heavy 0.06 bevel);
    #   (2) the two front windows + the doorway are now BOOLEAN-CUT recesses — real reveals with a
    #       pane / door set back in the opening — instead of boxes stuck onto the wall face;
    #   (3) the bevel drops 0.06 → 0.03 so the cornice, pilasters and the new recess edges read as
    #       crisp cast chamfers, not soap-bar rounding.
    # Silhouette identity is unchanged: hipped roof + ridge vent, pilaster-framed entrance under a
    # sloped awning on two posts, flanking windows, rooftop vent + antenna mast (top-down read).
    walls = box((3.5, 3.0, 1.8), (0, 0, 0.90))                 # main wall block (front face at y=+1.5)
    # Boolean recesses milled into the front (+Y) face: two windows + a doorway. Each cutter pokes
    # ~0.21 m into the wall from the face, leaving a reveal the bevel then crisps into a cast edge.
    boolean_cut(walls, [
        box((0.66, 0.42, 0.56), (-1.05, 1.5, 1.22)),           # window opening L
        box((0.66, 0.42, 0.56), (1.05, 1.5, 1.22)),            # window opening R
        box((0.78, 0.42, 1.16), (0, 1.5, 0.58)),               # doorway opening
    ])
    parts = [
        walls,
        box((3.66, 3.16, 0.26), (0, 0, 0.13)),                 # foundation plinth (proud footing → hard groundline)
        box((0.28, 0.28, 1.9), (-1.73, 1.48, 0.95)),           # front pilaster L (frames the facade)
        box((0.28, 0.28, 1.9), (1.73, 1.48, 0.95)),            # front pilaster R
        box((3.7, 3.2, 0.20), (0, 0, 1.80)),                   # eave / cornice band (roofline lip)
        pyramid(2.55, 1.10, (0, 0, 2.34)),                     # hipped roof
        box((1.7, 0.55, 0.20), (0, 0, 2.88)),                  # ridge vent cap along the roof apex
        # Panes + door set back in the boolean reveals (depth read, not proud slabs).
        box((0.60, 0.06, 0.50), (-1.05, 1.42, 1.22)),          # window pane L (recessed)
        box((0.60, 0.06, 0.50), (1.05, 1.42, 1.22)),           # window pane R (recessed)
        box((0.72, 0.07, 1.08), (0, 1.42, 0.56)),              # door panel (recessed)
        # Front entrance awning on two posts over the doorway.
        box((1.34, 0.60, 0.10), (0, 1.84, 1.34), rot=(math.radians(-12), 0, 0)),  # sloped entrance awning
        box((0.09, 0.09, 1.20), (-0.58, 2.02, 0.60)),          # awning post L
        box((0.09, 0.09, 1.20), (0.58, 2.02, 0.60)),           # awning post R
        # Rooftop kit + mast (top-down read).
        box((0.46, 0.46, 0.52), (-0.2, -0.2, 2.94)),           # rooftop vent housing
        cyl(0.045, 1.60, (1.15, 1.0, 3.55)),                   # antenna mast
        cyl(0.11, 0.34, (1.15, 1.0, 2.96), verts=8),           # antenna base
        box((0.56, 0.05, 0.05), (1.15, 1.0, 3.85)),            # mast cross-spreader
    ]
    return weld("camp_hq", parts, mat, bevel=0.03)


def picatinny_slots(x0, x1, y_half, z_top, count, slot_w=0.013, depth=0.016):
    """Cutter boxes for a Picatinny rail's transverse recoil-groove ladder — `count` slots evenly
    spread across [x0, x1], each a thin box straddling the rail top (`z_top`) so `boolean_cut` mills a
    crisp cross-slot. Returns the cutter list (caller boolean-subtracts them from the rail)."""
    cutters = []
    for i in range(count):
        cx = x0 + (x1 - x0) * (i + 0.5) / count
        cutters.append(box((slot_w, y_half * 2.4, depth), (cx, 0, z_top)))
    return cutters


def build_weapon_rifle():
    mat = make_material("weapon_rifle", rgba("weapon_rifle"))  # gunmetal
    # The eye-level HERO prop (§4's own "honest weak axis") — the WS-F tier-1 model: it fills the
    # screen embodied, so it gets the most detail budget and the boolean lever. Real Picatinny slots
    # milled into the flat-top rail, a flared magazine WELL seating the mag into the receiver (kills
    # the old "floating mag" read), rib bands on the handguard (the "ribbed" claim finally met), a
    # skeletonized/lightened collapsible stock, and a proud ejection-port cover + charging handle for
    # small-part credibility. Receiver at origin, barrel +X. bevel stays tight (0.006) — booleans do
    # the detail, so no melty over-rounding.
    receiver = box((0.46, 0.06, 0.11), (0.0, 0, 0))            # upper/lower receiver
    # Ejection-port cover cut into the +Y side of the receiver, then a proud cover lip beside it.
    boolean_cut(receiver, [box((0.11, 0.02, 0.05), (0.075, 0.031, 0.012))])

    rail = box((0.40, 0.05, 0.037), (0.0, 0, 0.083))           # flat-top picatinny rail
    # Slot the exposed rail fore & aft of where the optic clamps on (optic body spans x≈[-0.075,-0.005]).
    boolean_cut(rail, picatinny_slots(0.02, 0.19, 0.025, 0.106, 3)
                      + picatinny_slots(-0.185, -0.105, 0.025, 0.106, 1))

    # Ribbed handguard: a smooth core tube banded by three proud rib rings — reads as segmented/ribbed
    # rather than a plain pipe (the old failure). Rings are along +X like the tube they hug.
    handguard = [cyl(0.030, 0.22, (0.26, 0, -0.012), rot=(0, math.radians(90), 0), verts=12)]
    for rx in (0.17, 0.26, 0.35):
        handguard.append(cyl(0.038, 0.018, (rx, 0, -0.012), rot=(0, math.radians(90), 0), verts=8))

    # Skeletonized collapsible stock: a solid buttstock lightened by a boolean through-slot, so it
    # reads as a wire/collapsible stock instead of a solid brick.
    stock = box((0.20, 0.05, 0.10), (-0.32, 0, 0.0))
    boolean_cut(stock, [box((0.11, 0.08, 0.042), (-0.335, 0, 0.0))])

    # Flared magazine well bridging the receiver bottom to the canted STANAG mag — the boolean-adjacent
    # inset read the plan calls for, minus a wasted hidden cut (the mag plugs the opening).
    magwell = box((0.115, 0.062, 0.085), (-0.015, 0, -0.078))

    parts = [
        receiver, rail,
        box((0.07, 0.05, 0.05), (-0.04, 0, 0.126)),            # optic body (low-profile red-dot)
        box((0.05, 0.055, 0.025), (-0.04, 0, 0.161)),          # optic hood
        box((0.04, 0.048, 0.014), (-0.055, 0, 0.14)),          # optic lens bezel (front face)
        *handguard,
        box((0.20, 0.066, 0.012), (0.26, 0, -0.05)),           # handguard underrail (M-LOK slab)
        cyl(0.018, 0.46, (0.42, 0, 0), rot=(0, math.radians(90), 0), verts=10),  # barrel
        box((0.02, 0.03, 0.07), (0.40, 0, 0.05)),              # front sight post
        cyl(0.035, 0.06, (0.64, 0, 0), rot=(0, math.radians(90), 0), verts=10),  # muzzle device
        magwell,
        box((0.07, 0.05, 0.20), (-0.02, 0, -0.155), rot=(0, math.radians(8), 0)),  # magazine (canted STANAG)
        stock,
        box((0.06, 0.045, 0.05), (-0.20, 0, 0.05)),            # cheek riser
        box((0.045, 0.03, 0.02), (-0.185, 0, 0.09)),           # charging-handle latch (rear-top)
        box((0.06, 0.05, 0.14), (-0.10, 0, -0.10), rot=(0, math.radians(-14), 0)),  # grip
    ]
    return weld("weapon_rifle", parts, mat, bevel=0.006)


def build_crate():
    mat = make_material("crate", rgba("crate"))  # wood — low cover prop
    # Slatted shipping crate: a core box, four proud corner posts, and a mid-height banding course
    # so it reads as built planks instead of a featureless 1 m cube.
    #
    # WS-F tier-4 lift (visual-design-plan §WS-F): raise it off flat-panel greybox to a built
    # military shipping crate, on the mechanical/architectural lever (booleans + battens + tighter
    # bevel — NOT skinning). (1) A diagonal cross-brace batten sits proud on every side face (the
    # classic reinforced-crate tell); (2) the lid gains a milled centre seam plus a pair of proud
    # cleats bracing it; (3) the bevel tightens 0.03 → 0.02 so the battens, banding and plank edges
    # stay crisp instead of soap-bar rounded.
    parts = [box((0.94, 0.94, 1.0), (0, 0, 0.50))]            # core
    for sx in (-1, 1):
        for sy in (-1, 1):
            parts.append(box((0.10, 0.10, 1.0), (sx * 0.47, sy * 0.47, 0.50)))  # corner post
    parts += [
        box((1.02, 1.02, 0.10), (0, 0, 0.30)),                 # lower banding course
        box((1.02, 1.02, 0.10), (0, 0, 0.70)),                 # upper banding course
    ]
    # Diagonal cross-brace battens — proud on each of the four side faces (the reinforced-crate read).
    # +/-X faces: batten lies in the Y-Z plane (rotate about X); +/-Y faces: in the X-Z plane.
    for sx in (-1, 1):
        parts.append(box((0.045, 0.10, 1.24), (sx * 0.49, 0, 0.50), rot=(math.radians(38 * sx), 0, 0)))
    for sy in (-1, 1):
        parts.append(box((0.10, 0.045, 1.24), (0, sy * 0.49, 0.50), rot=(0, math.radians(38 * sy), 0)))
    # Lid: a centre seam milled across it + two proud cleats bracing the boards.
    lid = box((1.0, 1.0, 0.08), (0, 0, 1.0))
    boolean_cut(lid, [box((1.05, 0.018, 0.11), (0, 0, 1.0))])  # centre seam across the lid
    parts += [
        lid,
        box((0.80, 0.09, 0.05), (0, -0.30, 1.05)),             # lid cleat (fore)
        box((0.80, 0.09, 0.05), (0, 0.30, 1.05)),              # lid cleat (aft)
    ]
    return weld("crate", parts, mat, bevel=0.02)


def build_turret():
    mat = make_material("turret", rgba("turret"))  # steel defensive emplacement
    # A credible automated weapon emplacement: a bolted ring plate on a base pad, a rotating drum, an
    # armoured gun housing with a sloped face shield, twin elevation trunnion arms cradling the gun, a
    # top sensor/optic block, a side ammo can, a recoil cylinder slung under the barrel, and a muzzle
    # brake. Kept lean — only the parts that make it read as a weapon, not a box on a stick.
    #
    # WS-F tier-3 lift (visual-design-plan §WS-F): real BOOLEAN inset detail on the two parts the
    # player reads closest — (1) a recessed armoured vision slit milled into the sloped face shield
    # (the hero +X plate) instead of a blank slab, and (2) transverse ports cut through the muzzle
    # brake so it reads as a brake, not a plain collar. The bevel tightens 0.03 → 0.022 so the shield
    # slope, ring plate and housing edges stay crisp (no soap-bar rounding).
    shield = box((0.50, 0.98, 0.62), (0.40, 0, 1.20), rot=(0, math.radians(-10), 0))  # sloped face shield (+X)
    boolean_cut(shield, [box((0.28, 0.58, 0.10), (0.60, 0, 1.34), rot=(0, math.radians(-10), 0))])  # vision slit
    brake = cyl(0.11, 0.16, (1.42, 0, 1.20), rot=(0, math.radians(90), 0), verts=10)  # muzzle brake
    boolean_cut(brake, [box((0.05, 0.30, 0.30), (1.40, 0, 1.20)),   # transverse ports (two gaps → three fins)
                        box((0.05, 0.30, 0.30), (1.46, 0, 1.20))])
    parts = [
        box((1.6, 1.6, 0.40), (0, 0, 0.20)),                   # base pad
        box((1.2, 1.2, 0.14), (0, 0, 0.47)),                   # bolted ring plate on the pad
        cyl(0.55, 0.70, (0, 0, 0.70), verts=12),               # rotating drum
        box((0.74, 0.84, 0.46), (-0.05, 0, 1.15)),             # gun housing
        shield,
        box((0.16, 0.10, 0.40), (0.52, 0.42, 1.20)),           # elevation trunnion arm L
        box((0.16, 0.10, 0.40), (0.52, -0.42, 1.20)),          # elevation trunnion arm R
        box((0.32, 0.34, 0.22), (-0.20, 0, 1.47)),             # sensor / optic block (on top)
        box((0.34, 0.30, 0.26), (-0.30, 0.40, 1.18)),          # ammo can (side)
        cyl(0.07, 0.30, (0.55, 0, 1.04), rot=(0, math.radians(90), 0), verts=8),  # recoil cylinder stub (under barrel)
        cyl(0.07, 1.30, (0.78, 0, 1.20), rot=(0, math.radians(90), 0), verts=10),  # barrel
        cyl(0.10, 0.18, (0.34, 0, 1.20), rot=(0, math.radians(90), 0), verts=10),  # barrel shroud
        brake,
    ]
    return weld("turret", parts, mat, bevel=0.022)


def build_tree():
    mat = make_material("tree", rgba("tree"))  # foliage greybox (single material)
    # A stylized low-poly conifer: a tapered trunk plus stacked cone tiers of decreasing radius.
    # Cones/cylinders are deterministic (the old two-UV-sphere canopy varied run-to-run). Each tier
    # is rotated a few degrees so its facets don't line up with the tier below and is nudged slightly
    # off the trunk axis, giving the canopy a natural, hand-grown irregularity instead of a perfect
    # stack of identical cones. The tiers overlap in z so the skirts read as one ragged silhouette.
    #
    # WS-F tier-4 lift (visual-design-plan §WS-F — "maybe a more organic tree canopy"): a light
    # touch, not a rescue. (1) A splayed ROOT-FLARE ring at the base grounds the trunk instead of a
    # stick poking the dirt; (2) the canopy goes 4 → 6 tiers with denser skirts (verts 10 → 12) and
    # a wider spread of per-tier rotation/offset, so the silhouette reads as a full, layered conifer
    # rather than a tidy 4-cone stack; (3) the crown tapers through two small tiers to a finer point.
    # All offsets are hand-tuned constants → the regen stays bit-reproducible.
    parts = [
        cone(0.34, 0.14, 0.24, (0, 0, 0.12), verts=10),                                       # splayed root flare
        cone(0.22, 0.13, 1.42, (0, 0, 0.78), verts=8),                                        # tapered trunk
        cone(1.12, 0.34, 1.10, (0.05, -0.03, 1.42), rot=(0, 0, math.radians(0)), verts=12),   # lowest skirt (widest)
        cone(0.94, 0.28, 1.00, (-0.06, 0.04, 1.92), rot=(0, 0, math.radians(15)), verts=12),  # lower tier
        cone(0.76, 0.22, 0.95, (0.04, -0.04, 2.40), rot=(0, 0, math.radians(30)), verts=12),  # mid-low tier
        cone(0.58, 0.16, 0.90, (-0.03, 0.05, 2.86), rot=(0, 0, math.radians(9)), verts=10),   # mid-high tier
        cone(0.42, 0.11, 0.85, (0.03, -0.02, 3.28), rot=(0, 0, math.radians(24)), verts=10),  # upper tier
        cone(0.27, 0.0, 0.78, (-0.02, 0.02, 3.70), rot=(0, 0, math.radians(6)), verts=8),     # crown point
    ]
    return weld("tree", parts, mat, bevel=0.0)


def build_rock():
    mat = make_material("rock", rgba("rock"))  # grey boulder
    # A cluster of faceted icospheres (deterministic — the old UV sphere tessellated differently each
    # run), each squashed, tilted, and offset so the silhouette is an irregular cleaved boulder, not
    # a ball. No chamfer — the raw flat-shaded triangular facets are exactly the sharp fractured-stone
    # read we want (and a bevel on a sphere bevels every facet edge, ballooning the tri count for no
    # aesthetic gain).
    #
    # WS-F tier-4 lift (visual-design-plan §WS-F — "light touch; already fine"): sharpen the cleaved
    # read without leaving the faceted-primitive lever. (1) A tall angular SHARD lobe rises off the
    # main mass so the boulder has a jutting broken edge, not a smooth dome; (2) a fourth shed CHIP at
    # the far foot spreads the debris field and breaks the last of the radial symmetry; (3) the main
    # mass is squashed a touch flatter + tilted harder so it reads as a cleaved slab. Subdivisions
    # stay at 1 — the coarse facets ARE the fractured-stone look.
    main = icosphere(0.90, (0, 0, 0.50), subdivisions=1)
    main.dimensions = (1.92, 1.40, 0.98)               # squash to a cleaved slab, base near z=0
    main.rotation_euler = (0, math.radians(11), math.radians(20))  # tilt the cleavage plane harder
    shard = icosphere(0.50, (-0.10, 0.20, 0.72), subdivisions=1)
    shard.dimensions = (0.72, 0.62, 1.02)              # tall jutting broken shard off the top
    shard.rotation_euler = (math.radians(-14), math.radians(9), math.radians(35))
    spur = icosphere(0.55, (0.62, -0.32, 0.36), subdivisions=1)
    spur.dimensions = (1.02, 0.80, 0.72)               # offset lobe — breaks the symmetry
    spur.rotation_euler = (math.radians(10), 0, math.radians(-15))
    chip = icosphere(0.34, (-0.58, 0.30, 0.24), subdivisions=1)
    chip.dimensions = (0.64, 0.56, 0.44)               # small shed chip at the foot
    chip2 = icosphere(0.26, (0.44, 0.52, 0.20), subdivisions=1)
    chip2.dimensions = (0.52, 0.44, 0.34)              # second shed fragment — spreads the debris field
    chip2.rotation_euler = (math.radians(18), 0, math.radians(40))
    return weld("rock", [main, shard, spur, chip, chip2], mat)


def build_barricade():
    mat = make_material("barricade", rgba("barricade"))  # sandbag berm cover
    # A stacked sandbag berm: discrete bags laid in three offset (running-bond) courses. Each bag is
    # a flattened, chamfered box rotated a few degrees off-axis and dipped in height, so the course
    # sags and bulges like real filled bags rather than a tidy brick wall. A deterministic per-bag
    # wobble (indexed, not random) keeps the regen bit-reproducible.
    #
    # WS-F tier-3 lift (visual-design-plan §WS-F): tighten the read from "spaced chocolate bars" to a
    # packed berm. (1) Courses now OVERLAP in x (bag width > pitch) so there are no gaps at the bevel
    # seams; (2) each bag is nudged fore/aft (±y) so the wall face is a bulging stack, not one flat
    # plane; (3) the bevel eases 0.09 → 0.07 — still a soft filled-bag pillow, but crisp enough not to
    # read as molten. The sandbag silhouette identity is unchanged.
    parts = []
    # (x centres per course). Pitch < bag width → neighbours overlap into a continuous packed wall.
    lower = [-1.00, -0.50, 0.0, 0.50, 1.00]      # widest base course (5 bags)
    upper = [-0.75, -0.25, 0.25, 0.75]           # mid course, running-bond offset (4 bags)
    top = [-0.50, 0.0, 0.50]                      # short crest course (3 bags)
    for i, x in enumerate(lower):
        sag = 0.02 if i % 2 else -0.02
        fb = 0.05 if i % 2 else -0.05            # fore/aft nudge → bulging face, not a flat plane
        parts.append(box((0.60, 0.76, 0.30 + sag), (x, fb, 0.15 + sag * 0.5),
                         rot=(0, 0, math.radians(4 if i % 2 else -3))))  # lower course bag
    for i, x in enumerate(upper):
        sag = -0.02 if i % 2 else 0.015
        fb = -0.05 if i % 2 else 0.05
        parts.append(box((0.58, 0.66, 0.28 + sag), (x, fb, 0.44 + sag * 0.5),
                         rot=(0, 0, math.radians(-4 if i % 2 else 5))))  # mid course bag (running bond)
    for i, x in enumerate(top):
        fb = 0.04 if i % 2 else -0.04
        parts.append(box((0.52, 0.56, 0.24), (x, fb, 0.68),
                         rot=(0, 0, math.radians(3 if i % 2 else -4))))  # short top crest course
    return weld("barricade", parts, mat, bevel=0.07)


# --- Faction cosmetic silhouettes (factions-plan WS-C, D68) -----------------------------------
# Per-army, presentation-only variants of the headline archetypes. The renderer maps
# `(Army, kind) → ModelKind` (render/src/lib.rs::model_for_unit) exactly as it maps bare `UnitKind`,
# so a US-side rifleman draws `trooper_us` and a French tank draws `tank_fr` (+ `tank_turret_fr`).
# These NEVER reach `core` — silhouettes/names add zero checksum surface (invariant #1/#7 untouched).
# Geometry stays in the SAME local frame as the shared kin it replaces (Z-up, base z≈0; tank barrel
# +X with the turret-ring pivot at the hull origin; rifle receiver at origin, barrel +X) so the
# existing placement/token math (`token_meshes`, `weapon_view_model`) works unchanged.


def build_trooper_us():
    # US Army infantry — a Vietnam-era GI, a shade bulkier through the chest/shoulders than the Neutral
    # kin (the fairness-bounded US silhouette tell). Same skinned skeleton + M1 steel pot; adds a frag
    # grenade on the belt and a tropical rucksack with a bedroll. Olive (OCP-era). Presentation-only —
    # never reaches `core` (invariant #1/#7 untouched).
    P = infantry_palette("us", (0.31, 0.35, 0.20), (0.20, 0.23, 0.15))
    parts = soldier_parts(P, bulk=1.12, helmet="pot")
    parts += [
        (box((0.08, 0.08, 0.11), (0.17, -0.20, 1.03)), P["gun"]),             # frag grenade on the belt (US tell)
        (box((0.17, 0.28, 0.38), (-0.20, 0.0, 1.26)), P["pack"]),             # tropical rucksack (US carries more, −X)
        (cyl(0.055, 0.28, (-0.215, 0.0, 1.47), rot=(math.radians(90), 0, 0), verts=8), P["pack"]),  # bedroll (±Y)
    ]
    return weld("trooper_us", parts, bevel=0.0)


def build_trooper_fr():
    # French Army infantry (FELIN) — slimmer through the chest/shoulders than the US kin, and a flatter,
    # front-brimmed SPECTRA helmet instead of the rounded steel pot (the fairness-bounded FR silhouette
    # tell). Same skinned skeleton; a compact backpack. French army green fatigues. Presentation-only —
    # never reaches `core`.
    P = infantry_palette("fr", (0.27, 0.31, 0.20), (0.18, 0.21, 0.15))
    parts = soldier_parts(P, bulk=0.92, helmet="spectra")
    parts += [
        (box((0.14, 0.24, 0.34), (-0.175, 0.0, 1.26)), P["pack"]),            # compact backpack (tucked, −X)
    ]
    return weld("trooper_fr", parts, bevel=0.0)


def build_tank_us():
    # M1 Abrams HULL: long, low, flat, wide — the WS-F tier-2 lever applied with the Abrams silhouette
    # tell (a broad chassis and a famously LONG SHALLOW glacis, ~20°). Boolean glacis milled into the
    # upper hull, a boolean sponson undercut, a distinct road-wheel run (7 wheels + larger idler/
    # sprocket), and a tight 0.018 bevel — same crisp language as the Neutral hull, kept distinctly
    # Abrams. Turret is the separate `tank_turret_us` model (slews independently, P7). Pivot at origin.
    mat = make_material("tank_us", rgba("tank_us"))
    lower = box((3.5, 1.6, 0.42), (0, 0, 0.40))                # low broad tub
    upper = box((3.55, 2.15, 0.36), (0, 0, 0.72))              # low wide upper hull (overhangs tracks)
    boolean_cut(upper, [box((6.0, 3.2, 4.0), (2.46, 0, 2.42), rot=(0, math.radians(20), 0))])  # long shallow glacis
    boolean_cut(upper, [box((3.2, 0.36, 0.18), (0, 0.99, 0.56)),
                        box((3.2, 0.36, 0.18), (0, -0.99, 0.56))])  # sponson undercut
    parts = [
        lower, upper,
        box((0.42, 1.6, 0.30), (-1.72, 0, 0.56), rot=(0, math.radians(-16), 0)),  # rear plate
    ]
    parts += running_gear(
        track_y=1.00, wheel_z=0.30,
        wheels=((1.62, 0.27), (1.02, 0.23), (0.42, 0.23), (-0.18, 0.23),
                (-0.78, 0.23), (-1.38, 0.23), (-1.72, 0.27)),
        track_dims=(3.7, 0.46, 0.42), track_z=0.30,
        fender_dims=(3.8, 0.52, 0.10), fender_z=0.60,
    )
    return weld("tank_us", parts, mat, bevel=0.018)


def build_tank_turret_us():
    # M1 Abrams TURRET: large, low, wide, angular slab with a long 120 mm gun. Pivot at hull origin,
    # barrel +X (turret_yaw 0 == hull 0); seated on the hull top (z≈0.85).
    mat = make_material("tank_turret_us", rgba("tank_turret_us"))
    parts = [
        cyl(0.68, 0.12, (0, 0, 0.80), verts=12),               # ring base (drops into the hull socket)
        box((2.0, 1.7, 0.55), (-0.15, 0, 1.05)),               # broad flat turret
        box((0.7, 1.7, 0.30), (1.05, 0, 1.05), rot=(0, math.radians(-12), 0)),  # sloped gun mantlet
        box((1.1, 1.5, 0.16), (-0.7, 0, 0.86)),                # rear turret bustle rack (low, broad)
        cyl(0.10, 2.10, (1.5, 0, 1.05), rot=(0, math.radians(90), 0)),  # long 120mm barrel, +X
        box((0.5, 0.5, 0.18), (-0.9, 0.5, 1.40)),              # commander's cupola/CITV
        cyl(0.09, 0.22, (-0.5, -0.55, 1.42), verts=8),         # loader's M240 MG
        cyl(0.13, 0.20, (0.45, 0, 1.05), rot=(0, math.radians(90), 0), verts=12),  # bore-evacuator collar
    ]
    return weld("tank_turret_us", parts, mat, bevel=0.04)


def build_tank_fr():
    # Leclerc HULL: compact and taller than the Abrams, with a distinctly STEEPER glacis (~40°) — the
    # WS-F tier-2 lever with the Leclerc silhouette tell. Boolean glacis + sponson undercut, a distinct
    # 6-wheel run, a sloped rear plate, and the tight 0.018 bevel. Separate turret model (P7).
    mat = make_material("tank_fr", rgba("tank_fr"))
    lower = box((2.95, 1.5, 0.46), (0, 0, 0.44))               # compact tub
    upper = box((3.0, 1.95, 0.40), (0, 0, 0.80))               # taller upper hull (overhangs tracks)
    boolean_cut(upper, [box((5.0, 3.0, 4.0), (2.79, 0, 2.13), rot=(0, math.radians(40), 0))])  # steep glacis
    boolean_cut(upper, [box((2.7, 0.34, 0.20), (0, 0.90, 0.62)),
                        box((2.7, 0.34, 0.20), (0, -0.90, 0.62))])  # sponson undercut
    parts = [
        lower, upper,
        box((0.5, 1.5, 0.34), (-1.46, 0, 0.62), rot=(0, math.radians(-20), 0)),  # sloped rear plate
    ]
    parts += running_gear(
        track_y=0.90, wheel_z=0.31,
        wheels=((1.28, 0.27), (0.70, 0.22), (0.15, 0.22), (-0.40, 0.22), (-0.95, 0.22), (-1.42, 0.27)),
        track_dims=(3.1, 0.44, 0.44), track_z=0.31,
        fender_dims=(3.2, 0.50, 0.10), fender_z=0.63,
    )
    return weld("tank_fr", parts, mat, bevel=0.018)


def build_tank_turret_fr():
    # Leclerc TURRET: cleaner, taller box with a prominent rear bustle (autoloader) — a distinctly
    # different silhouette from the Abrams' broad flat turret. Pivot at hull origin, barrel +X.
    mat = make_material("tank_turret_fr", rgba("tank_turret_fr"))
    parts = [
        cyl(0.62, 0.12, (0, 0, 0.83), verts=12),               # ring base (drops into the hull socket)
        box((1.5, 1.3, 0.62), (-0.10, 0, 1.10)),               # main turret box (taller)
        box((0.5, 1.1, 0.30), (0.85, 0, 1.06), rot=(0, math.radians(-12), 0)),  # sloped gun mantlet
        box((1.0, 1.4, 0.55), (-1.05, 0, 1.05)),               # rear bustle (autoloader) — overhangs
        cyl(0.09, 1.90, (1.35, 0, 1.12), rot=(0, math.radians(90), 0)),  # 120mm barrel, +X
        box((0.4, 0.4, 0.40), (-0.3, 0.45, 1.55)),             # roof sight mast (the Leclerc tell)
        cyl(0.08, 0.20, (-0.3, -0.45, 1.42), verts=8),         # remote MG mount
        cyl(0.12, 0.20, (0.42, 0, 1.12), rot=(0, math.radians(90), 0), verts=12),  # bore-evacuator collar
    ]
    return weld("tank_turret_fr", parts, mat, bevel=0.04)


def build_weapon_rifle_us():
    # M4 carbine viewmodel: conventional layout — receiver at origin, barrel forward (+X), magazine
    # BELOW/forward of the grip, collapsible stock to the rear, flat-top rail on top. Modelled in the
    # same frame as `weapon_rifle` so `weapon_view_model` re-bases +X→forward unchanged.
    mat = make_material("weapon_rifle_us", rgba("weapon_rifle_us"))
    receiver = box((0.46, 0.06, 0.11), (0.0, 0, 0))            # upper/lower receiver
    boolean_cut(receiver, [box((0.11, 0.02, 0.05), (0.075, 0.031, 0.012))])  # ejection-port pocket (+Y)

    rail = box((0.40, 0.05, 0.04), (0.0, 0, 0.085))            # flat-top picatinny rail
    boolean_cut(rail, picatinny_slots(0.02, 0.19, 0.025, 0.108, 3)
                      + picatinny_slots(-0.19, -0.11, 0.025, 0.108, 1))  # milled recoil grooves

    # Ribbed M4 handguard: smooth core + rib bands (the WS-F "ribbed" read the old smooth tube missed).
    handguard = [cyl(0.032, 0.20, (0.30, 0, -0.02), rot=(0, math.radians(90), 0), verts=12)]
    for rx in (0.23, 0.31, 0.39):
        handguard.append(cyl(0.040, 0.018, (rx, 0, -0.02), rot=(0, math.radians(90), 0), verts=8))

    # Skeletonized collapsible stock — the distinctive M4 lightened buttstock, via a boolean slot.
    stock = box((0.22, 0.05, 0.10), (-0.34, 0, 0.0))
    boolean_cut(stock, [box((0.12, 0.08, 0.044), (-0.355, 0, 0.0))])

    magwell = box((0.10, 0.062, 0.085), (0.02, 0, -0.065))     # flared STANAG mag well (seats the mag)

    parts = [
        receiver, rail,
        box((0.07, 0.05, 0.05), (-0.05, 0, 0.13)),             # optic body (low-profile red-dot)
        box((0.05, 0.055, 0.025), (-0.05, 0, 0.165)),          # optic hood
        box((0.04, 0.048, 0.014), (-0.065, 0, 0.145)),         # optic lens bezel (front face)
        cyl(0.018, 0.46, (0.42, 0, 0), rot=(0, math.radians(90), 0)),  # barrel (forward)
        *handguard,
        magwell,
        box((0.06, 0.05, 0.19), (0.02, 0, -0.145)),            # STANAG magazine (forward of grip)
        box((0.07, 0.05, 0.14), (-0.10, 0, -0.10), rot=(0, math.radians(-14), 0)),  # pistol grip
        stock,
        box((0.045, 0.03, 0.02), (-0.185, 0, 0.09)),           # charging-handle latch (rear-top)
        box((0.02, 0.03, 0.06), (0.40, 0, 0.05)),              # front sight post
    ]
    return weld("weapon_rifle_us", parts, mat, bevel=0.006)


def build_weapon_rifle_fr():
    # FAMAS bullpup viewmodel: the headline-distinct rifle silhouette — magazine BEHIND the grip
    # (toward the stock), a tall full-length carry handle on top, short overall. Receiver at origin,
    # barrel +X, same frame as `weapon_rifle`.
    mat = make_material("weapon_rifle_fr", rgba("weapon_rifle_fr"))
    body = box((0.50, 0.07, 0.15), (-0.05, 0, 0))              # bullpup body (action sits at the rear)
    # Ejection-port pocket on the +Y side (bullpup port sits well aft), plus a proud cocking-handle
    # slot milled under the carry handle up front.
    boolean_cut(body, [box((0.10, 0.02, 0.05), (-0.16, 0.036, 0.02))])

    # The FAMAS tell — the tall full-length carry handle — milled with a longitudinal sight channel
    # down its top (the iron-sight trough), so it reads as a real sighting rib, not a plain slab.
    handle = box((0.34, 0.03, 0.10), (-0.02, 0, 0.16))
    boolean_cut(handle, [box((0.28, 0.016, 0.035), (-0.02, 0, 0.205))])

    # Vented forward handguard under the barrel — two cooling slots cut through, the classic FAMAS
    # ribbed forestock read (boolean lever, not a smooth block).
    foregrip = box((0.15, 0.075, 0.075), (0.22, 0, -0.055))
    boolean_cut(foregrip, [box((0.03, 0.09, 0.04), (0.19, 0, -0.05)),
                           box((0.03, 0.09, 0.04), (0.26, 0, -0.05))])

    magwell = box((0.075, 0.062, 0.06), (-0.20, 0, -0.05))     # flared mag well (seats the bullpup mag)

    parts = [
        body, handle,
        box((0.02, 0.03, 0.10), (0.15, 0, 0.11)),              # front handle post
        box((0.02, 0.03, 0.10), (-0.19, 0, 0.11)),             # rear handle post
        box((0.045, 0.045, 0.03), (-0.19, 0, 0.075)),          # rear aperture sight (in the trough)
        cyl(0.016, 0.34, (0.38, 0, 0.0), rot=(0, math.radians(90), 0)),  # thin barrel (forward)
        foregrip,
        magwell,
        box((0.05, 0.05, 0.15), (-0.20, 0, -0.125)),           # magazine BEHIND the grip (bullpup)
        box((0.06, 0.05, 0.13), (0.02, 0, -0.10), rot=(0, math.radians(-10), 0)),  # pistol grip (forward of mag)
    ]
    return weld("weapon_rifle_fr", parts, mat, bevel=0.006)


def build_turret_us():
    # US Army defensive emplacement — WS-F tier-4 / faction variant of the neutral `turret`. Reads as
    # a heavy US crew-served gun position: a low, broad, bolted-armour housing with an M2 .50-cal-style
    # heavy MG on a pintle, a big rectangular ammo can, and a squared face shield with a boxy vision
    # slit. Same mechanical/architectural lever as tier 3 (booleans + tuned bevels), same emplacement
    # footprint/pivot as the neutral turret, but a distinctly US-heavy silhouette. CARC grey-green.
    mat = make_material("turret_us", rgba("turret_us"))
    # Squared armoured face shield (broad, low) with a boxy boolean vision slit — the US "armour slab".
    shield = box((0.44, 1.16, 0.52), (0.42, 0, 1.16), rot=(0, math.radians(-6), 0))
    boolean_cut(shield, [box((0.24, 0.40, 0.12), (0.60, 0, 1.24), rot=(0, math.radians(-6), 0))])  # vision slit
    # Heavy MG barrel with a perforated jacket (boolean cooling slots) + a slotted flash hider — the
    # M2/.50-cal read, chunkier than the neutral turret's slim gun.
    jacket = cyl(0.10, 0.44, (0.70, 0, 1.14), rot=(0, math.radians(90), 0), verts=10)
    boolean_cut(jacket, [box((0.30, 0.06, 0.24), (0.70, 0, 1.14)),   # transverse cooling slots (perforated jacket)
                         box((0.30, 0.24, 0.06), (0.70, 0, 1.14))])
    flash = cyl(0.09, 0.14, (1.16, 0, 1.14), rot=(0, math.radians(90), 0), verts=8)
    boolean_cut(flash, [box((0.05, 0.24, 0.06), (1.16, 0, 1.14)),    # slotted flash hider
                        box((0.05, 0.06, 0.24), (1.16, 0, 1.14))])
    parts = [
        box((1.7, 1.7, 0.40), (0, 0, 0.20)),                   # broad base pad
        box((1.3, 1.3, 0.16), (0, 0, 0.48)),                   # bolted ring plate
        cyl(0.60, 0.60, (0, 0, 0.72), verts=12),               # rotating drum (low + broad)
        box((0.90, 1.00, 0.50), (-0.10, 0, 1.12)),             # heavy bolted gun housing (low, wide)
        shield,
        box((0.20, 0.12, 0.44), (0.46, 0.46, 1.14)),           # pintle trunnion arm L
        box((0.20, 0.12, 0.44), (0.46, -0.46, 1.14)),          # pintle trunnion arm R
        box((0.46, 0.40, 0.34), (-0.34, 0.42, 1.10)),          # big rectangular ammo can (side)
        box((0.30, 0.24, 0.14), (0.12, 0, 1.46)),              # spade/butterfly grips + backplate (top-rear)
        box((0.30, 0.36, 0.20), (-0.26, 0, 1.44)),             # boxy optic/sensor block (on top)
        cyl(0.06, 0.24, (0.50, 0, 1.14), rot=(0, math.radians(90), 0), verts=8),  # receiver stub
        cyl(0.05, 0.30, (1.00, 0, 1.14), rot=(0, math.radians(90), 0), verts=8),  # barrel (jacket → flash hider)
        jacket, flash,
    ]
    return weld("turret_us", parts, mat, bevel=0.02)


def build_turret_fr():
    # French Army defensive emplacement — WS-F tier-4 / faction variant of the neutral `turret`. Reads
    # as a sleeker modern REMOTE weapon station (RWS): a compact stabilised gun pod carried high on a
    # narrow slewing mast, a boxed thermal-sight housing beside the gun, and a slim barrel — no crew
    # shield (it's remote-operated), the deliberate silhouette contrast with the US crew-served gun.
    # Same emplacement footprint/pivot; French army green.
    mat = make_material("turret_fr", rgba("turret_fr"))
    # Slim gun with a stepped muzzle (boolean-cut ports) — a modern medium autocannon read.
    barrel = cyl(0.055, 0.90, (0.66, 0, 1.52), rot=(0, math.radians(90), 0), verts=10)
    muzzle = cyl(0.075, 0.14, (1.14, 0, 1.52), rot=(0, math.radians(90), 0), verts=10)
    boolean_cut(muzzle, [box((0.05, 0.22, 0.05), (1.14, 0, 1.52)),
                         box((0.05, 0.05, 0.22), (1.14, 0, 1.52))])  # ported muzzle
    # Angular gun cradle with a milled recess where the barrel seats (the sculpted RWS look).
    cradle = box((0.52, 0.40, 0.30), (0.22, 0, 1.52))
    boolean_cut(cradle, [box((0.40, 0.16, 0.16), (0.34, 0, 1.58))])  # barrel trough
    parts = [
        cyl(0.62, 0.28, (0, 0, 0.14), verts=12),               # low circular base pad
        cyl(0.40, 0.20, (0, 0, 0.36), verts=12),               # slewing ring
        box((0.44, 0.44, 0.90), (-0.05, 0, 0.86), rot=(0, math.radians(4), 0)),  # narrow slewing mast (carries the pod high)
        box((0.56, 0.72, 0.34), (0.0, 0, 1.42)),               # compact stabilised gun pod
        cradle,
        box((0.34, 0.34, 0.30), (-0.18, 0.40, 1.44), rot=(0, math.radians(-8), 0)),  # boxed thermal-sight housing (side)
        box((0.10, 0.10, 0.22), (-0.20, -0.36, 1.60)),         # laser/RF sensor stalk (opposite side)
        box((0.30, 0.20, 0.16), (-0.28, 0, 1.30)),             # rear electronics/counterweight box
        barrel, muzzle,
    ]
    return weld("turret_fr", parts, mat, bevel=0.018)


MODELS = [
    ("trooper", build_trooper,
     "Infantry unit — an organic skinned humanoid (skeleton + Skin modifier) in fatigues cradling an "
     "M16 under an M1 steel-pot helmet, with a rucksack + bedroll."),
    ("tank", build_tank,
     "Greybox vehicle hull — chassis + tracks (turret is a separate model so it slews independently)."),
    ("tank_turret", build_tank_turret,
     "Greybox tank turret — gun mantlet + barrel, pivoting about the hull's turret ring (P7)."),
    ("camp_hq", build_camp_hq,
     "Greybox structure — command building: hipped roof + ridge vent, pilaster-framed recessed "
     "doorway under an entrance awning, flanking windows, rooftop vent + antenna mast."),
    ("weapon_rifle", build_weapon_rifle,
     "First-person weapon viewmodel — receiver, barrel, magazine, stock, grip."),
    ("crate", build_crate,
     "Cover prop — a 1m crate."),
    ("turret", build_turret,
     "Defensive structure — weapon emplacement: base pad + ring plate, rotating drum, armoured gun "
     "housing with a sloped shield, elevation trunnions, sensor block, barrel + muzzle brake."),
    ("tree", build_tree,
     "Scenery / soft cover — conifer: tapered trunk + four staggered, rotated cone tiers."),
    ("rock", build_rock,
     "Scenery / hard cover — a faceted three-lobe boulder (tilted, irregular silhouette)."),
    ("barricade", build_barricade,
     "Cover prop — a three-course sagging sandbag berm (offset running bond, per-bag wobble)."),
    ("tracer", build_tracer,
     "Tank-shell tracer — a small +X-elongated bolt, placed at the shell and yawed by velocity (P7)."),
    # Faction cosmetic silhouettes (WS-C, D68) — presentation-only per-army variants.
    ("trooper_us", build_trooper_us,
     "US Army infantry — skinned GI, bulkier build, M1 steel pot, frag grenade + rucksack (WS-C)."),
    ("trooper_fr", build_trooper_fr,
     "French Army infantry — skinned soldier, slimmer build, flatter brimmed SPECTRA helmet (WS-C)."),
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
    ("turret_us", build_turret_us,
     "US Army emplacement — heavy crew-served .50-cal: low broad bolted housing, perforated-jacket "
     "gun + slotted flash hider, big ammo can, squared shield (WS-F/WS-C)."),
    ("turret_fr", build_turret_fr,
     "French Army emplacement — remote weapon station: compact stabilised gun pod on a narrow "
     "slewing mast, boxed thermal sight, slim ported gun, no crew shield (WS-F/WS-C)."),
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
