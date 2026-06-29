# Test & feedback hardening plan — verify the game *plays*, not just *computes*

> **Status: COMPLETE (2026-06-29).** All four workstreams landed (WS-1/TF-1 earlier this
> cycle; WS-2/3/4 on 2026-06-29). The `viz-runner` suite now pixel-asserts firing, killing,
> the dark-while-embodied fairness bar, and the hitmarker on a connecting shot; the host
> input→`Command::Fire` pipeline (incl. camera-forward == fire-dir) is covered headless in
> `cargo test`; and the embodied "I hit him" cue (hitmarker + SFX) ships off the
> avatar-source `SimEvent::Damaged` stream. A focused push to close the one class of gap the
> current test setup structurally cannot see. The deterministic-sim tooling is strong —
> headless harnesses (`sim-runner`: phase2/stress/duel/infantry/matchup), a cross-arch
> per-tick checksum matrix (`determinism.yml`), workspace unit tests, clippy/fmt/cargo-deny
> gates. That layer proves the sim is **correct and identical everywhere**. What it does
> *not* prove is that the game is **playable and readable**: that firing shows on screen,
> that the enemy you aim at dies, that the host input pipeline maps a trigger pull to the
> right shot. Two real bugs this cycle — "I can't tell if anything is firing" and "it's
> impossible to kill an enemy" — lived entirely in that unverified perception/input layer.
> One of them (embodied fire picking the lowest-index target) was a genuine logic bug a
> harness *could* have caught; the other half is feedback the game never gave. This plan
> adds the missing layer. **Honest caveat:** the visual harness needs a real GPU, so it is
> a local smoke test (like the existing `viz-runner`), never the no-GPU CI matrix.

---

## Why this exists

A code+tooling audit on 2026-06-28 found the testing surface is lopsided:

- **The render/feedback layer is barely tested.** `viz-runner` renders the real `Game`
  through `Game::frame` to PNG and pixel-asserts a few invariants — but only on the
  **default scene**, and only for UI affordances (command view, selection rim, radial,
  marquee, embodied-dark, alert HUD). It never renders the **combat** scenes, never drives
  **embodied firing**, and never turns on the **debug overlay**. So the muzzle-flash overlay
  (added this cycle) and the embodied-fire kill path have **zero** visual verification.
- **The host input pipeline is untested end-to-end.** Every combat harness constructs
  `Command::Fire { dir }` directly, bypassing the real path: mouse/key → `InputFrame` →
  `engine::fire::fire_command(yaw, fire)` → quantized `dir`. The aim convention (yaw 0 →
  `+X`; the screen-right/`−Y` mapping in `integrate_look_yaw`; camera-forward == fire-dir)
  was verified by *reading code*, not by a test. A regression there is invisible to every
  harness.
- **There is no in-game hit feedback.** A connecting embodied shot produces no hitmarker,
  no target flash, no hit SFX — so even with targeting fixed, a player gets no "I hit him"
  signal. That is a *product* gap, not a test gap, but it is the other half of why "can't
  kill" felt true, so it belongs in the same push.
- **A standing `viz-runner` assertion is red** (`embodied_combat_strategic_map_stays_dark`):
  after combat the "embodied" frame shows command-view content (grid + control-point rings,
  `UNITS: 0`). Likely the avatar dies mid-fight and ejects to command (invariant #5), so the
  frame is post-ejection — i.e. the *scenario* lets the avatar die, not necessarily an
  engine bug. Unconfirmed; the failure has been carried as "known pre-existing."

What's already good and stays untouched: the determinism matrix, the sim-level harnesses,
and the unit-test floor. This plan is **additive** — it does not reopen any invariant, and
invariant #6 (fairness while dark) bounds every new presentation cue.

---

## Workstreams

Built in isolated worktrees, merged in the order below; each lands with tests green
dev+release and (for sim-touching work) the determinism + 2-peer runners still agreeing.

### WS-1 — Visual combat verification *(the big one)*

Extend `viz-runner` to render the **embodied combat scenes** and prove firing/killing on
screen.

- Plumb a **scene selector** into `viz-runner` (it currently hardcodes `Scene::Default`;
  `Game::new_scene` is already public — pass `Scene::Infantry` / `Scene::Duel`).
- Turn the **debug overlay on** (`Game::toggle_debug_hitboxes`) so the muzzle flash + range
  rings render.
- Drive scripted **fire** through the real path: an `InputFrame` with `fire: true` (and a set
  `look`/yaw aimed at a target) across several frames via the existing `advance()` helper.
- Render to PNG (`target/viz/combat_*.png`) for eyeball **and agent visual inspection** (the
  Read tool renders PNGs), and **pixel-assert**:
  - the muzzle-flash overlay appears on a firing frame (`render::debug` `COLOR_MUZZLE`
    `[1.0, 0.95, 0.55]` pixels present where ~zero before);
  - an enemy actually dies under sustained fire (enemy-red pixel count / `UNITS`/`ENEMY`
    readout drops toward zero).
- **Files:** `viz-runner/src/main.rs`; reuse `engine::{Game, Scene}`, `render::debug`.
- **Acceptance:** `pnpm desktop:viz` emits the combat PNGs and passes the new assertions
  locally (GPU-gated, not CI). Depends on the WS-2 baseline being green first.

### WS-2 — Fix the standing embodied-dark viz FAIL *(do first — clean baseline)*

Root-cause `embodied_combat_strategic_map_stays_dark` before adding more viz scenes on top
of a red bar.

- Confirm the hypothesis from the PNG + a sim trace: does the embodied avatar **die and
  eject** during the scripted combat window (so the "embodied" frame is really command
  view)? If so the **scenario** is wrong — keep the avatar alive (or re-embody) so the frame
  under assertion is genuinely embodied.
- If instead the fog/embodiment path is leaking strategic content into the dark frame, that
  is a **real invariant-#6 bug** — fix the fog path and add an `engine` regression test.
- **Never narrow the assertion to make it pass** (invariant #6 / `fix-ci` discipline): a
  desync-class or fairness-class red bar is a real signal.
- **Files:** `viz-runner/src/main.rs`; possibly `engine` fog/embodiment.
- **Acceptance:** `viz-runner` exits 0 with the cause explained in the commit; a regression
  test if it was an engine bug.

### WS-3 — Input-pipeline integration tests *(independent — any time)*

Cover the mouse/key → `Command::Fire` composition the harnesses skip, with no display.

- Test the real seam end to end: a held-fire `InputFrame` at a given yaw must emit
  `Command::Fire` whose quantized `dir` matches `(cos yaw, sin yaw)`; and the embodied
  camera-forward (`embodied_view_proj`'s look dir) must agree with the fire `dir` across
  several yaws (the "you hit what's under the crosshair" guarantee — the bug class behind
  the targeting report).
- Cover the look convention (`integrate_look_yaw`: rightward `look_dx` → view toward `−Y`)
  and crouch (tighter cone) at the composition level, not just the existing per-fn unit
  tests.
- **Files:** `engine/src/lib.rs` (frame-input → command mapping test seam), `pal-desktop`
  input tests.
- **Acceptance:** the composition tests ship in `cargo test` (no GPU); they fail if the aim
  convention or trigger mapping regresses.

### WS-4 — In-game hit feedback *(product; viz-assert it via WS-1)*

Give the player the "I hit him" signal the game never sent.

- Derive a **local-hit** cue from `SimEvent::Damaged` where `source` is the embodied avatar
  (presentation-only; invariant-#6-safe — it is feedback on *your own action*, not map
  intel). Drive a **hitmarker** (crosshair flash) and/or a target damage flash, plus a hit
  **SFX** cue through the existing `pal::Audio` mix.
- This **folds into** the existing roadmap item *UI/UX polish → Game-feel polish (hit SFX +
  VFX)* — track it there, don't double-count.
- **Files:** `engine` (consume `SimEvent::Damaged` for the avatar → render/audio cue),
  `render` (hitmarker pass), `pal` audio cue.
- **Acceptance:** the WS-1 combat viz scene shows the hitmarker on a connecting shot; a unit
  test on the "did the avatar land a hit this tick" derivation.

---

## Sequencing & dependencies

```
WS-2 (fix red bar) ──► WS-1 (combat viz)  ──►  WS-4 viz-assert
                                   ▲
WS-3 (input tests, independent) ───┘ (can land any time)
```

WS-2 first (don't build on a red bar). WS-1 is the keystone — it makes firing/killing
*visible* and is what lets me (or a reviewer) verify by looking, not just by checksum. WS-4
lands after WS-1 so its cue can be pixel-asserted. WS-3 is independent and can slot in
whenever.

## Risks & notes

- **GPU-gated.** WS-1/WS-2/WS-4-viz need a real adapter, so they stay local smoke tests like
  today's `viz-runner` — they are *not* added to the no-GPU CI matrix. The determinism matrix
  remains the load-bearing CI gate.
- **No invariant reopened.** Every new cue is presentation-only and bounded by invariant #6;
  no sim state, no checksum surface. WS-3's seams are float-at-the-boundary host code, exactly
  like the existing `fire`/`locomote` quantization (invariant #1 unaffected).
- **Pixel asserts are coarse by design** — colour-bucket counts with margins, not exact
  frames (the established `viz-runner` style), so they survive trivial render tweaks but still
  catch "the flash never drew" / "the enemy never died."
