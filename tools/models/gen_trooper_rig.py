#!/usr/bin/env python3
# Going Dark — trooper animation rig + clip authoring (CP-3 / visual-design-plan WS-B).
#
# The *floor* animation deliverable: a rigged greybox trooper carrying four short clips
# (idle / walk / fire / death), exported to a single glTF (`.glb`) that CARRIES REAL ANIMATION
# CHANNELS. Companion to `gen_models.py` (which bakes the static `.mesh` the engine loads today);
# this script bakes the *animated* interchange source the eventual skeletal player will consume.
#
# Method — a RIGID-PART rig (decisions.md D41 script-not-binary, same as gen_models.py):
#   * the trooper is built from distinct box parts (pelvis / torso / head / helmet / arms / legs /
#     rifle), NOT one organic skinned blob;
#   * a small bone hierarchy drives them, and each part is bound RIGIDLY to exactly ONE bone
#     (a single vertex group at weight 1.0 + one Armature modifier) — so every clip is pure
#     per-bone TRS with NO soft vertex weights. This is deliberately the cheap floor: it needs no
#     custom skinning shader (full soft vertex-skinning is explicitly OUT OF SCOPE for this slice).
#   * four Actions are keyframed and pushed to their own NLA tracks so the glTF exporter emits each
#     as a separate, named animation.
#
# Output (committed, per D41 — the generator script is the source of record, the `.glb` its
# regenerable artifact, provenance in the manifest):
#   assets/models/rigs/trooper_rig.glb        — the rigged mesh + 4 animation clips
#   assets/models/rigs/manifest.json          — source / license / sha256 / per-clip frame ranges
#
# NOTE — this artifact is NOT loaded at runtime yet. Today's runtime "animation" is the procedural
# pose in `render::anim` (bob / lean / recoil), driven by the SAME clip-selection seam
# (`render::anim::select_clip`) so a real skeletal player slots in behind it later. Wiring a glTF
# skeletal/rigid-part loader that consumes THIS file is the owed follow-up (visual-design-plan WS-B).
#
# Run headless:
#     blender --background --python tools/models/gen_trooper_rig.py
#     # or:  pnpm assets:rig
#
# Deterministic + license-clean by construction: code-authored geometry + keyframes, CC0-1.0.

import bpy
import os
import json
import math
import struct
import hashlib

try:
    HERE = os.path.dirname(os.path.abspath(__file__))
except NameError:  # pragma: no cover
    HERE = os.getcwd()
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
OUT_DIR = os.path.join(REPO, "assets", "models", "rigs")

AUTHOR = os.environ.get("GONEDARK_ASSET_AUTHOR", "Jared Howard")
LICENSE = "CC0-1.0"

# Olive greybox tints, mirroring gen_models.py's neutral trooper palette (presentation only).
FATIGUE = (0.30, 0.34, 0.20, 1.0)
HELMET = (0.20, 0.23, 0.15, 1.0)
SKIN = (0.60, 0.45, 0.33, 1.0)
GUN = (0.08, 0.08, 0.09, 1.0)


# --- scene helpers --------------------------------------------------------------------
def reset_scene():
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for block in (bpy.data.meshes, bpy.data.materials, bpy.data.objects, bpy.data.armatures,
                  bpy.data.actions):
        for item in list(block):
            if getattr(item, "users", 0) == 0:
                block.remove(item)


def mat(name, rgba):
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    m.use_nodes = True
    bsdf = m.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = rgba
    bsdf.inputs["Roughness"].default_value = 0.85
    m.diffuse_color = rgba
    return m


def box(name, dims, loc, material):
    bpy.ops.mesh.primitive_cube_add(size=1.0, location=loc)
    o = bpy.context.active_object
    o.name = name
    o.dimensions = dims
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=True)
    o.data.materials.append(material)
    return o


# --- the rig --------------------------------------------------------------------------
# Bone hierarchy (Z-up, feet at z≈0), each part bound rigidly to ONE bone. Head/tail place the
# joint; the bone's Y axis runs head→tail. `parts` lists (part-name, bone) so each mesh box is
# weighted 1.0 to that bone's vertex group.
BONES = [
    # name,        head,              tail,               parent
    ("pelvis",     (0.0, 0.0, 0.92),  (0.0, 0.0, 1.10),   None),
    ("spine",      (0.0, 0.0, 1.10),  (0.0, 0.0, 1.45),   "pelvis"),
    ("head",       (0.0, 0.0, 1.50),  (0.0, 0.0, 1.70),   "spine"),
    ("arm_L",      (0.20, 0.0, 1.44), (0.12, 0.30, 1.15), "spine"),
    ("arm_R",      (-0.20, 0.0, 1.44),(-0.12, 0.30, 1.15),"spine"),
    ("leg_L",      (0.10, 0.0, 0.90), (0.11, 0.0, 0.06),  "pelvis"),
    ("leg_R",      (-0.10, 0.0, 0.90),(-0.11, 0.0, 0.06), "pelvis"),
]


def build_armature():
    arm_data = bpy.data.armatures.new("trooper_rig")
    arm_obj = bpy.data.objects.new("trooper_rig", arm_data)
    bpy.context.scene.collection.objects.link(arm_obj)
    bpy.context.view_layer.objects.active = arm_obj
    bpy.ops.object.mode_set(mode="EDIT")
    ebs = arm_data.edit_bones
    for name, head, tail, parent in BONES:
        b = ebs.new(name)
        b.head = head
        b.tail = tail
        if parent:
            b.parent = ebs[parent]
    bpy.ops.object.mode_set(mode="OBJECT")
    return arm_obj


def build_parts():
    fatigue = mat("rig_fatigue", FATIGUE)
    helmet = mat("rig_helmet", HELMET)
    skin = mat("rig_skin", SKIN)
    gun = mat("rig_gun", GUN)
    # (part object, bone it is rigidly bound to)
    return [
        (box("pelvis_m", (0.34, 0.22, 0.24), (0.0, 0.0, 1.00), fatigue), "pelvis"),
        (box("torso_m", (0.40, 0.24, 0.42), (0.0, -0.01, 1.28), fatigue), "spine"),
        (box("head_m", (0.20, 0.20, 0.22), (0.0, 0.01, 1.60), skin), "head"),
        (box("helmet_m", (0.26, 0.26, 0.14), (0.0, 0.0, 1.71), helmet), "head"),
        (box("armL_m", (0.12, 0.34, 0.14), (0.16, 0.14, 1.28), fatigue), "arm_L"),
        (box("armR_m", (0.12, 0.34, 0.14), (-0.16, 0.14, 1.28), fatigue), "arm_R"),
        (box("legL_m", (0.15, 0.24, 0.90), (0.10, 0.02, 0.47), fatigue), "leg_L"),
        (box("legR_m", (0.15, 0.24, 0.90), (-0.10, 0.02, 0.47), fatigue), "leg_R"),
        # M16 cradled across the chest, bound to the right (firing) arm so recoil reads on it.
        (box("rifle_m", (0.60, 0.06, 0.07), (0.0, 0.34, 1.16), gun), "arm_R"),
    ]


def bind_part(part, bone, arm_obj):
    """Rigid single-bone bind: one vertex group == the bone, all verts weight 1.0, one Armature
    modifier. Every vertex follows exactly one bone (no soft weights) → pure per-bone TRS."""
    vg = part.vertex_groups.new(name=bone)
    vg.add(range(len(part.data.vertices)), 1.0, "REPLACE")
    m = part.modifiers.new("armature", "ARMATURE")
    m.object = arm_obj
    part.parent = arm_obj


# --- clip authoring -------------------------------------------------------------------
# Each clip is an Action of pose-bone keyframes. Rotations are Euler XYZ in the bone's local frame
# (Y = along the bone). Kept short + subtle: this is the "not jarring" floor, not mocap.
def key_bone(arm_obj, bone, frame, loc=None, rot=None):
    pb = arm_obj.pose.bones[bone]
    pb.rotation_mode = "XYZ"
    if rot is not None:
        pb.rotation_euler = [math.radians(a) for a in rot]
        pb.keyframe_insert("rotation_euler", frame=frame)
    if loc is not None:
        pb.location = loc
        pb.keyframe_insert("location", frame=frame)


def reset_pose(arm_obj):
    for pb in arm_obj.pose.bones:
        pb.rotation_mode = "XYZ"
        pb.rotation_euler = (0.0, 0.0, 0.0)
        pb.location = (0.0, 0.0, 0.0)


def new_action(arm_obj, name):
    reset_pose(arm_obj)
    act = bpy.data.actions.new(name)
    if arm_obj.animation_data is None:
        arm_obj.animation_data_create()
    arm_obj.animation_data.action = act
    return act


def author_idle(arm_obj):
    # Slow breathing: pelvis rises a touch, spine settles, over a symmetric 1..48 loop.
    new_action(arm_obj, "idle")
    for f, dz in ((1, 0.0), (24, 0.02), (48, 0.0)):
        key_bone(arm_obj, "pelvis", f, loc=(0.0, 0.0, dz))
        key_bone(arm_obj, "spine", f, rot=(2.0 * (dz > 0.0), 0.0, 0.0))
    return (1, 48)


def author_walk(arm_obj):
    # Alternating stride: thighs swing about local X, arms counter-swing, pelvis bobs — a 1..24 loop.
    new_action(arm_obj, "walk")
    swing = 26.0
    arm = 18.0
    frames = [
        (1, swing, -swing, -arm, arm, 0.0),
        (7, 0.0, 0.0, 0.0, 0.0, 0.03),
        (13, -swing, swing, arm, -arm, 0.0),
        (19, 0.0, 0.0, 0.0, 0.0, 0.03),
        (24, swing, -swing, -arm, arm, 0.0),
    ]
    for f, lL, lR, aL, aR, bob in frames:
        key_bone(arm_obj, "leg_L", f, rot=(lL, 0.0, 0.0))
        key_bone(arm_obj, "leg_R", f, rot=(lR, 0.0, 0.0))
        key_bone(arm_obj, "arm_L", f, rot=(aL, 0.0, 0.0))
        key_bone(arm_obj, "arm_R", f, rot=(aR, 0.0, 0.0))
        key_bone(arm_obj, "pelvis", f, loc=(0.0, 0.0, bob))
    return (1, 24)


def author_fire(arm_obj):
    # Recoil pulse: right (firing) arm kicks back + spine leans back, then settles — a 1..12 clip.
    new_action(arm_obj, "fire")
    for f, kick, lean in ((1, 0.0, 0.0), (3, -22.0, -6.0), (12, 0.0, 0.0)):
        key_bone(arm_obj, "arm_R", f, rot=(kick, 0.0, 0.0))
        key_bone(arm_obj, "spine", f, rot=(lean, 0.0, 0.0))
    return (1, 12)


def author_death(arm_obj):
    # Topple: the whole body pitches forward at the pelvis and drops — a 1..24 one-shot.
    new_action(arm_obj, "death")
    for f, pitch, drop in ((1, 0.0, 0.0), (10, 55.0, -0.30), (24, 88.0, -0.62)):
        key_bone(arm_obj, "pelvis", f, rot=(pitch, 0.0, 0.0), loc=(0.0, 0.0, drop))
    return (1, 24)


def push_to_nla(arm_obj, act, track_name):
    """Stash an action on its own NLA track so the glTF exporter emits it as a named animation."""
    adata = arm_obj.animation_data
    track = adata.nla_tracks.new()
    track.name = track_name
    track.strips.new(act.name, int(act.frame_range[0]), act)
    adata.action = None  # clear the active action so only NLA strips define the clips


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def glb_animation_names(path):
    """Parse a `.glb`'s JSON chunk and return its animation names — a self-check that the clips
    actually survived export as glTF animation channels."""
    with open(path, "rb") as f:
        magic, _ver, _len = struct.unpack("<4sII", f.read(12))
        assert magic == b"glTF", "not a glb"
        chunk_len, chunk_type = struct.unpack("<II", f.read(8))
        assert chunk_type == 0x4E4F534A, "first chunk is not JSON"
        doc = json.loads(f.read(chunk_len))
    return [a.get("name", "") for a in doc.get("animations", [])]


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    reset_scene()

    arm_obj = build_armature()
    parts = build_parts()
    for part, bone in parts:
        bind_part(part, bone, arm_obj)

    bpy.context.view_layer.objects.active = arm_obj
    clips = []
    for author, name in (
        (author_idle, "idle"),
        (author_walk, "walk"),
        (author_fire, "fire"),
        (author_death, "death"),
    ):
        act = new_action(arm_obj, name)
        start, end = author(arm_obj)
        push_to_nla(arm_obj, bpy.data.actions[name], name)
        clips.append({"name": name, "frame_start": start, "frame_end": end})
    reset_pose(arm_obj)

    # Export the armature + all rigidly-bound parts with every NLA clip as a glTF animation.
    glb_name = "trooper_rig.glb"
    glb_path = os.path.join(OUT_DIR, glb_name)
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.export_scene.gltf(
        filepath=glb_path,
        export_format="GLB",
        use_selection=True,
        export_animations=True,
        export_animation_mode="NLA_TRACKS",
        export_nla_strips=True,
        export_apply=False,  # keep the armature; applying modifiers would strip the rig
    )

    names = glb_animation_names(glb_path)
    assert len(names) == 4, f"expected 4 animation clips in the glb, got {names}"

    manifest = {
        "note": (
            "Rigged greybox trooper + animation clips (idle/walk/fire/death), generated by "
            "tools/models/gen_trooper_rig.py (decisions.md D41). The `.glb` carries real glTF "
            "animation channels on a rigid-part rig (each box part bound 1.0 to a single bone — no "
            "soft vertex skinning). This is the CP-3 animation FLOOR authoring artifact; it is not "
            "loaded at runtime yet — today's playback is the procedural pose in render::anim, driven "
            "by the same clip-selection seam. Regenerate with `pnpm assets:rig`. License-clean by "
            "construction (code-authored geometry + keyframes, CC0-1.0)."
        ),
        "license_default": LICENSE,
        "assets": [
            {
                "name": "trooper_rig",
                "category": "rigs",
                "file": "rigs/" + glb_name,
                "description": (
                    "Rigid-part greybox trooper (pelvis/torso/head/helmet/arms/legs/rifle) on a "
                    "7-bone hierarchy, carrying four clips: idle, walk, fire, death."
                ),
                "source": "procedural (Blender bpy — tools/models/gen_trooper_rig.py)",
                "generator": bpy.app.version_string,
                "author": AUTHOR,
                "license": LICENSE,
                "url": "",
                "bytes": os.path.getsize(glb_path),
                "sha256": sha256(glb_path),
                "clips": clips,
                "animation_names": names,
            }
        ],
    }
    with open(os.path.join(OUT_DIR, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"  wrote {glb_name}  ({manifest['assets'][0]['bytes']} B, clips={names})")
    print("  wrote manifest.json")


if __name__ == "__main__":
    main()
