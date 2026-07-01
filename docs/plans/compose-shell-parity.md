# Compose shell parity plan — bringing Android's out-of-match shell up to desktop

> **Status: SUBSTANTIALLY COMPLETE (as of 2026-06-30).** All three tiers landed — the launch-config
> seam (Tier 0), Settings/Profile/About + title mode-split (Tier 1), and the gunsmith + campaign
> mission-select/briefing (Tier 2) — and a **parity-gap sweep** ([§12](#12-parity-gap-sweep-2026-06-30))
> then closed the six concrete UI/content divergences a four-cluster audit turned up. The Android
> Compose shell is now at **feature + value parity** with the desktop egui shell
> ([D36](../decisions.md)) across every shipped out-of-match surface. What remains is **structural**
> (a campaign progress/unlock model on Android; desktop-side shell-pref persistence) and **blocked**
> (PvP/lobby/store/consent per [`phase-4-plan.md`](phase-4-plan.md) §2) — tracked in §12. Scope is
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
| Top-level play modes | CAMPAIGN / PvE / PvP (`TitleAction`, `shell.rs:27`) | one generic START (`TitleScreen.kt:78`) |
| Settings (audio/look) | real, wired ([D75](../decisions.md), `shell.rs:247`) | **no-op stub** (`MainActivity.kt:32`) |
| Profile | callsign/faction/record (`shell.rs:375`) | absent |
| About / field-manual | `draw_about` (`shell.rs:850`) | absent |
| Gunsmith / loadout | `engine::loadout_ui`, read at Deploy (`main.rs:484`) | absent — boots `DEFAULT_SEED` default match |
| Campaign mission-select + briefing | `Screen::MissionSelect`/`Briefing` (`main.rs`) | absent |

Input handling for what Android *does* show is fine (the three buttons work). The gap is missing
surfaces, not broken ones.

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
> About/field-manual, and the title **mode-split** (CAMPAIGN/PvE/PvP + SETTINGS/PROFILE/FIELD-MANUAL)
> over a D78 animated Compose backdrop. Accessibility cues + touch-rebind editor remain out of scope
> (phase-4-plan §5). Verified: `:app:compileDebugKotlin` + `:app:testDebugUnitTest` green (63 tests).


| Surface | Desktop reference | Compose work | Scope notes |
|---|---|---|---|
| **Settings** (audio + look subset) | `SettingsState` `shell.rs:247`; applied `main.rs:367-373` | sliders (master/SFX/music, sensitivity), invert-Y, quality; persist via **DataStore**; fold values into the Tier-0 `LaunchConfig` | [D75](../decisions.md) shipped this subset on desktop, so it's explicitly buildable. **Accessibility cues + touch-layout/rebind editor stay BLOCKED** (phase-4-plan §2/§5) — ship audio/look, flag the rest. |
| **Profile** | `ProfileState` `shell.rs:375`; `sanitize_callsign`/`win_rate_pct` | callsign field, faction picker, lifetime record; DataStore persist | fully buildable |
| **About / field-manual** | `draw_about` `shell.rs:850`, `ControlRow` `shell.rs:470` | static content screen reached from Settings | lowest-risk surface — good first slice to prove the nav graph + test-seam pattern |
| **Title mode-split** | `TitleAction`/`resolve_title_action` `shell.rs:27-94` | CAMPAIGN / PvE / PvP buttons + a Compose nav graph | buttons are trivial; CAMPAIGN/PvE route to Tier 2; **PvP → a "blocked" notice** (match-setup is Q5/Phase-3) |

---

## 5. Tier 2 — buildable once Tier 0 lands (config-seam-blocked, NOT netcode-blocked)

> **Gunsmith + campaign mission-select/briefing: ✅ LANDED.** The Compose gunsmith (`LoadoutSelection`
> seam, labels verbatim from `core::gunsmith`) and the Operations-hub **mission-select + briefing**
> (the single "Seize the Outpost" node → `mission1`, with a difficulty cycler) ship. Campaign opens
> mission-select → briefing → gunsmith → Deploy into `mission1` with the chosen loadout; PvE/PvP open
> the gunsmith and Deploy into Skirmish. The engine now **fully consumes** the wire loadout
> (`new_scene_with_loadout`) and audio gains. **Owed:** the briefing's **difficulty** (needs a `diff`
> wire key + mission-tuning plumbing) and **look-sensitivity** (the Android look delta is derived in
> `engine::touch_controls`, not scalable at the PAL boundary) — both shown/carried but not yet applied
> on Android. **Persistence:** Settings/Profile/loadout now survive restarts via `ShellPrefs`
> (SharedPreferences). **Update:** the shipped campaign is now the **two-node chain** *Seize* →
> *Hold* on both the shared model (`engine::default_campaign()`) and the Android `CampaignModel`
> mirror, with the node→scene launch mapping (`Scene::for_mission`) wired through the backend — but
> the Compose mission-select tiles still render/launch only the root until this D32-blocked chrome
> renders the gated node and threads the selected `launch.node` through. Still pending: PvP
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
5. **Title mode-split + backdrop** (D78 option 1) — CAMPAIGN/PvE/PvP buttons; PvP → blocked notice.
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

These are deliberately **not** done — each is a chunk of real work, and two are symmetric gaps where
each platform is missing the *other's* state:

1. **Campaign progress model — parity reached; the 2-node graph now ships mirrored (was the
   single-node risk).** `CampaignModel.kt` now carries the full `CampaignProgress`/`NodeProgress`
   (Locked/Available/Cleared) derivation, the clear gate, best-tier tracking, and the persistence
   codec — the JVM-testable twin of desktop's `Campaign`. **The shipped campaign is now the two-node
   chain** *Seize* → *Hold* (`engine::default_campaign()`), and `campaignNodes` mirrors it (Hold
   `prerequisites = [0]`, gated behind Seize); the node→scene launch mapping (`Scene::for_mission`)
   is wired on both hosts, and the `CampaignModelTest`/`CampaignProgressTest` pin the 2-node
   structure + the Hold briefing verbatim. So the old "breaks parity the moment a second/gated node
   lands" risk is **resolved** for this node. What's left is only the **D32-blocked Compose chrome**
   (the native mission-select/briefing tiles that read this model) and threading the *selected* node
   index through the Android launch wire (`launch.node`) once that chrome exists — the desktop egui
   hub + the Android backend's node-resolution path are already correct.
2. **Desktop doesn't persist shell prefs.** Settings/Profile/loadout are in-memory only on desktop and
   lost on exit; only `campaign.dat` (campaign progress) persists. **Android is ahead here** — it
   round-trips all three via `ShellPrefs`. The two shells persist *disjoint* state.
3. **Look-sensitivity / briefing-difficulty are carried but inert on Android** — already tracked as
   "owed" in [§5](#5--tier-2--buildable-once-tier-0-lands-config-seam-blocked-not-netcode-blocked)
   (the look delta lives in `engine::touch_controls`, not scalable at the PAL boundary; difficulty
   needs a `diff` wire key + mission-tuning plumbing).
4. **Inverted About entry point** — desktop reaches About from inside Settings (`SettingsAction::About`);
   Android surfaces it as a "FIELD MANUAL" button on the title. A deliberate [D78](../decisions.md) UX
   choice; left as-is, noted so it isn't mistaken for a regression.
