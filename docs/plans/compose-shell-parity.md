# Compose shell parity plan — bringing Android's out-of-match shell up to desktop

> **Status: SUBSTANTIALLY COMPLETE (as of 2026-06-30).** All three tiers landed — the launch-config
> seam (Tier 0), Settings/Profile/About + title mode-split (Tier 1), and the gunsmith + campaign
> mission-select/briefing (Tier 2) — and a **parity-gap sweep** ([§12](#12-parity-gap-sweep-2026-06-30))
> then closed the six concrete UI/content divergences a four-cluster audit turned up. The Android
> Compose shell is now at **feature + value parity** with the desktop egui shell
> ([D36](../decisions.md)) across every shipped out-of-match surface. The Android campaign
> progress/unlock model has since landed and closed end-to-end (2026-07-03: every playable node —
> root or gated — launches through the wire; §12 item 1). A 2026-07-03 re-audit closed the other
> two structural items too: **desktop shell-pref persistence shipped** (`92f5fc3` → the
> `app/src/shell/persist.rs` codec; §12 item 2), and the **briefing-difficulty + look-sensitivity
> wires are consumed end-to-end** on Android (`bec478e`/`ae32cbd`; §12 item 3, §5). The
> [D85](../decisions.md) Stock/Muzzle gap (§12 item 5) closed 2026-07-03 on both halves — the
> desktop persist encode and the full Android chain (Compose slots + `stk=`/`muz=` wire keys +
> prefs keys). The 2026-07-03 desktop landings all closed with same-day Compose twins: the
> [D101](../decisions.md) PvP staging door (§12 item 7), the D98 conflict-atlas hub grouping
> (§12 item 8), and the full skirmish match-setup (§12 item 6 — battlefield / both armies /
> opponent tier over the new `earmy`/`skirm` wire keys). **Every structural parity item is now
> closed**, and the shared map-library seam landed same-commit on both shells too
> ([D102](../decisions.md): the `BATTLEFIELDS` table + the `map=` wire key — content work,
> never a parity gap). What remains is **blocked** on later phases (PvP queues/lobby/store/
> consent per [`phase-4-plan.md`](phase-4-plan.md) §2). Scope is
> **Android Compose only**; iOS has no native target at all (Phase 3). Sections 1–2 below are the
> original gap analysis, kept for the *why*; the per-tier status notes record what landed.

---

## 1. Why Android is behind (it's structural, not a regression)

Per [D32](../decisions.md), out-of-match chrome is **native per-platform** — Android's surfaces
are Kotlin/Jetpack-Compose and **cannot** be shared from the desktop egui shell
(`app/src/shell.rs`). "Parity" therefore means *re-authoring* each desktop surface in Compose, not
wiring up shared code. The only thing both platforms share is the engine (`engine::Game`) and the
GPU-free, logic-free [`core::shell`](../../core/src/shell.rs) seam ([D34](../decisions.md)).

Both shells landed together — Android `d148cb0` (D35), desktop `bf2acf0` (D36) — but only the
desktop side received follow-on work (`a528e2c` theme, `15c95d8` 3D-backdrop title, `d52a07b`
Settings/Profile/About, `3766778` campaign wiring). There are **no `feat(android)` commits touching
`TitleScreen.kt`/`MainActivity.kt` after `d148cb0`**. The Compose title is frozen at D35; the egui
shell is ~4 waves ahead. That divergence is the entire gap.

There is also a structural difference desktop doesn't have. On desktop the shell and engine are
**one process, one `App`**: Deploy just calls `Game::new_scene_with_loadout(...)` in-process
(`app/src/main.rs:492`), and live settings are pushed into the running game every frame
(`set_look_prefs`/`set_gains`, `app/src/main.rs:367-373`). On Android the Compose shell
(`MainActivity`) and the engine (`NativeActivity` → `android_main`) are **separate activities**, and
today the hand-off carries **nothing**: `MainActivity.kt:41` does a bare
`startActivity(NativeActivity)`, and `pal-android/src/android_backend.rs:154` calls
`Game::new(device, format, DEFAULT_SEED)` with no config.

**That missing config hand-off is the linchpin.** Most of the gap is not "draw more Compose" — it
is "there is no way to tell the engine what to launch." Build that seam first and three surfaces
unblock at once.

---

## 2. The concrete delta (desktop → Android)

| Capability | Desktop | Android today |
|---|---|---|
| Live 3D animated title backdrop | `shell.rs:802-809` (`render::title_backdrop`) | flat `MaterialTheme.background` (`TitleScreen.kt:45`) |
| Top-level play modes | CAMPAIGN / SKIRMISH / PvP (`TitleAction`, `shell.rs:27`) | one generic START (`TitleScreen.kt:78`) |
| Settings (audio/look) | real, wired ([D75](../decisions.md), `shell.rs:247`) | **no-op stub** (`MainActivity.kt:32`) |
| Profile | callsign/faction/record (`shell.rs:375`) | absent |
| About / field-manual | `draw_about` (`shell.rs:850`) | absent |
| Gunsmith / loadout | `engine::loadout_ui`, read at Deploy (`main.rs:484`) | absent — boots `DEFAULT_SEED` default match |
| Campaign mission-select + briefing | `Screen::MissionSelect`/`Briefing` (`main.rs`) | absent |
| Title-screen hub (identity card, deploy rail, NEXT OPERATION shortcut) | corner-anchored hub — DEPLOY rail bottom-left, identity card top-right, NEXT OPERATION/CONTINUE bottom-right (`app/src/shell/egui_shell.rs`, [D97](../decisions.md)) | centred single-column title — no identity/progress surfaced (`TitleScreen.kt`) |

Input handling for what Android *does* show is fine (the three buttons work). The gap is missing
surfaces, not broken ones. The title-hub row is **not** a Tier-N todo like the rows above it — D97
explicitly scopes it as a possible future Tier-2 follow-up, not a blocker, since routing semantics
and validation rules didn't fork.

---

## 3. Tier 0 — the launch-config seam (foundation, build first) — ✅ LANDED

> **Status: LANDED.** The seam ships: `pal-android/src/launch.rs` (pure, host-tested codec),
> `LaunchConfig.kt` (mirror codec), the JNI `Intent`-extra reader in `android_backend.rs`, and
> `MainActivity` now boots the real **Skirmish** match (desktop's default) via the extra. Wire
> format v1: `v=1;scene=skirmish;opt=0;bar=0;mag=0;vol=80;sfx=80;sens=100;invy=0` (tolerant decode).
> Verified: Rust host tests (dev+release), `cargo ndk` arm64 cdylib compiles, Kotlin
> `compileDebugKotlin` + `testDebugUnitTest` green. On-device boot-into-Skirmish is the one check
> owed when a device is available (the JNI reader is un-unit-testable glue, mirroring `thermal`'s
> sensor and `finish_activity`). The wire carries loadout/audio/look keys too, tolerant-decoded to
> defaults until the gunsmith/Settings surfaces populate them.


A typed launch config that crosses **Compose → `NativeActivity` → `android_main`**, replacing the
bare `Game::new(…DEFAULT_SEED)`.

```
 ┌─────────────┐  Intent extras (LaunchConfig)  ┌───────────────┐  parse  ┌──────────────────────┐
 │ Compose     │ ─────────────────────────────► │ NativeActivity│ ──────► │ android_main         │
 │ MainActivity│   scene/mission id, loadout,    │ (intent)      │         │ Game::new_scene_with │
 │             │   difficulty, audio/look prefs  │               │         │ _loadout(...)        │
 └─────────────┘                                 └───────────────┘         └──────────────────────┘
```

- **Kotlin side:** a `LaunchConfig` data class serialized into `Intent` extras at `startMatch()`.
- **Rust side:** `android_main` reads the extras off the activity's intent (JNI via the
  `android-activity` `AndroidApp`) and calls `Game::new_scene_with_loadout(...)` — the *exact* call
  desktop already uses (`main.rs:492`) — instead of `Game::new(...)`. The engine entry path then
  matches desktop.
- **Why Intent extras, not a Storage-PAL file:** the config is produced out-of-match (Compose) and
  consumed once at match start — a one-shot launch payload, not live shared state. Intent extras are
  the right tool; no Storage round-trip needed.
- **Not netcode-blocked.** This is plumbing across the Activity boundary; it has no Phase-3
  dependency. Highest leverage piece in the plan.

**Test seam (CLAUDE.md floor):** a pure Kotlin `LaunchConfig` encode/decode round-trip gets a JVM
test (the `BuildStampTest.kt` pattern); the Rust extra-parse gets a host-testable parse fn + unit
test — extracted off the JNI glue exactly as `pal-android/thermal.rs` split the pure mapping from
the JNI reader.

---

## 4. Tier 1 — buildable now (Settings/Profile/About need *nothing* from Tier 0) — ✅ LANDED

> **Status: LANDED.** All four surfaces ship as stateless Compose screens + pure JVM-tested seams,
> wired through a `MainActivity` `ShellRoute` navigator (the Compose twin of the desktop `Screen`
> enum). Settings (audio/look subset, integer-wire-aligned), Profile (callsign/faction/record),
> About/field-manual, and the title **mode-split** (CAMPAIGN/SKIRMISH/PvP + SETTINGS/PROFILE/FIELD-MANUAL)
> over a D78 animated Compose backdrop. Accessibility cues + touch-rebind editor remain out of scope
> (phase-4-plan §5). Verified: `:app:compileDebugKotlin` + `:app:testDebugUnitTest` green (63 tests).


| Surface | Desktop reference | Compose work | Scope notes |
|---|---|---|---|
| **Settings** (audio + look subset) | `SettingsState` `shell.rs:247`; applied `main.rs:367-373` | sliders (master/SFX/music, sensitivity), invert-Y, quality; persist via **DataStore**; fold values into the Tier-0 `LaunchConfig` | [D75](../decisions.md) shipped this subset on desktop, so it's explicitly buildable. **Accessibility cues + touch-layout/rebind editor stay BLOCKED** (phase-4-plan §2/§5) — ship audio/look, flag the rest. |
| **Profile** | `ProfileState` `shell.rs:375`; `sanitize_callsign`/`win_rate_pct` | callsign field, faction picker, lifetime record; DataStore persist | fully buildable |
| **About / field-manual** | `draw_about` `shell.rs:850`, `ControlRow` `shell.rs:470` | static content screen reached from Settings | lowest-risk surface — good first slice to prove the nav graph + test-seam pattern |
| **Title mode-split** | `TitleAction`/`resolve_title_action` `shell.rs:27-94` | CAMPAIGN / SKIRMISH / PvP buttons + a Compose nav graph | buttons are trivial; CAMPAIGN/SKIRMISH route to Tier 2; **PvP → a "blocked" notice** (match-setup is Q5/Phase-3) |

---

## 5. Tier 2 — buildable once Tier 0 lands (config-seam-blocked, NOT netcode-blocked)

> **Gunsmith + campaign mission-select/briefing: ✅ LANDED.** The Compose gunsmith (`LoadoutSelection`
> seam, labels verbatim from `core::gunsmith`) and the Operations-hub **mission-select + briefing**
> (the single "Seize the Outpost" node → `mission1`, with a difficulty cycler) ship. Campaign opens
> mission-select → briefing → gunsmith → Deploy into `mission1` with the chosen loadout; SKIRMISH/PvP open
> the gunsmith and Deploy into Skirmish. (Since [D81](../decisions.md) the gunsmith is
> customization-only behind Settings on both shells; briefing/mode-select Deploy boots directly with
> the *persisted* loadout — `MainActivity.kt:186-187`, `:218-225`.) The engine now **fully consumes**
> the wire loadout (`new_scene_with_loadout`) and audio gains. **Owed — both since closed
> (2026-07-03 re-audit; wire traces in §12 item 3):** the briefing's **difficulty** (the `diff` wire
> key ships and drives the fight via `Difficulty::from_tier` → the shared `apply_campaign_tuning`,
> [D83](../decisions.md)) and **look-sensitivity** (the "not scalable at the PAL boundary" objection
> was solved with a `Game` setter instead: the launch `sens`/`invy` values reach
> `engine::touch_controls::set_look_prefs` every frame) — at the time of writing both were
> shown/carried but not applied; **both now are**. **Persistence:** Settings/Profile/loadout now
> survive restarts via `ShellPrefs` (SharedPreferences) — since extended to the army pick + the
> campaign cleared set (`ShellPrefsCodec.KEY_ARMY`/`KEY_CAMPAIGN`), and desktop has since landed its
> own twin (§12 item 2). **Update:** the shipped campaign is now a **12-node graph** — four
> conflicts, each a self-contained *Seize* → *Hold* → *Push* chain ([D105](../decisions.md)) —
> on both the shared model (`engine::default_campaign()`) and the Android
> `CampaignModel` mirror, with the node→scene launch mapping (`Scene::for_mission`) wired through
> the backend, and the Compose mission-select tiles now render **and launch every playable node**
> (§12 item 1, closed 2026-07-03 via the pure `missionLaunchConfig` seam). Still pending: PvP
> match-setup (Q5/Phase-3).


| Surface | Desktop reference | What Tier 0 unblocks |
|---|---|---|
| **Gunsmith / loadout** | `engine::loadout_ui::LoadoutEditor`; `draw_loadout` `main.rs:287`, read at Deploy `main.rs:484` | a Compose gunsmith edits the loadout; Deploy packs it into `LaunchConfig`; engine already accepts it |
| **Campaign: mission-select + briefing** | `Screen::MissionSelect`/`Briefing(NodeId)`; `draw_mission_select`/`draw_briefing` | Compose mission-select + briefing (difficulty cycler); launch packs `NodeId` + tier into `LaunchConfig`; campaign system already lives in `engine` |

These are gated **only** on the Activity-boundary config seam — split out from the genuinely-blocked
items so they aren't mistaken for Phase-3 work.

---

## 6. Explicitly BLOCKED — do not attempt under this plan

So these aren't confused with "just unbuilt":

- **PvP match setup** (army/map/mode) — Q5 / Phase-3 netcode.
- **Lobby & matchmaking** — Phase-3 netcode.
- **Settings: accessibility cues + touch-layout/rebind editor** — phase-4-plan §5; the going-dark
  fairness cues (invariant #6) must ship *with* the editor, not as an afterthought.
- **Store / IAP** — Q9 (billing) + Q11 (catalog).
- **Consent & legal UI** — the gate ships in `server`; the screen is blocked native chrome.

---

## 7. The 3D title backdrop — the awkward one (→ D78)

Desktop's title paints a live animated `render::title_backdrop::TitleBackdrop` (a **wgpu** scene,
`shell.rs:802-809`) with cursor→NDC parallax, then composites egui over it. Compose has **no wgpu
surface** — the Android title is a flat `MaterialTheme.background` (`TitleScreen.kt:45`). Three
options:

1. **Richer flat/animated Compose backdrop** (gradient, drifting vector motif, Compose animation) —
   cheap, ~80% of the perceived polish, no engine surface. **Recommended.**
2. **Embed a wgpu `SurfaceView`** behind Compose to run the real `TitleBackdrop` — high cost (a
   second render surface in the shell process, lifecycle/threading), and it partly re-litigates the
   D32 native-chrome split.
3. **Accept the backdrop as desktop-only chrome** and don't chase pixel parity.

Locked as **option 1** in [D78](../decisions.md).

---

## 8. Pure-seam duplication — Kotlin vs single-source (→ D79)

Compose UI is test-exempt, but every pure decision/validation fn gets extracted to a plain-Kotlin
seam with a JVM test — the `BuildStamp.kt` pattern. That means re-implementing the desktop seams
(`resolve_title_action`, `sanitize_callsign`, `win_rate_pct`, settings `clamp`, the bounds
`SENS_MIN/MAX`, `CALLSIGN_MAX`) in Kotlin. D32 sanctions chrome forking, but **numeric bounds and
validation rules drifting between platforms would be a real consistency/fairness bug**, not just a
style nit. Two paths:

- **Re-implement in Kotlin with JVM tests + a synced-constants discipline** — light, idiomatic, no
  JNI on the hot UI path. **Recommended.**
- **Single-source the bounds/validation in `core::shell` and call over JNI** — invariant-#2-pure but
  heavy for trivial presentation helpers, and drags JNI into out-of-match chrome.

Locked as the light path in [D79](../decisions.md), with the bounds mirrored from `core` and a JVM
test asserting them so drift is caught.

---

## 9. Test discipline (carry every surface)

- Compose `@Composable` UI is exempt (un-unit-testable glue, like winit/android event glue in the
  engine) — but its **pure logic is not**. Each surface lands its decision/validation seam as plain
  Kotlin with a `src/test` JVM test, mirroring `BuildStamp.kt`/`BuildStampTest.kt`.
- The Rust `LaunchConfig` parse (Tier 0) lands a host-side parse fn + unit test, off the JNI glue.
- No determinism/lockstep surface is touched (this is chrome + one launch payload), so the
  cross-platform checksum matrix is unaffected — call that out in each commit so it isn't assumed.

---

## 10. Suggested sequencing (each a scoped commit)

1. **Tier 0** — `LaunchConfig` seam (Kotlin encode + Rust parse, both tested); engine entry switches
   to `Game::new_scene_with_loadout`. *Highest leverage.*
2. **About / field-manual** — lowest-risk Compose surface; proves the nav graph + test-seam pattern.
3. **Profile** — DataStore persistence + `sanitize_callsign`/`win_rate_pct` Kotlin seams + JVM tests.
4. **Settings** (audio/look subset) — sliders, DataStore, fold into `LaunchConfig`; flag
   accessibility/rebind out-of-scope.
5. **Title mode-split + backdrop** (D78 option 1) — CAMPAIGN/SKIRMISH/PvP buttons; PvP → blocked notice.
6. **Gunsmith** (Tier 2) — Compose loadout editor → `LaunchConfig`.
7. **Campaign mission-select + briefing** (Tier 2).

---

## 11. Decisions this plan needs (record via `/decision`)

- **[D78](../decisions.md) — Android title backdrop** ✅ RECORDED — Compose-native animated backdrop
  (option 1), not an embedded wgpu surface.
- **[D79](../decisions.md) — pure-seam duplication** ✅ RECORDED — re-implement the chrome
  decision/validation seams in Kotlin with JVM tests + mirrored-from-`core` bounds, rather than a
  JNI single-source.

See [`phase-4-plan.md`](phase-4-plan.md) §2 (surface table), [D32](../decisions.md) (native-shell
split), [D34](../decisions.md) (the `core::shell` seam), [D35](../decisions.md)/[D36](../decisions.md)
(the two Boot & title shells).

---

## 12. Parity-gap sweep (2026-06-30)

Once all three tiers had landed, a four-worker audit (one per surface cluster — title+nav+persistence,
settings+profile+about, loadout+gunsmith, campaign+mission+briefing) compared the Compose shell
against the canonical desktop reference (`app/src/shell.rs` + the shared `engine`/`core` seams).
**Value-level parity was already solid** — setting ranges/defaults/clamps, the keymap rows, callsign
sanitisation + win-rate math, and the four difficulty tiers all matched (most already test-pinned per
[D79](../decisions.md)). The audit found **six closeable UI/content gaps**, all fixed in one
path-scoped Android commit:

| Gap | Desktop reference | Fix |
|---|---|---|
| **Briefing copy drift** (worst — a live cross-shell content divergence, and unguarded by tests) | `core::mission_tuning::MISSION_ONE_BRIEFING.situation` | Android now mirrors the situation string **verbatim** (was a paraphrase that also folded in `objective_line`, which the desktop briefing surface doesn't show); pinned by `CampaignModelTest` so it can't silently drift again |
| **Gunsmith RESET missing** | `LoadoutAction::Reset` / `LoadoutEditor::reset()` | added the RESET button + `LoadoutSelection.reset()`/`STANDARD` seam (DEPLOY · RESET · BACK) |
| **Profile RESET RECORD missing** | `ProfileAction::ResetStats` | added the button + `ProfileState.resetRecord()` (zeroes matches/wins, keeps callsign + faction) |
| **About build-stamp missing** | `about_ui` renders the stamp on the card | About screen now takes a `versionStamp` and renders it above BACK |
| **Mission-select subtitle missing** | `mission_select_ui`'s instructional line | added verbatim under the OPERATIONS banner |
| **Trade-hint glyph drift** | `slot_trade_hint` uses ASCII `<->` (deliberate, font-safe) | Android changed `↔` → `<->` to match the desktop literal byte-for-byte |

New/extended JVM tests cover `reset()`/`resetRecord()` and pin the verbatim briefing + trade-hint
strings; `gradlew testDebugUnitTest` green. No determinism/lockstep surface touched (chrome only), so
the checksum matrix is unaffected.

### Structural parity items still open (need a design call — *not* a mirror tweak)

These were deliberately **not** done in the sweep — each a chunk of real work. A **2026-07-03
re-audit** then verified items 1–3 closed in code (evidence inline below), item 4 remains a
deliberate UX fork, item 5 is a gap the re-audit found (closed the same day, both halves), and
items 6–8 are the 2026-07-03 desktop landings (the skirmish match-setup screen, the D101 PvP
staging door, the D98 atlas-grouped hub) — all three closed with same-day Compose twins:

1. **Campaign progress model — ✅ CLOSED (2026-07-03).** `CampaignModel.kt` carries the full
   `CampaignProgress`/`NodeProgress` (Locked/Available/Cleared) derivation, the clear gate,
   best-tier tracking, and the persistence codec — the JVM-testable twin of desktop's `Campaign`.
   **The shipped campaign is a 12-node graph** — four conflicts, each a self-contained *Seize* →
   *Hold* → *Push* chain ([D105](../decisions.md)) — via `engine::default_campaign()`,
   and `campaignNodes` mirrors it in lock-step (each conflict's Hold `prerequisites` the Seize
   before it, gating within a war only); the node→scene
   launch mapping (`Scene::for_mission`) is wired on both hosts, and the
   `CampaignModelTest`/`CampaignProgressTest` pin the chain structure + the Hold briefing verbatim.
   The last open piece — the Compose chrome launching only the root node — is now closed too: the
   mission-select tiles render **every** node (locked tiles disabled from the derived
   `NodeProgress`), and the briefing Deploy resolves the *selected* node — root or gated — through
   the pure launch-resolution seam (`MissionLaunch.kt`: `missionLaunchConfig` threads the node's
   `sceneToken` + `NodeId` ordinal + replay tier onto the `scene`/`node`/`diff` wire keys), matching
   the desktop host's `pending_launch` scene/node pair. `MissionLaunchTest` pins the routing on the
   JVM — per-node scene/index resolution, the `mission2`/`node=1` wire round trip, and the full
   launch → win-code → record-on-win → persistence loop for the gated Hold node — so a future node
   can't silently regress to the root.
2. **Desktop doesn't persist shell prefs — ✅ CLOSED (verified 2026-07-03; landed `92f5fc3`
   2026-06-30, the same day this sweep was written).** The full round trip ships: the tolerant
   `key=value` codec `encode_shell_prefs`/`decode_shell_prefs` (`app/src/shell/persist.rs:45`/`:96`
   — pure, round-trip-tested in `app/src/shell/tests.rs`; split out of `shell.rs` by `8df9610`),
   loaded once at `App` init (`app/src/main.rs:195` → `load_shell_prefs`, `:1147`) and saved on
   leaving any screen that edits the state — Settings / Profile / Loadout / ArmySelect
   (`app/src/main.rs:685-692` → `persist_shell_prefs`, `:1169-1181`, best-effort like
   `campaign.dat`). Coverage **meets or exceeds** Android's `ShellPrefs`: settings (audio, sens,
   invert-Y, plus desktop-only fov / CVD palette / alert-cue mode / keybind map), profile
   (callsign/faction/record), loadout, and the army pick (`1ce01cb`); campaign progress still rides
   `campaign.dat` separately. **The one residual hole is now closed too (2026-07-03):** the encode
   blob used to write only optic/barrel/magazine while decode also read the D85 stock/muzzle keys
   ([D85](../decisions.md) (2026-07-01) postdated the codec), so a customized Stock/Muzzle silently
   reset on a desktop restart. `encode_shell_prefs` now writes `stock=`/`muzzle=`
   (`persist.rs`, `stock_index`/`muzzle_index`), and the round-trip test's sample loadout sets
   **every** slot non-default so the encoder can't drop one again; item 5's Android half remains.
3. **Look-sensitivity / briefing-difficulty are carried but inert on Android — ✅ CLOSED (verified
   2026-07-03; landed `ae32cbd` 2026-06-30 / `bec478e` 2026-07-01).** Both wires now run
   end-to-end:
   - **Difficulty:** the briefing cycler threads the tier into the launch
     (`MainActivity.kt:225` → `missionLaunchConfig`, `MissionLaunch.kt:72`: `diff =
     difficulty.tier()`) → the `diff=` wire key (`LaunchConfig.kt:80`; parsed + clamped
     `pal-android/src/launch.rs:180`) → the android glue maps it back
     (`Difficulty::from_tier(launch.diff)`) and applies its combat tuning through the **shared**
     `Game::apply_campaign_tuning` (`pal-android/src/android_backend.rs:584-585` —
     [D83](../decisions.md), resolving Q21: the tier both drives the fight and is what the
     win-result code records the clear at, `launch.rs:292`).
   - **Look-sensitivity:** Settings `sensX100`/`invertY` thread every launch
     (`MissionLaunch.kt:42-43`) → the `sens=`/`invy=` keys (`LaunchConfig.kt:78-79`; parsed
     `launch.rs:178-179`) → seeded at boot (`android_backend.rs:152-155`,
     `input.set_look_prefs(sens_x100_to_f32(…), invert_y)`) → pushed into the engine each frame
     (`android_backend.rs:330`, `game.set_touch_look_prefs(…)` → `engine/src/lib.rs:2631` →
     `engine::touch_controls::set_look_prefs`, `touch_controls.rs:396`, unit-tested
     `:1019-1059`). The original objection — the look delta is derived inside
     `engine::touch_controls`, not scalable at the PAL boundary — was answered with a `Game`
     setter rather than `InputFrame` scaling.
4. **Inverted About entry point** — desktop reaches About from inside Settings (`SettingsAction::About`);
   Android surfaces it as a "FIELD MANUAL" button on the title. A deliberate [D78](../decisions.md) UX
   choice; left as-is, noted so it isn't mistaken for a regression. (Re-verified 2026-07-03:
   `app/src/shell/settings.rs:208`, `TitleScreen.kt:142`.)
5. **D85 gunsmith breadth is desktop-only — ✅ CLOSED (2026-07-03, both halves; found OPEN by the
   same day's re-audit).** [D85](../decisions.md) (2026-07-01) made **Stock + Muzzle** real sim
   sidegrade slots, and the desktop gunsmith cycled all five (`engine/src/loadout_ui.rs:160-171`)
   while Android carried only optic/barrel/magazine — a real value-level divergence in
   **sim-affecting** loadout slots, not chrome. Now closed end-to-end:
   - **Compose gunsmith**: `Slot.Stock`/`Slot.Muzzle` with labels + trade hints mirrored verbatim
     from `core/src/gunsmith.rs` (`LoadoutSelection.kt`; the D79 tables/tests extended — labels,
     hints, cycle/reset/clamp all pinned in `LoadoutSelectionTest`). `GunsmithScreen` iterates
     `Slot.entries`, so the two rows render with no chrome change.
   - **Launch wire**: new `stk=`/`muz=` keys on both ends (`LaunchConfig.kt` ↔
     `pal-android/src/launch.rs`), tolerant-decoded — a pre-D85 emitter defaults both to Standard
     (back-compat pinned on both sides); `build_match_game` now fields all five slots into the
     `Loadout` (`android_backend.rs`), and `missionLaunchConfig`/`launchConfigOf` thread them
     (`MissionLaunchTest` pins the fold).
   - **Prefs**: `loadout.stock`/`loadout.muzzle` keys in `ShellPrefsCodec` **and** in
     `ShellPrefs.ALL_KEYS` (the read-loop gate — omitting it would have silently dropped the saved
     value); pre-D85 store decode + garbage-value degradation pinned in `ShellPrefsCodecTest`.
   - Desktop's own persist-encode omission was fixed the same day (see item 2).
   Verified: `gradlew testDebugUnitTest` green (137 JVM tests), `cargo test -p gonedark-pal-android`
   green dev+release, and the aarch64 `cargoNdkBuild` compiles (after the jni-0.22 haptics fix,
   which had broken the android-target build at HEAD).
6. **Skirmish match-setup screen — ✅ CLOSED (2026-07-03, same day).** The desktop SKIRMISH door
   opens the full [`modes.md`](../modes.md) §3 setup surface (`app/src/shell/skirmish.rs`:
   battlefield / both armies / opponent tier, launched through `apply_campaign_tuning` +
   `select_army` for both sides, REMATCH-aware). The Compose twin landed the same day:
   `SkirmishSetupScreen.kt` over the pure `SkirmishSetup.kt` seam (`nextArmy`/`clampBattlefield`/
   `reseedPlayerArmy`/`skirmishLaunchConfig`, pinned by `SkirmishSetupTest` against the desktop
   semantics), with two new wire keys mirrored in `LaunchConfig.kt` **and** `launch.rs` in the
   same commit: `earmy` (enemy army; `0` = keep the scenario default) and `skirm` (the
   configured-skirmish discriminator — the glue applies the tier + enemy pick through the shared
   seams and **never** records a campaign clear, so a skirmish win on Seize Ground stays the
   no-stakes sandbox). The old `ModeSelectScreen.kt` is deleted, mirroring the desktop's retired
   picker. The remaining §3 work named here — the shared **map-library** seam (D34 manifest
   listing) — landed same-commit on both shells ([D102](../decisions.md): one `BATTLEFIELDS`
   table, Kotlin twin `Battlefield.kt`, the `map=` wire key; never a parity gap, closed as
   content work).
7. **PvP staging door — ✅ CLOSED (2026-07-03, same day).** Desktop's PvP button now opens the
   dedicated staging screen ([D101](../decisions.md): the three queues in
   [`modes.md`](../modes.md) §5 build order, nothing joinable pre-net via the pure
   `queue_joinable` seam, the §4a identity line) and the shared "SELECT MODE" picker is
   **deleted** (`app/src/shell/mode_select.rs` is gone; `SHELL_GAME_MODES` now backs only the
   skirmish battlefield list). The Compose twin landed the same day: `PvpScreen.kt` over the
   pure `PvpStaging.kt` seam (`pvpQueues`/`queueJoinable`, pinned by `PvpStagingTest` against
   the Rust table), `resolveTitleAction` split (`Pvp -> TitleRoute.Pvp`; `TitleActionTest` now
   pins that **no two play modes share a door**), and `ModeSelectScreen.kt` retitled SKIRMISH
   as that door's interim picker (item 6 owns the rest). No new wire keys — the staging door
   launches nothing.
8. **Conflict-atlas hub grouping — ✅ CLOSED (2026-07-03, same day).** Desktop's Operations hub
   renders the D98 conflict → operation → battle grouping (`hub_sections` in
   `app/src/shell/mission_select.rs`: conflict headers with year-span + rollup, operation
   sub-headers, grouped tiles). The Compose twin landed the same day: `hubSections` +
   `GroupProgress` + the header-label formatters in `CampaignModel.kt` (JVM-pinned by
   `HubSectionsTest`, incl. label-formatting parity with the desktop output), consumed by the
   grouped `MissionSelectScreen.kt`. Presentation only — no wire/progress-model change (the
   item-1 launch seam already covers every node).
9. **Atlas globe (backdrop → fully navigable → per-battle overview → camera flight) —
   deliberate desktop-only presentation (2026-07-03, [D103](../decisions.md)/
   [D104](../decisions.md)/[D106](../decisions.md)/[D107](../decisions.md)).** Desktop's
   campaign front door is now the **navigable conflict atlas** (`shell::atlas` over
   `render::globe_backdrop`: drag/zoom, a year scrubber, pin-click → the conflict's filtered
   hub); picking a war now lands on a **battlefield overview** instead of a settled backdrop —
   the globe zooms onto that conflict's ground with one progress-toned pin per authored battle,
   and the briefing keeps that same view with the briefed node's pin focused
   ([D106](../decisions.md)). The atlas ↔ battlefield hop is a cancellable **camera flight**,
   not a cut ([D107](../decisions.md)). Android's hub **deliberately keeps the
   grouped list** — the phone-side cost is exactly the D32 strain
   [Q28](../open-questions.md#q28--conflict-atlas) named (no engine surface in the Compose
   shell), plus the fiddly-touch-navigation half D104 left deferred. Like item 4, this is a
   recorded UX fork, **not an owed mirror**: Q28 fork 2 is closed *for desktop* (D104);
   Android's presentation gets its own decision (engine surface in Compose vs. a native 2.5D
   regional map) when the campaign earns it. The mirrored *data* is field-complete either
   way — `CampaignModel.kt`'s `Conflict` carries `latX10`/`lonX10` (D79) and `MissionNode` now
   also carries the optional per-battle `latX10`/`lonX10` anchor (D106) — Android renders
   neither.
