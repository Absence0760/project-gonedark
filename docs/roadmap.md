# Roadmap

> Build order and milestones. The sequencing reflects the project's three biggest
> risks: **touch controls** (a product risk, not an engine one), **embodied combat feel
> over the network** (the FPS layer rides RTS-optimal lockstep — see Phase 0.5), and
> **determinism** (a correctness risk that gets exponentially harder to retrofit). All
> are pulled as early as possible.
>
> Cross-platform (Windows/Linux/Android/iOS) threads through every phase rather than
> being a phase of its own — see [`platforms.md`](platforms.md). The key rule: the
> Platform Abstraction Layer boundary goes in at Phase 0 so platform code never leaks
> into the core. Develop on Linux desktop; ship Android-first; iOS last (most
> external friction).

## Phase 0 — Control prototype *(do this before anything else)*

> **Status: PASSED (2026-06-23, [`decisions.md`](decisions.md) D14).** The embody↔command
> loop feels good in hand, validated on real hardware (Galaxy S24). Touch-feel risk
> retired; resolves [`open-questions.md`](open-questions.md) Q4. The throwaway prototype
> (`prototypes/phase0-controls/`, a Godot build) has since been deleted on Phase 1
> completion (D22). Two caveats carried into D14: audio is still faked, and embodied feel
> *over the network* is untested — that's Phase 0.5.

**Goal:** prove the core interaction is fun on a touchscreen before building any
systems behind it.

The first risk to hit — and the one this phase exists to kill — is not the engine: it's
whether *CoH*-style command **and** a competent FPS scheme **and** an instant swap
between them feel good on a small touchscreen. If this isn't fun, no amount of engine
work saves it. (Embodied feel *over the network* is the next risk — Phase 0.5.)

- [x] Throwaway prototype (can be in anything fast — even a non-final engine).
- [x] One controllable unit; tap-to-select / order on the command layer.
- [x] Embody → FPS controls → surface, with the swap feeling instant.
- [x] The "world goes dark" vignette + an alert ping, faked.
- [x] **Exit criterion:** the embody ↔ command loop feels good in hand. Kill or rework
  the concept here if it doesn't.

## Phase 0.5 — Embodiment-over-network latency spike *(before the engine spine)*

> **Status: PASSED (2026-06-23, [`decisions.md`](decisions.md) D15).** Embodied combat
> feels good over lockstep **with avatar-local prediction** (raw lockstep felt laggy),
> validated phone-vs-laptop over real Wi-Fi up to a simulated "cellular" link. Resolves
> [`open-questions.md`](open-questions.md) Q7; Q8 (tick rate) resolved in Phase 1 via D16
> (30 Hz too coarse) + D21 (global 60 Hz). The throwaway harness (`prototypes/phase0.5-netfeel/`)
> has since been deleted on Phase 1 completion (D22). Plan: [`phase-0.5-plan.md`](phase-0.5-plan.md).

**Goal:** prove embodied FPS combat feels acceptable under the chosen
deterministic-lockstep + input-delay netcode — *before* committing the full engine.

The netcode is RTS-optimal and FPS-hostile: input delay executes orders a few ticks
ahead, with no prediction/rollback/lag-comp (see
[`architecture.md`](architecture.md) §"Embodied combat over lockstep — the open
tension" and [`open-questions.md`](open-questions.md) Q7/Q8). Phase 0 can't surface
this — it's
single-unit and local. If embodied combat feels laggy over the wire, you want to know
*now*, not after building the ECS, renderer, and systems on top of an unfit netcode
model.

- [x] Throwaway, like Phase 0 — minimal, not the final engine.
- [x] **Two networked clients**, one embodied unit each, fighting under *real* input delay
  (and at the real 30 Hz tick, to test Q8 alongside Q7).
- [x] Try **avatar-local prediction** (predict only your own embodied entity, reconcile
  against the tick) if raw lockstep feels bad — the current lean for Q7.
- [x] **Exit criterion:** a credible path to good embodied combat feel over the net — *or*
  a decision to change the netcode model (Q7) or tick rate (Q8) **before** Phase 1.
  Retrofitting a prediction/rollback boundary into a finished sim is far costlier than
  designing to it.

## Phase 1 — Vertical slice

> **Status: DONE — PASSED ([`decisions.md`](decisions.md) D22).** The custom Rust engine is
> validated end-to-end on real arm64 (Galaxy S24, Adreno 750). **On-device evidence:**
> `pnpm android:checksum` confirmed the device sim-runner checksum stream **bit-identical** to
> desktop over 300 ticks (`4c34c6b5951edf57`); the `adb logcat` FPS heartbeat showed
> **120 fps** sustained at the locked **60 Hz** sim tick — demonstrating sim/render decoupling
> live on hardware. One unit moves via the flow field; tap-to-move works; the two-finger embody
> toggle flips the world dark. All three decide-first gates locked (D17, D18, D21). The
> **Unity/Godot fallback (D8) is retired**; the throwaway prototypes are deleted. **Phase 2
> (game systems) is the active phase.** Honest caveat: validated on a flagship; frame-rate/thermal
> on mid-range silicon and the 200-unit power budget are Phase 3 (D21). Detailed plan and
> sign-off record: **[`phase-1-plan.md`](phase-1-plan.md)**.

**Goal:** the real engine spine, end to end, with one of everything.

- [x] ECS world + scheduler; data-oriented component storage.
- [x] Fixed-tick deterministic sim loop + render interpolation. **Sim rate locked
  ([D21](decisions.md), closing Q10):** 30 Hz was too coarse for embodied combat (D16), so the
  loop runs a single **global 60 Hz** tick (`core::sim::TICK_HZ = 60`) — with one unit on real
  arm64 it has huge headroom, so dual-rate is unjustified now and **deferred to Phase 3** (the
  200-unit thermal re-evaluation), not killed.
- [x] Embodiment as an input-source swap on a single entity; fog → avatar-only on embody. Wire
  the **avatar-local-prediction boundary (D15) from the first netcode commit** — presentation
  path only, never writing sim state.
- [x] Minimal Vulkan renderer (instanced units), camera, top-down view.
- [x] One unit type moving via a flow field on screen.
- [x] **Validate on real arm64 hardware**, not just the emulator.
- [x] **Exit criterion (met — D22):** one unit, commandable and embodiable, running
  deterministically at target frame rate on a target device. Passed on Galaxy S24;
  fallback retired (D22).

## Phase 2 — Game systems

> **Status: SIGNED OFF — systems-complete ([`decisions.md`](decisions.md) D31), device-audio +
> feel-playtests carried forward.** Systems spine ([D23](decisions.md)) + host wiring
> ([D24](decisions.md)) landed. A first,
> fully-deterministic implementation of every bullet below lives in `core` as eight new modules
> (`terrain, combat, economy, territory, fog, orders, alerts, event`): fixed-point combat with
> suppression/cover/line-of-sight, territory capture, resources/economy/camps + production, fog
> of war (a pure client-side derivation, not sim state), the widened order/stance vocabulary with
> a literal-executor + retreat trigger, and the alert channel. All fixed-point, float-free, and
> folded into the per-tick checksum (territory/economy are sim state; fog/alerts are excluded as
> derived presentation). `core` tests grew 57 → 128 (green dev + release); the headless
> `sim-runner` scenario now exercises the systems so the cross-arch determinism matrix covers
> Phase 2. **The host/presentation wiring is now in ([D24](decisions.md)):** fog rendering, the
> embodied alert HUD, the embodied audio mix, and the touch UI (multi-unit selection + the
> order/stance vocabulary on screen) — all pure presentation derivations, so the checksum stream
> stayed byte-identical. **A polish round ([D26](decisions.md)) then made it real and checkable:**
> desktop **audio output** renders the mix through a `cpal` stream (opt-in `audio` feature,
> procedural placeholder sounds — `pnpm play:audio`); command-layer **selection is now drawn** (a
> white rim); the full order/stance vocabulary (patrol/hold/fall-back/retreat) is reachable
> ([D25](decisions.md), which also corrects a mis-scoping in D24); combat lethality + the economy
> tables got a **first-pass balance baseline** — since **measured** into a more-justified baseline
> against a deterministic balance-metrics harness (`sim-runner --metrics`: TTK / equal-cost-trade /
> suppression-pin / economy-ramp), which fixed two degeneracies the first pass hid (a
> strictly-dominated Heavy and cosmetic suppression) ([D30](decisions.md)); and a headless
> **offscreen render harness** (`viz-runner`, `pnpm desktop:viz`) now asserts these behaviors with
> real pixels. **Honest caveats (still NOT done):** balance is a *measured baseline*, not final
> feel (the numbers — incl. the 30% retreat default — still expect to move from human playtests);
> audio sounds are *procedural placeholders* (no asset/design pass); and the netcode/lockstep layer
> is Phase 3. **The Android AAudio sink is now real** — `oboe`/AAudio via the shared host-tested
> `pal::mix` seam, for-target-built but with on-device audibility still owed ([D29](decisions.md)).
> **Phase 2 is signed off systems-complete ([D31](decisions.md))**: everything above is built,
> tested, and verified by automation (full suite dev+release, the CI cross-arch checksum matrix,
> the `viz-runner` real-pixel assertions, the `--metrics` balance digest); what remains is a
> human/device confirmation layer (on-device audio by ear, balance feel by hand, the on-device
> `adb` checksum leg), carried forward not faked. Open forks Q1/Q2/Q3 are deliberately left open —
> fog and alerts ship as a *mechanism*, not a lock; each lean was reaffirmed at Phase-2 close (D31)
> but locking depends on real audio / live PvP that doesn't exist yet. **A later
> [playability push](playability-plan.md) (D37–D40) then closed the remaining *functional* gaps
> the systems sign-off left** — embodied firing ([D37](decisions.md)), a win/lose evaluator
> ([D38](decisions.md)), the enemy commander AI ([D39](decisions.md)), and a real first-person
> world ([D40](decisions.md)), plus in-match text — without disturbing the signed-off systems;
> by-hand feel and the art pass are still owed.

**Goal:** the actual game.

- [x] Combat, suppression, cover, line-of-sight.
- [x] Territory capture, resources, economy.
- [x] Camp building & upgrading.
- [x] Fog of war (and its interaction with embodiment).
- [x] The **order/stance system** — the real depth layer (patrol routes, engagement
  ranges, retreat triggers, trigger zones, queued production). This is where "smart
  play" lives, per the design.
- [x] Literal-executor unit AI; abilities/orders.
- [x] Alert channel + the embodied audio mix (strategic sound bleeding into FPS).

## Phase 3 — Scale & net

> **Status: IN PROGRESS.** Plan and four-workstream sequencing:
> **[`phase-3-plan.md`](phase-3-plan.md)**. Workstream A underway: a deterministic **200-unit
> stress scenario** + on-device timing mode in `sim-runner` (`pnpm desktop:sim:stress`) showed 200
> units running **~2× over the 16.6 ms 60 Hz budget on desktop**, pinpointing the per-unit
> flow-field rebuild (not threading) as the #1 cost. **Flow-field caching then landed** (one field
> per distinct goal per tick, bit-identical) → **~8× faster, ~3.7 ms/tick median, comfortably under
> budget** — which also makes sim-side parallelism likely unnecessary for Phase 3. Workstream B
> opened: **D27 (netcode topology) is decided and its first slice landed** — `core::lockstep`, a
> platform-free **sans-I/O** deterministic 2-client loop + wire codec, verified in-process over a
> lossy/jittery/reordering channel (peers' checksums agree + match a no-network reference; no
> sockets yet). The snapshot format ([D28](decisions.md)) and **Q2** (enemy detection of "gone
> dark", [D33](decisions.md): a tunable tell, default Subtle) are now both decided, and the
> `core::detection` mechanism has landed; the *two-human* PvP mind game still needs the net layer.

**Goal:** make it hold up at size and (if pursued) in multiplayer.

- [ ] 200-unit stress tests; job-system parallelism; profiling on target hardware.
- [ ] Deterministic lockstep netcode; input delay; per-tick checksum diffing in CI.
- [ ] Reconnect/snapshot handling; Wi-Fi↔cellular handoff.
- [ ] PvP attention mind-game tuning (see open questions: enemy detection of "gone dark").

## Phase 4 — Polish & ship

> **Status: OPENING (plan landed) — Phase 3 still IN PROGRESS.** Plan and workstream sequencing:
> **[`phase-4-plan.md`](phase-4-plan.md)**. Per [D32](decisions.md) the out-of-match app shell is
> **native per-platform** (reached through a narrow shell↔sim seam), with the **in-session** shell
> in-engine. **The seam prerequisite has landed:** `core::shell` — a GPU-free, logic-free façade
> (intent in, presentation-safe view out; fairness structural via no `&World`), recorded in
> [D34](decisions.md). **The four buildable-now Rust workstreams have all landed** — A (seam ✅) · B
> (in-engine in-session shell ✅) · C (device tiers / dynamic-res / thermal ✅) · D (telemetry +
> consent gate ✅); full suite green dev+release. The **native out-of-match shells** are next —
> **"Boot & title" has now landed on both Android and desktop** — the **Android Compose landing
> screen** ([D35](decisions.md)) and the **desktop egui title screen** ([D36](decisions.md)), each
> the first native surface buildable once the seam landed; only the **iOS** Boot & title shell is
> still pending (no iOS target at all). The remaining surfaces stay pending — Settings, onboarding,
> match setup, lobby, store, and consent — deferred behind missing per-platform UI projects and the
> [Q5](open-questions.md)/[Q9](open-questions.md)/[Q11](open-questions.md)/Phase-3 blockers.

**Goal:** wrap the game in everything that ships *around* the match — the app shell, the
storefront, the first-run teach — and tune it to mid-range silicon.

- [x] Thermal/battery tuning; device quality tiers; dynamic resolution.
- [x] Telemetry + live-ops scaffolding (consent-gated, per [`infrastructure.md`](infrastructure.md)).

### Meta-UI / app shell — the screens *around* the match

Phase 2 built the **in-match** UI (the touch command UI, fog, the embodied alert HUD —
D24/D25). What's still unbuilt is the **app shell**: every screen the player touches before,
between, and after a match. It is one body of work, scoped here so it doesn't get smuggled in
piecemeal. The shipping touch *gameplay* scheme (D14/Q4) is **not** part of this — that lives
in the in-match layer; the **settings surface that configures it** is.

| Surface | What it covers | Depends on |
|---|---|---|
| **Boot & title** | Splash, title/attract screen, build-channel + version stamp. **Landed (Android [D35](decisions.md) + desktop [D36](decisions.md)):** a native Jetpack Compose title/landing screen on Android (the launcher) and a native egui title screen on desktop (`app` now opens here, Start enters the match); only the iOS shell still pending (no iOS target) | — |
| **Onboarding / tutorial** | Teach the going-dark cost; telegraph the blindness *before* it bites; a guided first-possession beat. The single most important screen — invariant #6 lives or dies on whether a new player reads a loss as *"I stayed too long"* | [Q5](open-questions.md) (PvE-first is the natural teach surface); invariant #6 |
| **Settings** | Graphics tiers (↔ device quality tiers above), audio-mix levels, the touch-layout / rebind editor (configures the D14 scheme), desktop key/gamepad rebinds, **accessibility** | invariant #6 (see accessibility note) |
| **Match setup** | Army/loadout composition, map + mode select; skirmish-vs-PvP entry | order/stance vocab (D25) |
| **Lobby & matchmaking** (PvP) | Party/invite, connection-quality readout, ready-up. **Seam:** the net plumbing is Phase 3 (D27 lockstep, reconnect/handoff); only the *surface* is Phase 4 | Phase 3 netcode; [Q5](open-questions.md) |
| **Progression & profile** | Persistence, stats, cosmetic inventory | account/persistence backend ([`infrastructure.md`](infrastructure.md)) |
| **Store / IAP** | Cosmetic purchases, restore-purchases, receipts, refund paths | [Q9](open-questions.md) (per-platform billing rails); [Q11](open-questions.md) (hero-tier cosmetics feed the catalog) |
| **Consent & legal** | Telemetry/privacy consent, age gate, ToS/EULA — **gates** store + telemetry, so it precedes them | [`infrastructure.md`](infrastructure.md) |
| **In-session shell** | Pause, surrender/leave, post-match summary, reconnect prompt | reconnect/handoff is Phase 3 |

**Cross-cutting constraints:**

- **Fairness (invariant #6) outranks the shell.** No meta-UI element — not a notification, a
  reconnect toast, nor a post-match teaser — may leak strategic intel *while embodied*. The
  in-session shell renders under the same avatar-only fog as the game.
- **Accessibility is load-bearing here, not optional polish.** The going-dark alert channel is
  a directional **flash + audio** (invariant #6); a colorblind or hard-of-hearing player needs
  an equivalent cue or the core mechanic is unfair to them. The settings surface owns this.
- **One shell or four?** Settled in **[D32](decisions.md)**: **native per-platform shells** for
  these out-of-match surfaces (SwiftUI / Jetpack Compose / a desktop shell), with the **in-session**
  shell (pause/reconnect/post-match) kept **in-engine** because it renders under avatar-only fog
  (invariant #6). Invariant #2 holds — the fork is *chrome*, not game logic; the sim/netcode/order
  vocab stay single-sourced in `core`, reached through a narrow GPU-free, logic-free shell↔sim seam.

---

## PvE — the Operations campaign *(the first shippable product)*

> **Status: DESIGNED, build pending.** [D58](decisions.md) resolves [Q5](open-questions.md) →
> **PvE-first, PvP fast-follow**: the single-player **Operations campaign** is the first shippable
> product and the onboarding surface for *going dark* (invariant #6). Design:
> **[`pve-campaign.md`](pve-campaign.md)** + **[`customization.md`](customization.md)**; decisions
> [D58](decisions.md)–[D61](decisions.md); execution plan: **[`pve-campaign-plan.md`](pve-campaign-plan.md)**.

**Goal:** a stranger can install the game and play a campaign of missions — learning the
blindness cost in a place that punishes overstaying *honestly* — with a movable HUD and a
horizontal weapon-customization loadout. This is net-new content scope sitting on the
systems-complete, playable engine (D31, D37–D40); it is **not** an engine-risk phase, so it's
scoped here as its own pillar rather than renumbered into the risk-ordered phases above.

The campaign is a **CoH/Delta-Force Operations hub** — a node-graph of replayable missions with
difficulty tiers + scenario-parameter modifiers. Missions are **data** (a parameterized scenario +
an objective set); objectives are **host-side derivations off the deterministic event stream**, so
they add **zero checksum/desync surface** (the same footing as `evaluate_outcome`, fog, and alerts).
Everything anchors to seams that already exist — `evaluate_outcome`/`FactionForces`
([`engine/src/session_shell.rs`](../engine/src/session_shell.rs)), the data-driven `Sim::new` +
spawn path ([`core/src/sim.rs`](../core/src/sim.rs)), the honest `commander_orders`
([`core/src/commander.rs`](../core/src/commander.rs)), and the `SimEvent`/`territory`/`alerts`
streams.

| WS | Workstream | What it builds | Anchors |
|---|---|---|---|
| **A** | **Mission/objective core** | Host-side `Objective`/`ObjectiveSet` evaluated post-`Sim::step` off `SimEvent` (generalizes `evaluate_outcome`); the **Seize** archetype + the **"10 troops, take the base"** first mission. Ships with `core`/`engine` tests (green dev+release, determinism matrix green). | `event.rs`, `session_shell.rs`, `sim.rs` |
| **B** | **Operations hub** | Node-graph meta-progression, unlock state, mission-select + briefing surface (native shell, [D32](decisions.md)). | `core::shell` seam, native shells |
| **C** | **Gunsmith loadout** | Fixed-point attachment-delta model in `core` (checksum-folded, sidegrades-only per [D60](decisions.md)) + pre-match loadout UI. Ships with `core` tests. | `combat.rs`, weapon component, `persist` fold |
| **D** | **HUD layout editor** | Per-layer drag/resize/opacity layout editor over the existing touch seams ([D61](decisions.md)); the build-out of the touch-layout settings item below. Presentation/input only. | `engine::touch_controls`, `render::touch_controls`, Settings shell |
| **E** | **Difficulty + modifiers + narrative glue** | A deterministic `difficulty` tier on `commander_orders`; scenario-parameter modifiers; light briefing framing. | `commander.rs`, scenario params |

**Sequencing:** A is the spine (a playable first mission proves the loop); B wraps it into a
campaign; C/D/E layer on. WS-A is the natural next code slice after this design lands. Open forks
threaded through: co-op ([Q14](open-questions.md)), mission authoring format ([Q15](open-questions.md)),
narrative depth ([Q16](open-questions.md)).

---

## Path to publishable — completion checklist

> A flat, checkable list of what stands between the current build (systems-complete +
> playable — D31, D37–D40) and something you'd hand a stranger or a store reviewer. These
> items **re-cut the phases above by "is it shippable,"** not by engine risk, so they
> deliberately overlap Phase 4's app shell and [`content-pipeline.md`](content-pipeline.md).
> Nothing here reopens a locked invariant — it's product completeness, not architecture.
> A box is checked **only** where the work has actually landed (decision id cited); the
> rest is the real remaining work.

### Playable game loop — building & upgrading troops

- [x] Camp building, production & economy exist in `core` ([D23](decisions.md))
- [x] Win/lose evaluator + match end ([D38](decisions.md))
- [x] Order/stance vocabulary reachable on the touch UI ([D25](decisions.md))
- [x] **Build menu UI** — place/queue structures from the command view
  (`engine::build_ui::build_commands` seam; B-key on desktop, [D48](decisions.md))
- [x] **Troop-training UI** — pick a unit type, see cost + queue + ETA
  (`engine::train_ui::train_commands` seam; R/H keys on desktop, [D48](decisions.md)). *Rally
  point: the `rally_point` quantization seam exists, but emitting it awaits a camp-rally sim
  command (flagged follow-up — there is no `Command` for a building's spawn rally yet).*
- [x] **Camp upgrades** — a readable tier display + one-button level-up
  (`engine::upgrade_ui::upgrade_commands` seam; U-key on desktop, [D48](decisions.md)). *Linear
  camp-tier leveling today; a richer per-structure/per-unit prerequisite **tree** is a `core`
  follow-up (new sim state + a `Command` variant), not a presentation change.*
- [x] Resource/economy readout that makes cost and income legible at a glance
  (`render::readout::EconomyReadout` — banked credits + income rate)
- [ ] A full match a new player can complete start→finish unaided (loop closes; needs the
  UI above wrapped around the D38 evaluator)

### Campaign & content — the first shippable product (PvE)

> Net-new content pillar ([D58](decisions.md)–[D61](decisions.md)); design in
> [`pve-campaign.md`](pve-campaign.md)/[`customization.md`](customization.md), build sequencing in
> [`pve-campaign-plan.md`](pve-campaign-plan.md). All unchecked — this is the active design→build
> frontier.

- [ ] **Mission/objective core (WS-A)** — host-side `Objective`/`ObjectiveSet` off the `SimEvent`
  stream (generalizes [D38](decisions.md)'s `evaluate_outcome`); zero checksum surface; ships with
  `core`/`engine` tests + determinism matrix green
- [ ] **Mission 1 — *Seize*** ("10 troops, take the enemy base"): the first playable mission and
  the going-dark teach beat
- [ ] **Operations hub (WS-B)** — node-graph meta-progression, unlock state, mission-select +
  briefing (native shell, [D32](decisions.md))
- [ ] **Gunsmith loadout (WS-C)** — fixed-point sidegrade attachment model, checksum-folded, +
  pre-match loadout UI ([D60](decisions.md))
- [ ] **HUD layout editor (WS-D)** — per-layer drag/resize/opacity presets over the existing touch
  seams, presentation/input-only, invariant-#6-bounded ([D61](decisions.md); also tracked under
  *UI / UX polish* below)
- [ ] **Difficulty + modifiers (WS-E)** — deterministic `commander_orders` difficulty tier; rotating
  scenario-parameter modifiers (never balance-number hacks)
- [ ] **PvP fast-follow** — the multiplayer pillar on the same lockstep core (after the PvE loop is
  proven; Phase 3 netcode is the prerequisite)

### UI / UX polish — make it read as a product

- [x] In-match command HUD, selection rim, embodied alert HUD ([D24](decisions.md)/[D26](decisions.md))
- [x] Native title screens — Android Compose ([D35](decisions.md)) + desktop egui ([D36](decisions.md))
- [ ] **Visual-design pass** on the command HUD — consistent iconography, type scale,
  spacing, colour language (so it looks intentional, not greybox)
- [ ] Touch-layout / rebind editor + correct touch-target sizing (the D14 scheme's settings surface).
  **Now scoped as the CoD-Mobile/MLBB HUD layout editor** — per-layer drag/resize/opacity presets,
  presentation/input-only, invariant-#6-bounded ([D61](decisions.md); PvE pillar WS-D)
- [ ] Onboarding / first-possession tutorial (teach the going-dark cost — invariant #6 lives here).
  **Lives in campaign mission 1** — the *Seize* "10 troops, take the base" beat scripts the
  overstay temptation ([`pve-campaign.md`](pve-campaign.md) §3; PvE pillar WS-A)
- [x] In-session shell — pause, surrender/leave, post-match summary. **Fully landed:** the
  post-match summary surface + its DISMISS button → leave-match → return-to-title transition
  ([D52](decisions.md)); pause overlay (Esc on desktop ([D53](decisions.md)), back-gesture on
  Android ([D54](decisions.md))) + in-match surrender are now wired ([phase-4-plan WS-B](phase-4-plan.md)).
- [ ] Settings — graphics tier, audio-mix levels, rebinds, **accessibility** (an equivalent
  cue for the directional-flash + audio alert channel)
- [ ] Game-feel polish — build/select/hit SFX + VFX, button states, screen transitions

### Art & assets — AI-generated placeholders (skip custom 3D for now)

- [x] **Adopt AI-generated placeholder models** for units, structures, and the embodied
  weapon instead of commissioned art ([D41](decisions.md)) — this pulls the "AI-assisted"
  route that [`content-pipeline.md`](content-pipeline.md) §2 reserved for *hero* art forward
  to *everything*, sitting at the greybox/low tier of the production ladder. **Done:** nine
  procedural greybox models, all now drawn — units (Rifleman→infantry, Heavy→tank), the camp,
  the first-person weapon, and the scenery/cover props ([D50](decisions.md))
- [x] One source `.glb` per unit/structure run through the cook → LOD chain so it
  satisfies the two-view filter (top-down token *and* eye-level mesh — §4). **Done:** the cook
  now emits a real **3-tier gltfpack LOD chain** per model, distance-selected at runtime
  ([D49](decisions.md)); the ASTC/atlas/LZ4-pak half stays Phase-3 follow-on
- [x] License-clean & logged — generated assets recorded in the asset manifest (§ license hygiene).
  **Done:** every tier carries `source`/`license`/`sha256` in `assets/models/manifest.json`
- [x] A consistent placeholder visual language across units/structures/world so the build
  looks deliberate rather than unfinished. **Done:** one harmonized greybox palette
  (units muted faction-neutral, structures warm/steel, scenery desaturated) in `gen_models.py`
- [x] FPS-view world dressing beyond the existing ground/sky/cover ([D40](decisions.md)) —
  enough to read as a *place*. **Done:** static scenery + cover props (trees, boulders, crates,
  sandbag berms, turrets) drawn in the embodied view, LOD-by-distance ([D50](decisions.md)) —
  and the embodied view now also draws the **fog-filtered, avatar-visible sim units** themselves
  (line-of-sight enemies/allies), not just static props ([D52](decisions.md))

### Release readiness — the store-facing layer

- [ ] Match-setup / skirmish-entry screen (Phase 4)
- [ ] Consent & legal gate (ToS / privacy / age) — gates telemetry + store, so it precedes them
- [ ] Store listing — icon, screenshots, description + a Play Console build channel
- [ ] Performance / thermal pass on mid-range arm64 (the honest Phase 1/3 caveat — not yet validated off-flagship)
- [ ] Crash + telemetry consent wiring verified end-to-end (telemetry/consent gate landed, Phase 4 workstream D)

---

## Competitive parity — reaching the incumbents' bar

> Where we sit against the field — Delta Force, CoD/Warzone Mobile, the FPS/RTS hybrid
> graveyard (Eximius/Silica/NS2), and the CoH/StarCraft RTS lineage — is analysed in
> **[`positioning.md`](positioning.md)**. The strategy in one line: **we lead on the four
> things that define the product (command + embody, vision-as-cost, a deterministic sim built
> for 200-unit lockstep — substrate landed, on-device scale is Phase 3, symmetric hybrid PvP)
> and lag on the production-value table-stakes every mobile
> shooter has polished for years.** These **CP-n** items reach *good-enough* on the
> table-stakes so the shooter half never embarrasses the hybrid — none reopens an invariant;
> each is bounded by #1 (determinism), #3 (literal-executor AI), #5 (embodiment), #6 (fair
> dark). They **re-cut** existing PvE/Phase-3/Phase-4 work by "does it close a competitive
> gap," so several deliberately overlap the pillars above rather than adding net-new scope.
>
> **Sequencing:** CP-7 + CP-2 are launch-critical (they gate whether a stranger *gets* and
> *enjoys* the core); CP-1 + CP-4 are launch-important (shooter-audience table-stakes);
> CP-3/CP-5/CP-6/CP-8 ramp after the PvE loop is proven. The **LEAD** rows need *protection*
> (hold the determinism gates + the one-player-both-jobs symmetry), not new work.

- [ ] **CP-1 — Gunsmith to mobile-expected depth.** Extend the WS-C sidegrade model
  ([D60](decisions.md)) to the attachment-category breadth a CoD-Mobile player expects (optics,
  barrel, stock, mag, grip, muzzle) — **horizontal only** (sidegrades, fixed-point,
  checksum-folded; never vertical power). *Builds on PvE WS-C.*
- [ ] **CP-2 — Embodied game-feel bar (launch-critical).** A focused gunplay pass so a Delta
  Force player doesn't bounce in ten seconds: hit feedback (impact/hitmarker/damage-direction),
  recoil/kick readability, responsive ADS, audio-coupled firing. **Presentation/feel only — never
  sim state (#4).** Define a written "good-enough floor" and playtest against it.
- [ ] **CP-3 — Animation/fidelity floor (conceded tier).** A "not jarring" floor — coherent
  locomotion/fire/death anims on the greybox so the eye-level view reads as a *place* — via the
  scripted pipeline ([`content-pipeline.md`](content-pipeline.md), [D41](decisions.md)/[D46](decisions.md)).
  **Explicitly not UE5 parity**; we concede photoreal fidelity and compete on the hybrid.
- [ ] **CP-4 — Mobile HUD + touch polish.** Ship the per-layer HUD layout editor (PvE WS-D,
  [D61](decisions.md)) + a touch-target/rebind pass so controls feel CoD-Mobile-class. *Overlaps
  the touch-layout editor under UI/UX polish above.*
- [ ] **CP-5 — Unified cross-platform entitlement.** One account/entitlement layer so progression,
  loadouts, and cosmetics follow the player across Android/iOS/desktop — the cross-progression
  Warzone Mobile trained the market to expect ([Q9](open-questions.md) billing rails feed this).
- [ ] **CP-6 — Audio identity pass.** Replace the procedural placeholders
  ([D26](decisions.md)/[D29](decisions.md)) with a deliberate sound identity via the scripted
  Csound/SoX pipeline — **load-bearing, not polish** (audio is the going-dark alert channel, #6);
  keep the accessibility-equivalent cue.
- [ ] **CP-7 — Onboarding that teaches the twist (launch-critical).** A new player must read their
  first death as *"I stayed too long"* (#6). Built into PvE mission 1
  ([`pve-campaign.md`](pve-campaign.md) §3, WS-A). No incumbent has the twist, so we can't borrow
  this teach — we have to nail it. *Overlaps the onboarding item under UI/UX polish above.*
- [ ] **CP-8 — Live-ops / content-cadence engine.** Wire the `server` scaffolding
  (telemetry/consent/live-ops) into the rotating scenario-parameter modifier system (PvE WS-E) for a
  sustainable post-launch cadence — **modifiers and content, never balance-number or power hacks**
  (#1/#6).

**PC-facing parity** — meeting a seated, genre-literate player's expectations without forking the
game (full analysis: [`positioning-pc.md`](positioning-pc.md)):

- [ ] **PC-1 — Mouse-and-keyboard combat feel.** The embodied layer must feel right with a *mouse*,
  not just thumbs — precise aim, sensible defaults, FOV control. PC players notice instantly.
  (Pairs with CP-2.)
- [ ] **PC-2 — PC control & options surface.** Full rebinds, graphics options, ultrawide /
  high-refresh / high-DPI support — the settings depth a PC player expects.
- [ ] **PC-3 — Replays & spectating (a determinism freebie).** A match is a seed + an input log
  (invariant #1), so replay + spectator view are *cheap* and a real PC / e-sports differentiator.
- [ ] **PC-4 — Mods / data-driven content.** Missions/scenarios are already data
  ([Q15](open-questions.md)); exposing them as moddable content is how StarCraft/Total War lasted
  decades — a PC-only longevity lever.

**Cross-platform parity** — keeping *one game* fair and coherent across phone/PC/console (full
analysis: [`positioning-cross-platform.md`](positioning-cross-platform.md)):

- [ ] **XP-1 — Cross-save & handoff.** Match/campaign/progress state lives server-side so you stop on
  one device and resume on another ("commute on your phone, finish on PC").
- [ ] **XP-2 — Input-based matchmaking policy.** For embodied PvP, decide the thumb-vs-mouse fairness
  model — **resolve [Q17](open-questions.md) *before* building PvP, not after.** PvE needs none of this.
- [ ] **XP-3 — Unified entitlement / one wallet** *(= CP-5)*. One account; unlocks/cosmetics follow the
  player; per-platform purchases ([Q9](open-questions.md)) all resolve into it. (Same work as CP-5,
  viewed cross-platform — not a separate build.)
- [ ] **XP-4 — Control parity without forking.** Each platform gets a native-feeling control scheme
  (touch / mouse+kbd / controller) over the **same shared core** (invariant #2) — controls differ, the
  game does not.

> **Protect the LEAD (no new work, but do not let it erode):** the determinism gates
> (`determinism.yml` cross-arch matrix, per-tick checksum diffing) and the **symmetric** hybrid
> PvP shape — each player is their own commander-and-avatar, never the asymmetric
> commander-vs-grunts split that killed every prior hybrid ([`positioning.md`](positioning.md) §3).
> Co-op ([Q14](open-questions.md)) is the one place that temptation returns; if built, each player
> keeps both jobs.

---

## Dev workflow & iteration

Native Rust doesn't hot-reload engine code for free — that's the iteration cost of the
performance ceiling, and the one real tradeoff of the language choice (D10). Options,
cheapest-value-first:

- **Automated edit→build→deploy→test loop** — `edit → cargo build (cargo-ndk for
  Android) → adb install → am start → adb logcat`. A coding agent can script the whole
  cycle and read logcat to self-diagnose crashes. The default; no special architecture.
- **Scripting / config hot reload** — keep tuning and balance in Lua or data files;
  reload instantly, zero recompile. **Best value for iterating on game feel — and the
  primary mitigation for Rust's weaker engine-code reload.** (iOS: interpreter mode
  only, no JIT.)
- **Asset hot reload** — watch textures/configs, reload at runtime. Easy, worth it
  early.
- **Reloadable game module** — game/sim logic behind a reload boundary so it survives a
  swap while the host owns state. In Rust this means `hot-lib-reloader` /
  `dexterous_developer` (hackier than a C++ `.so` swap) — adopt **only if** the build
  loop + scripting layer stop being enough, not up front.

**Emulator caveat:** the Android Emulator runs x86_64 — build that ABI in debug for
fast iteration — but its GPU and thermal behavior won't match a mid-range arm64
phone. Iterate logic on the emulator; **profile performance on real target devices.**

---

## Top risks

| Risk | Why it's dangerous | Mitigation |
|---|---|---|
| **Touch controls** | CoH controls were built for mouse+keyboard; layering FPS + instant swap on a touchscreen is harder than any engine problem here | **Phase 0 — PASSED (D14):** prototype felt good in hand on a Galaxy S24. Shipping touch UI landed in Phase 2 (D24/D25) |
| **Embodied combat feels laggy** | Lockstep + input delay is RTS-optimal but adds fixed input latency with no prediction/rollback — wrong for twitch FPS aim (Q7/Q8) | **Phase 0.5 — PASSED (D15):** avatar-local prediction makes it feel good across conditions. Tick rate (Q8) resolved D16/D21: global 60 Hz |
| **One world, two views** | The same battlefield must work top-down as an RTS map *and* at eye level as an FPS space — double the asset/collision/LoD cost | Prove one space in both views in the **Phase 1** slice before scaling content; production-side answer (sourcing, tiers, two-view filter) in [`content-pipeline.md`](content-pipeline.md) |
| **Build cost** | A custom native engine is a real investment | **Phase 1 PASSED (D22):** vertical slice validated on Galaxy S24; Unity/Godot fallback retired |
| **Determinism bugs** | Any float leaking into the sim breaks lockstep silently | Enforce fixed-point in the sim layer; per-tick checksum diffing in CI from day one |
| **Device fragmentation** | Android GPU/thermal variance is wide | Quality tiers + dynamic scaling baked in early, not as a post-ship patch |
| **Blindness feels unfair** | "World goes dark" can read as robbery if mishandled | Thin alert thread, strong audio, visceral/constant blindness feedback, fast re-entry (design doc §6) |
| **FPS-fidelity gap vs incumbents** | We share a storefront with Delta Force / CoD Mobile and *will* lose any head-to-head on raw gunfeel, animation, and texture fidelity; a shooter player who bounces in 10 s never reaches the hybrid | Don't fight on their axis — reach a written *good-enough floor* (CP-2/CP-3), consciously concede photoreal tier, and let the command+embody hybrid be the reason to stay ([`positioning.md`](positioning.md)) |
| **Compared piecemeal, not whole** | Judged feature-by-feature (our FPS vs DF, our gunsmith vs CoD, our RTS vs CoH) a focused incumbent wins every isolated column | Keep the *intersection* legible — the command×embody×vision-cost square is empty; lead there, parity elsewhere ([`positioning.md`](positioning.md) §2/§6) |
