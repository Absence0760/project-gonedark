# Visual-design plan — push the look & feel toward near-final

> **Status: IN PROGRESS.** The **foundation + first wave have landed** ([D74](../decisions.md) plus a
> run of render/asset commits on 2026-06-30): a single `render::theme` palette/type/space
> source-of-truth, an anti-aliased font atlas (replacing the 5×7 bitmap), dimensional greybox
> lighting + a cinematic present grade, rounded-card panel chrome, ground detail textures + a richer
> sky, art-directed chamfered meshes, Inkscape-baked command-bar HUD icons, a landing screen anchored
> over a live 3D backdrop, and a scripted "Going Dark" launcher icon. What remains is the **game-feel
> and readability** layer — the part a Delta Force / CoH player judges in seconds — sequenced below.
> This is the execution doc behind the roadmap's *Visual-design pass* item and the **CP-2 / CP-3 /
> CP-9** competitive-parity rows. **Everything here is presentation-only** — it lives strictly on the
> render/float side of [invariant #1/#4](../../CLAUDE.md), touches no `core`/sim type, and adds no
> checksum surface; [invariant #6](../../CLAUDE.md) (no strategic intel while embodied; an accessibility
> equivalent for the directional flash+audio alert) bounds every new cue.

---

## Why this exists

The sim is correct and the loop is playable (D31, D37–D40, D64), but the *look* read as greybox: a
per-module colour sprawl with no shared identity, an uppercase-only bitmap font, flat lighting, and a
flat floor. [D74](../decisions.md) stood up the first deliberate art-direction layer (theme + font),
and a wave of follow-on render/asset work this cycle took the rest of the static presentation
(lighting, present grade, panel chrome, textures, meshes, HUD icons, the landing/launcher surfaces)
from "prototype tell" to "intentional."

What is **not** yet done is the half that doesn't show up in a screenshot: how firing *feels*, how the
embodied view *moves*, and whether the command layer is *legible at a glance on a small screen*. Those
are the [`positioning.md`](../positioning/positioning.md) table-stakes the storefront judges us on —
CP-2 (game-feel), CP-3 (animation floor, a *conceded* tier we only need "not jarring" on), and CP-9
(command readability). This plan scopes them as one body of work so the visual push finishes coherently
rather than getting smuggled in piecemeal. It pairs with — but does **not** subsume — the audio
identity pass ([CP-6](../roadmap.md), [`content-pipeline.md`](../content-pipeline.md)); audio is its
own scripted-asset workstream (Csound/SoX) and the going-dark alert channel, cross-referenced here only
where a cue is audio-coupled.

All new assets follow the **script-not-binary** rule ([D41](../decisions.md)/[D46](../decisions.md)):
commit the generator script + a `manifest.json` provenance entry (`source`/`license`/`sha256`), never
an opaque blob — exactly as `tools/fonts/gen_hud_font.py` (D74) and `tools/models/gen_models.py` do.

---

## What has landed (WS-0 — recorded, not to redo)

| Piece | Where | Decision / commit |
|---|---|---|
| Central theme (palette / type scale / spacing), aligned to the desktop title-shell palette | `render::theme`, `app/src/shell.rs` | [D74](../decisions.md) |
| Anti-aliased monospace font atlas (ASCII 0x20–0x7E), raw R8 `include_bytes!` | `render::text`, `tools/fonts/gen_hud_font.py`, `assets/fonts/` | [D74](../decisions.md) |
| Dimensional greybox lighting + cinematic present grade | `render` | 2026-06-30 |
| Rounded-card in-session panel chrome | `render::overlay` | 2026-06-30 |
| Ground detail textures + richer sky (kills the flat-floor look) | `render`, ImageMagick | 2026-06-30 |
| Art-directed meshes — chamfers + silhouette detail | `tools/models/gen_models.py`, Blender | 2026-06-30 |
| Command-bar HUD icons via Inkscape-baked atlas | `render`, Inkscape | 2026-06-30 |
| Landing screen as anchored HUD over a live 3D backdrop | `app`/`render` | 2026-06-30 |
| Scripted "Going Dark" launcher icon (aperture going dark) | `assets/app-icon`, Inkscape | 2026-06-30 |

These are verified the established way: the offscreen `viz-runner` (`pnpm desktop:viz`) renders the
real `Game::frame` to PNG for eyeball + agent visual inspection and pixel-asserts the HUD invariants
(all green on the RTX 3070). **Follow-on workstreams keep that bar** — each lands with the relevant viz
PNG checked and any new pixel-assert added.

---

## Workstreams (remaining)

Built in isolated worktrees, merged in the order below; each lands with the workspace suite green
dev+release, the `viz-runner` assertions green (GPU-gated, local — never the no-GPU CI matrix), and —
since none of this touches `core` — the determinism matrix trivially unaffected.

### WS-A — Embodied game-feel pass *(CP-2, launch-critical)*

The focused gunplay pass so a shooter player doesn't bounce in ten seconds. Hit feedback already
landed (TF-4: hitmarker + hit SFX off the avatar-source `SimEvent::Damaged` stream); this is the rest.

- **Recoil / kick readability** — a presentation-only view-kick + crosshair bloom on fire, decaying
  per frame (render/host state, never sim). Read off the existing fire cadence; no new sim signal.
- **Responsive ADS** — a snappy aim-down-sight transition (FOV + sensitivity ramp) on the existing
  zoom button (wave-2 W6). Tune the curve; verify it reads on a phone-sized viewport.
- **Muzzle / impact VFX** — promote the debug muzzle flash to a real shaped flash + a brief impact
  spark/dust at the hit point (derived from the avatar's own `SimEvent::Damaged`, invariant-#6-safe).
- **Audio-coupled firing** — fire/impact cues fire in lockstep with the visual (hand-off to CP-6 for
  the actual sound identity; this WS owns only the *coupling timing*).
- **Written "good-enough floor"** + a playtest against it (the CP-2 acceptance gate).
- **Files:** `engine` (view-kick/ADS/feedback seams as pure testable fns, like `fire`/`avatar_landed_hit`), `render` (flash/impact/crosshair passes), `pal` audio cue timing.
- **Acceptance:** a `viz-runner` scene pixel-asserts the kick/flash on a firing frame; the feel-floor
  doc + a playtest sign-off (the human-feel half, carried honestly, not faked).

### WS-B — Animation floor *(CP-3, conceded tier — "not jarring", not UE5 parity)*

> **Status: FLOOR SLICE LANDED ([D84](../decisions.md)).** The clip-selection seam, the procedural
> playback stand-in, and the rig authoring are in; the *runtime skeletal player* is the owed
> follow-up. What landed: `render::anim::select_clip` (the pure `AnimState → AnimClip ∈
> {Idle,Walk,Fire,Death}` seam, priority `Death▶Fire▶Walk▶Idle`), driven from the render snapshot's
> existing `vel`/`firing` (no new sim authority); a subtle **procedural** per-instance pose
> (`anim_pose`/`pose_matrix` — bob / lean / recoil / topple, infantry-gated, `REST`-identical to
> `mesh::model_matrix`) wired into **both** the command and embodied token draw paths so troopers
> visibly animate now; and `tools/models/gen_trooper_rig.py` (`pnpm assets:rig`), a rigid-part
> Blender rig baking the four clips to `assets/models/rigs/trooper_rig.glb` with real glTF animation
> channels + a provenance manifest (script-not-binary, deterministic). All presentation-only
> (invariant #1/#4), no new render dep, `default`/`stress` checksum streams bit-identical.

Coherent locomotion / fire / death animation on the greybox so the eye-level view reads as a *place*.

- ✅ Rig + a small clip set (idle / walk / fire / death) on the trooper greybox via Blender, exported
  with glTF animation channels; script-not-binary ([D84](../decisions.md)). *(Cook through gltfpack →
  LOD chain ([D49](../decisions.md)) rides with the eventual skeletal-loader follow-up — the rig glb
  is authored + committed but not runtime-consumed yet.)*
- ✅ Drive clip selection from sim-derived state (moving vs idle vs firing vs dead) on the render side
  — a presentation read of existing component state, no new sim authority (`render::anim::select_clip`).
- **Explicitly bounded:** this is the *floor*, not photoreal fidelity (consciously conceded per
  [`positioning.md`](../positioning/positioning.md)). Stop at "not jarring."
- **Files:** `tools/models/gen_trooper_rig.py`, `assets/models/rigs/`, `render::anim` (clip seam +
  procedural pose), `render/src/lib.rs` (token draw wiring). *(cook/LOD path + a runtime skeletal
  player = the owed follow-up.)*
- **Acceptance:** ✅ the embodied + command viz scenes show units that animate coherently (procedural
  pose, GPU-verified via `pnpm desktop:viz`); ✅ manifest entries for the new clips; ✅ a unit test on
  the clip-selection seam. **Owed:** runtime skeletal playback consuming the authored `.glb`, and a
  driven death topple (dead units are dropped from the snapshot today — see [D84](../decisions.md)).

### WS-C — Command-layer readability & glanceability *(CP-9, launch-important)*

The RTS half must parse **at a glance on a small screen** and **teach itself fast**. This is
information architecture, paired with — and broader than — the icon/type/colour layer WS-0 delivered.

- A glanceability pass on selection / orders / economy / territory: what can the player read and act
  on in *one second*? Tighten the selection-contextual command panel ([D62](../decisions.md)),
  economy readout, and territory/control-point legibility.
- Consistent iconography + state language across the command bar, build/train/upgrade menus
  ([D48](../decisions.md)), and the radial.
- Bounded by [invariant #3](../../CLAUDE.md) (depth lives in the order/stance vocabulary, never smarter
  unit AI) and [invariant #6](../../CLAUDE.md) (no strategic intel leaks while embodied).
- **Files:** `render` (panel/readout/radial layout), `engine` (any pure layout/threshold seams).
- **Acceptance:** viz PNGs of the command view at phone aspect; a structured readability check (and
  ideally a fresh-eyes playtest — pairs with PC-5's mastery-legibility goal).

### WS-D — Accessibility cues *(load-bearing, invariant #6 — not optional polish)*

The going-dark alert is a directional **flash + audio**; a colorblind or hard-of-hearing player needs
an equivalent or the core mechanic is unfair to them.

- A colorblind-safe palette option (the `render::theme` ramp is already the single source of truth —
  add alternate ramps behind a setting) and a non-colour redundancy (shape/icon) for faction/state.
- A visual equivalent for the audio alert (a directional on-screen indicator) and an audio/haptic
  equivalent for the directional flash — each still fairness-bounded (an *alert*, not *intel*).
- Owned by the Settings surface ([D75](../decisions.md) is partial — accessibility + the rebind editor
  are still owed).
- **Files:** `render::theme` (alternate ramps), the alert cue passes, the Settings shell.
- **Acceptance:** the alert reads under each cue mode in a viz scene; the setting persists.

### WS-E — World & embodied visual depth *(finish the static look)*

The remaining environment polish the foundation wave set up but didn't exhaust.

- Fog / tonemap tuning for the embodied "world goes dark" moment (the visceral, fair blindness of
  invariant #6 — presentation only).
- Normal/detail maps on meshes + ground (ImageMagick-scripted) and richer greybox where silhouettes
  read thin; respect the 200-unit / mobile budget (gltfpack, [D49](../decisions.md)).
- Title / Settings / About theming completeness on the now-shared palette (close out the [D75](../decisions.md) partial).
- **Files:** `tools/` (texture/mesh generators + manifests), `render`, `app` shell theming.
- **Acceptance:** viz + title PNGs; manifest entries for every new generated texture/mesh.

### WS-F — Mesh fidelity pass *(model-quality lift across the roster)*

The trooper reskin (`d7cced1`) proved the method: box-stacking hit a hard ceiling on the *human* (a
golf-ball icosphere head on a slab torso, no readable arms), and the fix was a **technique change** —
a vertex skeleton through Blender's Skin modifier (`skinned_body`) + a proper local-space helmet cut
(`dome`) — driven by a disciplined **render → look → fix** loop, not another chamfer nudge. This
workstream carries that lift to the rest of the roster.

**The lesson is not "skin everything."** Skinning is for *organic* forms, and we have none left. The
remaining models are mechanical / architectural, where box-stacking is the *correct* base — the lever
there is different: **boolean cuts** for real sloped/inset detail, **tuned bevels** (kill the "melty"
over-rounding), denser local detail only on hero assets, and the same tight visual loop.

**Honest scope-setting:** unlike the trooper, none of these are broken — they cleared the WS-0
art-direction pass and read as acceptable greybox today. This is *polish*, ranked by how close the
player gets to each asset, not a rescue.

Priority tiers (closest-to-camera first):

| Tier | Models | Why it ranks here | Technique lever |
|---|---|---|---|
| 1 — hero | `weapon_rifle{,_us,_fr}` | eye-level FPS viewmodel — fills the screen embodied (§4's own "honest weak axis") | most detail budget; crisp small parts; booleans for the rail / mag well |
| 2 — embodied | `tank{,_us,_fr}` hull + `tank_turret{,_us,_fr}` | embodied tank (P7), and the hull is the weakest current model (lumpy road gear, melty slopes) | booleans for glacis + sponsons; distinct road-wheel read; tighter bevel |
| 3 — command dressing | `camp_hq`, `turret`, `barricade` | command-view + embodied backdrop | fix the melty base; crisper architectural edges |
| 4 — scenery | `tree`, `rock`, `crate`, `tracer` | ambient, rarely seen close | light touch; already fine (maybe a more organic tree canopy) |

Process — **one model at a time** (the visual-judgment loop is the crux and doesn't delegate cleanly;
`gen_models.py` is one file, so this is **sequential internally**, not a parallel-worktree fan-out):

- Render the committed `.glb` from 3–4 angles headless (Blender), eyeball it, list the specific reads
  that fail, fix in `gen_models.py`, re-render — repeat until it holds. The `viz-runner` token view is
  too far-zoomed to judge a single model; render the `.glb` directly.
- Hold the budget: LOD0 stays ≈ its current tri count (≤ ~1.5k), the LOD chain stays monotone, and the
  cooked `.mesh` regenerates **bit-identical** (deterministic) — the golden mesh tests + `pnpm
  desktop:viz` stay green.
- **Commit discipline (models-lane):** a full `pnpm assets:models` regen emits incidental glTF
  byte-noise on *unrelated* glbs; revert those and patch the manifest so only the owned models ship
  (exactly as the trooper commit did). **One category = one commit.**
- **Files:** `tools/models/gen_models.py`, `assets/models/**`, `assets/models/manifest.json`.
- **Acceptance:** per category — a before/after render sheet, golden mesh tests green dev+release,
  `pnpm desktop:viz` green, commit scoped to owned models. This is the *mesh* half of the **CP-3**
  "reads as a place" floor — distinct from WS-B, which is CP-3's *animation* half.

---

## Sequencing & dependencies

```
WS-0 (foundation) ── DONE ──► WS-A (game-feel, CP-2)  ── launch-critical, first
                          ├──► WS-C (readability, CP-9) ── launch-important, parallel-safe
                          ├──► WS-D (accessibility)      ── invariant #6, parallel-safe
                          ├──► WS-E (world depth)        ── parallel-safe, low-risk
                          ├──► WS-F (mesh fidelity)      ── parallel-safe across WS, sequential within
                          └──► WS-B (anim floor, CP-3)   ── conceded tier, ramps last
                                                             (WS-B rigs on WS-F's meshes)
```

WS-A first — it's the launch-critical row and the one a shooter player judges instantly. WS-C / WS-D /
WS-E / WS-F are independent presentation work that can land in any order (WS-D is the one with a
*fairness* mandate, so don't let it slip). WS-F (mesh fidelity) touches only `gen_models.py`, so it is
parallel-safe against the render/app-side workstreams but must run sequentially *within itself*;
ideally it precedes WS-B, which rigs animation on the meshes it lands. WS-B (animation) is the conceded
tier and ramps last — "not jarring" is the bar, not parity. CP-6 (audio identity) runs as its own
scripted-asset workstream and hands WS-A its fire/impact sounds.

## Risks & notes

- **Presentation-only, always.** No `core`/sim type changes, no new sim reads that could become
  authority, no checksum surface (invariant #1/#4). Host-side view-kick/ADS/feedback state is render
  glue, exactly like the existing `fire`/`locomote` quantization seams; extract pure logic to a
  testable fn rather than burying it in winit/wgpu glue.
- **Fairness outranks feel (invariant #6).** No new VFX/HUD cue may leak strategic intel while
  embodied; feedback is allowed only on the player's *own* action (the TF-4 precedent). WS-D is a
  hard requirement, not polish.
- **GPU-gated verification.** The viz suite needs a real adapter, so it stays a local smoke test like
  today's `viz-runner`, never the no-GPU CI matrix. The determinism matrix stays the load-bearing CI
  gate and is untouched by this plan.
- **Script-not-binary for every asset.** Generator + manifest entry per new model/texture/icon/clip;
  no committed blobs ([D41](../decisions.md)/[D46](../decisions.md)).
- **Game-feel has a human-judged half.** CP-2/CP-3 acceptance includes a playtest against a written
  floor; that sign-off is carried honestly (like the D31 by-hand-feel caveat), not asserted by pixels
  alone.
