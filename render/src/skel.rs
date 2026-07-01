//! Runtime skeletal playback of the authored trooper rig (CP-3 / WS-B, D84 follow-up).
//!
//! This is the *runtime skeletal player* owed by [D84](../../docs/decisions.md): it consumes the
//! authored rig ([`tools/models/gen_trooper_rig.py`]) and plays the clip picked by
//! [`crate::anim::select_clip`] on the trooper, superseding the procedural pose stand-in for the
//! generic infantry token.
//!
//! ## Why a cooked `.skel`, not glТF at runtime
//! The render crate stays `wgpu` + `bytemuck` only (D19/D46) — it carries **no** glTF reader. So,
//! exactly like the static `.mesh` cook ([`crate::mesh`]), the rig is baked to a dead-simple,
//! hand-parseable cooked file (`assets/models/rigs/trooper_rig.skel`, magic `GDSK`) that is
//! `include_bytes!`d in. The generator bakes it straight from the Blender scene alongside the
//! interchange `.glb`; provenance (`cooked_sha256`) lives in the rig manifest (script-not-binary,
//! D41).
//!
//! ## Rigid-part skinning — one matrix per part, no skinning shader
//! The rig is **rigid-part** (D84): every box part is bound 1.0 to exactly one bone, so there is no
//! soft vertex skinning. Runtime "skinning" is therefore a single matrix per part: each part draws
//! as an ordinary instanced [`crate::mesh::MeshInstance`] through the existing
//! [`crate::mesh::MeshPipeline`] at
//!
//! ```text
//!   model = place · A_bone(t) · inverse_bind[bone]
//! ```
//!
//! where `place` is the token's world placement ([`crate::mesh::model_matrix`]), `inverse_bind` is
//! the bone's rest inverse (baked in), and `A_bone(t)` is the bone's armature-space pose sampled from
//! the clip at time `t`. No new vertex attribute, no joint-matrix storage buffer, no shader change —
//! this reuses the mesh pipeline verbatim, which also keeps the merge surface tiny.
//!
//! ## Presentation-only (invariant #1 / #4)
//! Everything here is float render glue: it reads the presentation [`crate::anim::AnimClip`] (already
//! classified from a snapshot copy of `vel`/`firing`/alive) plus a phase clock, and emits float
//! matrices. No `core`/sim type is touched, nothing enters the checksum fold.
//!
//! ## Honest floor caveats (same as D84)
//! `Death` is baked + selectable + tested but not *driven* at runtime: dead units are dropped from
//! the render snapshot (`core::snapshot`), so a visible death topple needs cross-tick unit identity +
//! a linger — still owed, and deliberately out of scope here (it would need a sim-side change).
//! Idle / Walk / Fire play. Only the generic [`crate::mesh::ModelKind::Trooper`] is rig-driven; the
//! faction silhouettes (`TrooperUs`/`TrooperFr`) keep the procedural [`crate::anim`] pose (a
//! per-faction rig is future work), so this stays additive and merge-safe.

use crate::anim::AnimClip;
use crate::mesh::{MeshCpu, MeshInstance, MeshVertex};

/// Magic bytes at the head of a cooked `.skel` (GDSK) file. See the module docs + the format doc in
/// `tools/models/gen_trooper_rig.py`.
pub const SKEL_MAGIC: [u8; 4] = *b"GDSK";

/// Floats per baked joint pose: `t[3] + r[4](xyzw) + s[3]` = 10 × `f32`.
const TRS_FLOATS: usize = 10;
/// Bytes per cooked skinned vertex — the GDM2 layout `[px,py,pz,nx,ny,nz,cr,cg,cb,cm]` (10 × f32).
const VERTEX_BYTES: usize = 40;

/// A per-joint local transform sampled from a clip: translation, rotation quaternion `(x,y,z,w)`,
/// and scale — all in **armature space** (the bake pre-composes the hierarchy, so playback needs no
/// parent walk). Pure float presentation data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointTrs {
    pub t: [f32; 3],
    /// Rotation quaternion, `(x, y, z, w)`.
    pub r: [f32; 4],
    pub s: [f32; 3],
}

impl JointTrs {
    /// The identity transform (no translation, identity rotation, unit scale).
    pub const IDENTITY: JointTrs = JointTrs {
        t: [0.0, 0.0, 0.0],
        r: [0.0, 0.0, 0.0, 1.0],
        s: [1.0, 1.0, 1.0],
    };
}

/// One skeleton joint: its parent (for reference; the bake stores armature-space poses so playback
/// never walks the hierarchy) and its rest **inverse-bind** matrix (column-major).
#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    /// Index of the parent joint, or `-1` for a root. Informational only at runtime.
    pub parent: i32,
    /// Rest inverse-bind matrix, column-major `[[f32; 4]; 4]` (outer index = column).
    pub inv_bind: [[f32; 4]; 4],
}

/// One rigid-bound part: the bone it follows + its bind-space greybox geometry (reusing the shared
/// [`MeshCpu`] triangle soup, so upload/draw is identical to the static meshes).
#[derive(Clone, Debug, PartialEq)]
pub struct RigPart {
    /// Index into [`SkeletonCpu::joints`] of the single bone this part is rigidly bound to.
    pub joint: usize,
    /// Bind/armature-space geometry (Z-up world metres) — the part at rest.
    pub mesh: MeshCpu,
}

/// A named animation clip: baked per-frame, per-joint armature-space [`JointTrs`] at a fixed `fps`.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub name: String,
    /// `true` = the clip loops (idle/walk stride, fire recoil pulse); `false` = play once and hold
    /// the final frame (death topple).
    pub loops: bool,
    /// Frames per second the clip was baked at.
    pub fps: f32,
    /// `frames[frame][joint]` — every frame carries a pose for every joint.
    pub frames: Vec<Vec<JointTrs>>,
}

/// The parsed cooked rig: skeleton, rigid parts, and clips. Pure data (no GPU handle), so all of the
/// sampling / skinning math below is unit-testable off-GPU against the committed `.skel`.
#[derive(Clone, Debug, PartialEq)]
pub struct SkeletonCpu {
    pub joints: Vec<Joint>,
    pub parts: Vec<RigPart>,
    pub clips: Vec<Clip>,
}

/// Why a cooked `.skel` blob failed to parse. A typed failure beats a panic deep in the loader —
/// though in practice the only input is our own committed, golden-tested file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkelParseError {
    TooShort,
    BadMagic,
    BadVersion,
    /// Declared counts / lengths don't add up, or an index points out of range.
    Malformed,
}

/// Little-endian byte cursor over the cooked blob. Returns typed errors instead of panicking on a
/// short read.
struct Cursor<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> Cursor<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cursor { b, o: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], SkelParseError> {
        let end = self.o.checked_add(n).ok_or(SkelParseError::Malformed)?;
        if end > self.b.len() {
            return Err(SkelParseError::Malformed);
        }
        let s = &self.b[self.o..end];
        self.o = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, SkelParseError> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Result<i32, SkelParseError> {
        Ok(self.u32()? as i32)
    }
    fn f32(&mut self) -> Result<f32, SkelParseError> {
        let s = self.take(4)?;
        Ok(f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn mat4(&mut self) -> Result<[[f32; 4]; 4], SkelParseError> {
        let mut m = [[0.0f32; 4]; 4];
        for col in &mut m {
            for v in col.iter_mut() {
                *v = self.f32()?;
            }
        }
        Ok(m)
    }
}

impl SkeletonCpu {
    /// Parse a cooked `.skel` blob (GDSK; see the module docs). Validates the magic + version and
    /// that every declared count fits the blob, so a corrupt file is a typed error, not a surprise.
    pub fn parse(bytes: &[u8]) -> Result<SkeletonCpu, SkelParseError> {
        if bytes.len() < 4 {
            return Err(SkelParseError::TooShort);
        }
        if bytes[0..4] != SKEL_MAGIC {
            return Err(SkelParseError::BadMagic);
        }
        let mut c = Cursor::new(bytes);
        c.take(4)?; // magic
        let version = c.u32()?;
        if version != 1 {
            return Err(SkelParseError::BadVersion);
        }
        let joint_count = c.u32()? as usize;
        let part_count = c.u32()? as usize;
        let clip_count = c.u32()? as usize;

        let mut joints = Vec::with_capacity(joint_count);
        for _ in 0..joint_count {
            let parent = c.i32()?;
            let inv_bind = c.mat4()?;
            joints.push(Joint { parent, inv_bind });
        }

        let mut parts = Vec::with_capacity(part_count);
        for _ in 0..part_count {
            let joint = c.u32()? as usize;
            if joint >= joint_count {
                return Err(SkelParseError::Malformed);
            }
            let v_count = c.u32()? as usize;
            let i_count = c.u32()? as usize;
            let mut vertices = Vec::with_capacity(v_count);
            for _ in 0..v_count {
                vertices.push(MeshVertex {
                    pos: [c.f32()?, c.f32()?, c.f32()?],
                    normal: [c.f32()?, c.f32()?, c.f32()?],
                    color: [c.f32()?, c.f32()?, c.f32()?, c.f32()?],
                });
            }
            let mut indices = Vec::with_capacity(i_count);
            for _ in 0..i_count {
                let idx = c.u32()?;
                if idx as usize >= v_count {
                    return Err(SkelParseError::Malformed);
                }
                indices.push(idx);
            }
            parts.push(RigPart {
                joint,
                mesh: MeshCpu { vertices, indices },
            });
        }

        let mut clips = Vec::with_capacity(clip_count);
        for _ in 0..clip_count {
            let name_len = c.u32()? as usize;
            let name_bytes = c.take(name_len)?;
            let name = std::str::from_utf8(name_bytes)
                .map_err(|_| SkelParseError::Malformed)?
                .to_string();
            let loops = c.u32()? != 0;
            let fps = c.f32()?;
            let frame_count = c.u32()? as usize;
            let mut frames = Vec::with_capacity(frame_count);
            for _ in 0..frame_count {
                let mut pose = Vec::with_capacity(joint_count);
                for _ in 0..joint_count {
                    pose.push(JointTrs {
                        t: [c.f32()?, c.f32()?, c.f32()?],
                        r: [c.f32()?, c.f32()?, c.f32()?, c.f32()?],
                        s: [c.f32()?, c.f32()?, c.f32()?],
                    });
                }
                frames.push(pose);
            }
            clips.push(Clip {
                name,
                loops,
                fps,
                frames,
            });
        }

        // Silence the unused-const lints if the layout ever changes; they document the byte sizes.
        let _ = (TRS_FLOATS, VERTEX_BYTES);
        Ok(SkeletonCpu {
            joints,
            parts,
            clips,
        })
    }

    /// Find the clip index for an [`AnimClip`] by its baked name (`idle`/`walk`/`fire`/`death`).
    /// Returns `None` if the rig lacks that clip (a defensive fallback path, not expected in
    /// practice — the committed rig has all four).
    pub fn clip_index(&self, clip: AnimClip) -> Option<usize> {
        let want = match clip {
            AnimClip::Idle => "idle",
            AnimClip::Walk => "walk",
            AnimClip::Fire => "fire",
            AnimClip::Death => "death",
        };
        self.clips.iter().position(|c| c.name == want)
    }

    /// Per-joint skin matrices `A_bone(t) · inverse_bind` (column-major) for a sampled `pose`. A part
    /// bound to joint `j` draws at `place · skin[j]`. Pure; unit-tested.
    pub fn skin_matrices(&self, pose: &[JointTrs]) -> Vec<[[f32; 4]; 4]> {
        self.joints
            .iter()
            .enumerate()
            .map(|(j, joint)| {
                let a = trs_to_mat(pose.get(j).copied().unwrap_or(JointTrs::IDENTITY));
                mat4_mul(a, joint.inv_bind)
            })
            .collect()
    }

    /// Build one [`MeshInstance`] per rig part for a token: `model = place · skin[part.joint]`, with
    /// the given `color` (faction tint; `a` = emissive/flash, 0 for tokens). `clip` picks the track;
    /// `time` (seconds) is the playback clock (see [`crate::anim::unit_phase`], ≈ seconds at 60 Hz).
    /// Returns instances in [`SkeletonCpu::parts`] order (aligned with the GPU part meshes). Pure +
    /// GPU-free, so the whole skinning path is unit-tested without a device.
    pub fn part_instances(
        &self,
        place: [[f32; 4]; 4],
        color: [f32; 4],
        clip: AnimClip,
        time: f32,
    ) -> Vec<MeshInstance> {
        let pose = match self.clip_index(clip).and_then(|i| self.clips.get(i)) {
            Some(c) => c.sample(time),
            None => vec![JointTrs::IDENTITY; self.joints.len()],
        };
        let skin = self.skin_matrices(&pose);
        self.parts
            .iter()
            .map(|part| MeshInstance {
                model: mat4_mul(place, skin[part.joint]),
                color,
            })
            .collect()
    }
}

impl Clip {
    /// Total playback length in seconds. A looping clip spans all `frame_count` intervals (the bake
    /// dropped the duplicate seam frame, so wrapping frame `n-1 → 0` is the real continuing motion);
    /// a one-shot spans `frame_count - 1` intervals (it holds the final frame).
    pub fn duration(&self) -> f32 {
        let n = self.frames.len();
        if n <= 1 || self.fps <= 0.0 {
            return 0.0;
        }
        let intervals = if self.loops { n } else { n - 1 };
        intervals as f32 / self.fps
    }

    /// Sample the per-joint pose at `time` seconds. Looping clips wrap; one-shots clamp to (and hold)
    /// the final frame. Between the two bracketing frames, translation/scale lerp and rotation
    /// nlerps. Pure + total (empty/degenerate clips return identity), unit-tested off-GPU.
    pub fn sample(&self, time: f32) -> Vec<JointTrs> {
        let n = self.frames.len();
        if n == 0 {
            return Vec::new();
        }
        if n == 1 {
            return self.frames[0].clone();
        }
        // Position on the frame timeline in [0, n) for a loop, [0, n-1] for a one-shot.
        let fpos = if self.fps > 0.0 { time * self.fps } else { 0.0 };
        let (i0, i1, frac) = if self.loops {
            let span = n as f32;
            let mut p = fpos % span;
            if p < 0.0 {
                p += span; // wrap negative time cleanly
            }
            let i0 = p.floor() as usize % n;
            let i1 = (i0 + 1) % n;
            (i0, i1, p - p.floor())
        } else {
            let last = (n - 1) as f32;
            let p = fpos.clamp(0.0, last);
            let i0 = (p.floor() as usize).min(n - 1);
            let i1 = (i0 + 1).min(n - 1);
            (i0, i1, p - p.floor())
        };
        let a = &self.frames[i0];
        let b = &self.frames[i1];
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| JointTrs {
                t: lerp3(x.t, y.t, frac),
                r: quat_nlerp(x.r, y.r, frac),
                s: lerp3(x.s, y.s, frac),
            })
            .collect()
    }
}

// --- pure math (all float; render side of invariant #1) -------------------------------------------

/// Multiply two column-major `[[f32; 4]; 4]` matrices (`a · b`), matching the convention of
/// [`crate::mesh::model_matrix`] (outer index = column). Pure; unit-tested.
#[inline]
pub fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut m = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k][row] * b[col][k];
            }
            m[col][row] = s;
        }
    }
    m
}

/// Build a column-major TRS matrix `T · R · S` from a [`JointTrs`] (quaternion `(x,y,z,w)`). The
/// inverse of Blender's `Matrix.decompose`, so `trs_to_mat(decompose(M)) ≈ M`. Pure; unit-tested.
#[inline]
pub fn trs_to_mat(trs: JointTrs) -> [[f32; 4]; 4] {
    let [x, y, z, w] = trs.r;
    let [sx, sy, sz] = trs.s;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    // Rotation columns (images of the basis vectors), each scaled by the matching scale component.
    [
        [
            sx * (1.0 - 2.0 * (yy + zz)),
            sx * (2.0 * (xy + wz)),
            sx * (2.0 * (xz - wy)),
            0.0,
        ],
        [
            sy * (2.0 * (xy - wz)),
            sy * (1.0 - 2.0 * (xx + zz)),
            sy * (2.0 * (yz + wx)),
            0.0,
        ],
        [
            sz * (2.0 * (xz + wy)),
            sz * (2.0 * (yz - wx)),
            sz * (1.0 - 2.0 * (xx + yy)),
            0.0,
        ],
        [trs.t[0], trs.t[1], trs.t[2], 1.0],
    ]
}

/// Component-wise lerp of two 3-vectors.
#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Normalized-lerp of two quaternions `(x,y,z,w)` — cheap, stable, and plenty for the "not jarring"
/// floor (the per-frame steps are small). Takes the shorter arc (negates `b` on a negative dot) and
/// re-normalizes; falls back to `a` if the result degenerates. Pure; unit-tested.
#[inline]
pub fn quat_nlerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let mut q = [0.0f32; 4];
    for i in 0..4 {
        q[i] = a[i] + (b[i] * sign - a[i]) * t;
    }
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len > 1e-8 {
        [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
    } else {
        a
    }
}

// --- GPU-resident rig -----------------------------------------------------------------------------

/// The cooked trooper rig, loaded once and drawn through the shared [`crate::mesh::MeshPipeline`].
/// Holds the parsed [`SkeletonCpu`] (for sampling) plus one uploaded GPU mesh per rigid part. The
/// draw paths call [`SkeletonCpu::part_instances`] to get the per-part model matrices and batch each
/// part's instances against [`TrooperRig::part_mesh`].
pub struct TrooperRig {
    pub cpu: SkeletonCpu,
    parts: Vec<crate::mesh::MeshGpu>,
}

impl TrooperRig {
    /// Parse + upload the committed cooked rig (`assets/models/rigs/trooper_rig.skel`), embedded at
    /// build time so it rides into the binary/APK with no runtime file IO (same as the meshes). The
    /// committed file is golden-tested, so `parse` cannot fail here in practice; an unexpected error
    /// panics loudly rather than silently dropping the rig.
    pub fn load(device: &wgpu::Device) -> Self {
        const BYTES: &[u8] = include_bytes!("../../assets/models/rigs/trooper_rig.skel");
        let cpu = SkeletonCpu::parse(BYTES).expect("cooked trooper_rig.skel must parse");
        let parts = cpu
            .parts
            .iter()
            .map(|p| crate::mesh::MeshGpu::upload(device, &p.mesh, "gonedark.rig_part"))
            .collect();
        TrooperRig { cpu, parts }
    }

    /// The number of rigid parts (aligned with [`SkeletonCpu::part_instances`] output order).
    pub fn part_count(&self) -> usize {
        self.parts.len()
    }

    /// Borrow the GPU mesh for part `i` (in [`SkeletonCpu::parts`] order).
    pub fn part_mesh(&self, i: usize) -> &crate::mesh::MeshGpu {
        &self.parts[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    /// The committed cooked rig — the golden fixture every runtime test parses.
    const RIG_BYTES: &[u8] = include_bytes!("../../assets/models/rigs/trooper_rig.skel");

    fn approx(a: [[f32; 4]; 4], b: [[f32; 4]; 4], eps: f32) -> bool {
        (0..4).all(|c| (0..4).all(|r| (a[c][r] - b[c][r]).abs() < eps))
    }

    const IDENT: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    // ---- pure math ----

    #[test]
    fn mat4_mul_identity_is_neutral() {
        let m = [
            [1.0, 2.0, 3.0, 0.0],
            [4.0, 5.0, 6.0, 0.0],
            [7.0, 8.0, 9.0, 0.0],
            [10.0, 11.0, 12.0, 1.0],
        ];
        assert!(approx(mat4_mul(IDENT, m), m, EPS));
        assert!(approx(mat4_mul(m, IDENT), m, EPS));
    }

    #[test]
    fn mat4_mul_composes_like_model_matrix() {
        // place · translate-by-v should shift the translation column into place's frame.
        let place = crate::mesh::model_matrix([5.0, 0.0, 0.0], 1.0, std::f32::consts::FRAC_PI_2);
        let local = crate::mesh::model_matrix([1.0, 0.0, 0.0], 1.0, 0.0); // +1 along local X
        let m = mat4_mul(place, local);
        // place yaws +90° (X→Y) then translates +5X; local's +1X becomes +1Y in world, plus the +5X.
        assert!((m[3][0] - 5.0).abs() < EPS, "x translation {}", m[3][0]);
        assert!((m[3][1] - 1.0).abs() < EPS, "y translation {}", m[3][1]);
    }

    #[test]
    fn trs_identity_is_identity_matrix() {
        assert!(approx(trs_to_mat(JointTrs::IDENTITY), IDENT, EPS));
    }

    #[test]
    fn trs_translation_and_scale() {
        let m = trs_to_mat(JointTrs {
            t: [1.0, 2.0, 3.0],
            r: [0.0, 0.0, 0.0, 1.0],
            s: [2.0, 3.0, 4.0],
        });
        assert_eq!([m[3][0], m[3][1], m[3][2]], [1.0, 2.0, 3.0]);
        assert!((m[0][0] - 2.0).abs() < EPS); // scale x
        assert!((m[1][1] - 3.0).abs() < EPS); // scale y
        assert!((m[2][2] - 4.0).abs() < EPS); // scale z
    }

    #[test]
    fn trs_quarter_turn_about_z_maps_x_to_y() {
        // 90° about +Z: quaternion (0,0,sin45,cos45).
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let m = trs_to_mat(JointTrs {
            t: [0.0, 0.0, 0.0],
            r: [0.0, 0.0, s, s],
            s: [1.0, 1.0, 1.0],
        });
        // Column 0 is the image of local +X → should be +Y.
        assert!((m[0][0]).abs() < EPS && (m[0][1] - 1.0).abs() < EPS, "X→Y: {:?}", m[0]);
    }

    #[test]
    fn quat_nlerp_endpoints_and_normalization() {
        let a = [0.0, 0.0, 0.0, 1.0];
        let b = [0.0, 0.0, 1.0, 0.0];
        assert_eq!(quat_nlerp(a, b, 0.0), a);
        // t=1 → b (already unit).
        let e = quat_nlerp(a, b, 1.0);
        assert!((e[2] - 1.0).abs() < EPS && e[3].abs() < EPS);
        // Any interpolant stays unit-length.
        for i in 0..=10 {
            let q = quat_nlerp(a, b, i as f32 / 10.0);
            let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            assert!((len - 1.0).abs() < EPS, "nlerp stays unit, got {len}");
        }
    }

    #[test]
    fn quat_nlerp_takes_shorter_arc() {
        // a and -a are the same rotation; nlerp must not blow toward the antipode.
        let a = [0.0, 0.0, 0.0, 1.0];
        let neg = [0.0, 0.0, 0.0, -1.0];
        let mid = quat_nlerp(a, neg, 0.5);
        // Shorter arc → stays at a (unit), not a degenerate zero.
        let len = (mid[0] * mid[0] + mid[1] * mid[1] + mid[2] * mid[2] + mid[3] * mid[3]).sqrt();
        assert!((len - 1.0).abs() < EPS);
    }

    // ---- parser ----

    #[test]
    fn parses_committed_rig() {
        let s = SkeletonCpu::parse(RIG_BYTES).expect("committed rig parses");
        assert_eq!(s.joints.len(), 7, "7-bone hierarchy");
        assert_eq!(s.parts.len(), 9, "9 rigid box parts");
        assert_eq!(s.clips.len(), 4, "idle/walk/fire/death");
        // Every part binds to a valid joint and carries a non-empty flat-shaded soup.
        for p in &s.parts {
            assert!(p.joint < s.joints.len());
            assert!(!p.mesh.vertices.is_empty());
            assert_eq!(p.mesh.vertices.len() % 3, 0, "3 verts / triangle");
            assert_eq!(p.mesh.indices.len(), p.mesh.vertices.len());
        }
        // Clips present, named, non-empty, every frame poses every joint.
        for name in ["idle", "walk", "fire", "death"] {
            let c = s.clips.iter().find(|c| c.name == name).expect("clip present");
            assert!(!c.frames.is_empty(), "{name} has frames");
            assert!(c.fps > 0.0);
            for f in &c.frames {
                assert_eq!(f.len(), s.joints.len(), "{name} frame poses every joint");
            }
        }
    }

    #[test]
    fn rejects_bad_blobs() {
        assert_eq!(SkeletonCpu::parse(&[0u8; 2]), Err(SkelParseError::TooShort));
        let mut bad = RIG_BYTES.to_vec();
        bad[0] = b'X';
        assert_eq!(SkeletonCpu::parse(&bad), Err(SkelParseError::BadMagic));
        let mut ver = RIG_BYTES.to_vec();
        ver[4] = 9; // version bump
        assert_eq!(SkeletonCpu::parse(&ver), Err(SkelParseError::BadVersion));
        let truncated = &RIG_BYTES[..RIG_BYTES.len() - 8];
        assert_eq!(SkeletonCpu::parse(truncated), Err(SkelParseError::Malformed));
    }

    // ---- clip sampling ----

    #[test]
    fn idle_at_time_zero_is_the_bind_pose() {
        // The idle clip's first frame is authored at rest (no keyed offsets), so its skin matrices
        // must be (near-)identity — the load-bearing check that inv_bind and the TRS bake agree.
        let s = SkeletonCpu::parse(RIG_BYTES).unwrap();
        let idle = s.clips.iter().find(|c| c.name == "idle").unwrap();
        let pose = idle.sample(0.0);
        let skin = s.skin_matrices(&pose);
        for (j, m) in skin.iter().enumerate() {
            assert!(approx(*m, IDENT, 2e-3), "joint {j} skin ≈ identity at idle t=0: {m:?}");
        }
    }

    #[test]
    fn sampling_is_bounded_and_finite_across_all_clips() {
        // Sweep every clip well past its end; skin matrices must stay finite and near the token (no
        // NaN, no fling) — the stand-in can never throw a part off-screen.
        let s = SkeletonCpu::parse(RIG_BYTES).unwrap();
        for c in &s.clips {
            for k in 0..200 {
                let t = k as f32 * 0.05; // 0..10 s, several loops / past one-shot end
                let pose = c.sample(t);
                assert_eq!(pose.len(), s.joints.len());
                let skin = s.skin_matrices(&pose);
                for m in &skin {
                    for col in m {
                        for v in col {
                            assert!(v.is_finite(), "{} skin finite", c.name);
                        }
                    }
                    // Translation column stays within a metre or so of the rig origin.
                    assert!(
                        m[3][0].abs() < 3.0 && m[3][1].abs() < 3.0 && m[3][2].abs() < 3.0,
                        "{} skin translation bounded: {:?}",
                        c.name,
                        m[3]
                    );
                }
            }
        }
    }

    #[test]
    fn walk_actually_moves_the_legs_over_the_cycle() {
        // A locomotion clip must not be static: some joint's pose changes materially across the loop.
        let s = SkeletonCpu::parse(RIG_BYTES).unwrap();
        let walk = s.clips.iter().find(|c| c.name == "walk").unwrap();
        let p0 = walk.sample(0.0);
        let pmid = walk.sample(walk.duration() * 0.5);
        let moved = p0
            .iter()
            .zip(pmid.iter())
            .any(|(a, b)| (a.r[0] - b.r[0]).abs() + (a.r[1] - b.r[1]).abs() > 0.05);
        assert!(moved, "walk animates the skeleton over its cycle");
    }

    #[test]
    fn one_shot_holds_final_frame_past_the_end() {
        // death is a one-shot: sampling past its end clamps to (and holds) the last frame.
        let s = SkeletonCpu::parse(RIG_BYTES).unwrap();
        let death = s.clips.iter().find(|c| c.name == "death").unwrap();
        assert!(!death.loops);
        let end = death.sample(death.duration());
        let past = death.sample(death.duration() + 5.0);
        for (a, b) in end.iter().zip(past.iter()) {
            assert!((a.t[2] - b.t[2]).abs() < EPS, "death holds its final drop");
            assert!((a.r[0] - b.r[0]).abs() < EPS, "death holds its final topple");
        }
    }

    #[test]
    fn loop_wraps_cleanly() {
        // A looping clip sampled at exactly one full duration returns to (near) its start pose.
        let s = SkeletonCpu::parse(RIG_BYTES).unwrap();
        let idle = s.clips.iter().find(|c| c.name == "idle").unwrap();
        let p0 = idle.sample(0.0);
        let pwrap = idle.sample(idle.duration());
        for (a, b) in p0.iter().zip(pwrap.iter()) {
            assert!((a.t[2] - b.t[2]).abs() < 5e-3, "idle loop returns near start");
        }
    }

    // ---- clip selection + part instancing ----

    #[test]
    fn clip_index_maps_every_anim_clip() {
        let s = SkeletonCpu::parse(RIG_BYTES).unwrap();
        for (clip, name) in [
            (AnimClip::Idle, "idle"),
            (AnimClip::Walk, "walk"),
            (AnimClip::Fire, "fire"),
            (AnimClip::Death, "death"),
        ] {
            let idx = s.clip_index(clip).expect("clip present");
            assert_eq!(s.clips[idx].name, name);
        }
    }

    #[test]
    fn part_instances_places_every_part_at_the_token() {
        // One instance per part, all carrying the token colour and grounded near the token's world
        // placement (skin ≈ identity at idle t=0, so parts sit at place · bind).
        let s = SkeletonCpu::parse(RIG_BYTES).unwrap();
        let place = crate::mesh::model_matrix([10.0, -4.0, 0.0], super::super::TOKEN_SCALE, 0.7);
        let color = [0.2, 0.5, 0.8, 0.0];
        let insts = s.part_instances(place, color, AnimClip::Idle, 0.0);
        assert_eq!(insts.len(), s.parts.len(), "one instance per part");
        for it in &insts {
            assert_eq!(it.color, color, "token tint on every part");
            for v in it.model.iter().flatten() {
                assert!(v.is_finite());
            }
            // The part's world origin sits within a couple of metres of the token centre.
            let d = ((it.model[3][0] - 10.0).powi(2) + (it.model[3][1] + 4.0).powi(2)).sqrt();
            assert!(d < 2.0, "part near the token centre, got {d}");
        }
    }

    #[test]
    fn firing_and_walking_produce_different_poses() {
        // Different clips → different skinned output (the seam actually drives distinct animation).
        let s = SkeletonCpu::parse(RIG_BYTES).unwrap();
        let place = crate::mesh::model_matrix([0.0, 0.0, 0.0], 1.0, 0.0);
        let c = [1.0, 1.0, 1.0, 0.0];
        let walk = s.part_instances(place, c, AnimClip::Walk, 0.25);
        let fire = s.part_instances(place, c, AnimClip::Fire, 0.25);
        let differ = walk
            .iter()
            .zip(fire.iter())
            .any(|(a, b)| !approx(a.model, b.model, 1e-3));
        assert!(differ, "walk and fire skin to different poses");
    }
}
