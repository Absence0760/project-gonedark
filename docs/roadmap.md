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
> has since been deleted on Phase 1 completion (D22). Plan: [`phase-0.5-plan.md`](plans/phase-0.5-plan.md).

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
> (game systems) is signed off (D31); Phase 3 (scale & net) is the active phase and Phase 4
> (polish & ship) is opening.** Honest caveat: validated on a flagship; frame-rate/thermal
> on mid-range silicon and the 200-unit power budget are Phase 3 (D21). Detailed plan and
> sign-off record: **[`phase-1-plan.md`](plans/phase-1-plan.md)**.

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
> [playability push](plans/playability-plan.md) (D37–D40) then closed the remaining *functional* gaps
> the systems sign-off left** — embodied firing ([D37](decisions.md)), a win/lose evaluator
> ([D38](decisions.md)), the enemy commander AI ([D39](decisions.md)), and a real first-person
> world ([D40](decisions.md)), plus in-match text — without disturbing the signed-off systems;
> by-hand feel and the art pass are still owed. **The D30 balance baseline has since been
> overhauled** — D66 made combat lethal (×5 damage, ~1.5 s rifle TTK) and D67 added finite per-unit
> ammo + resupply; inter-unit balance at that lethal speed was reopened as [Q18](open-questions.md)
> and is now **resolved/landed** ([combat-rebalance plan](plans/combat-rebalance-plan.md) COMPLETE):
> D69 restored the Rifleman↔Heavy RPS (Heavy 280/90 → 300/100) and D70 added area suppression that
> pins a cluster before the kill — both harness-measured and re-pinned in `--metrics`. D72 (AI tanks
> also fire traveling projectiles) and D73 (a dedicated anti-tank infantry unit, restoring the armour
> RPS triangle) followed on the same lethal-combat arc.

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
> **[`phase-3-plan.md`](plans/phase-3-plan.md)**. Workstream A underway: a deterministic **200-unit
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
> **Four further codeable slices have now landed:** the **host-side RTT estimator** (a pure
> `engine::net_tuning` EWMA + hysteresis seam driving the agreed `Lockstep::propose_delay`), the
> **RTT sample feed** (`pal-desktop::pingpong` — `PingPongTransport`/`RttMeter`, transport-level
> ping/pong feeding `observe_rtt`; kept outside `core::lockstep` so `core` stays clock-free), the
> **detection HUD** (`render::detection` + the pure `engine::detection_markers` seam,
> invariant-#6-guarded), and the **honest AI consult** (the scripted commander chases a gone-dark
> hostile, config-gated default-OFF, only within what detection reveals — all default sim checksum
> streams verified bit-identical). What remains is **gated, not unwritten**: the dual-rate D21
> re-eval (**needs on-device thermal numbers**), the Wi-Fi↔cellular handoff (**needs a QUIC
> transport**), the two-human PvP mind game (**needs the net layer**), and rayon-into-`core`
> (deliberately deferred — unjustified at ~3.7 ms/tick and would need its own decision).

**Goal:** make it hold up at size and (if pursued) in multiplayer.

- [x] **200-unit stress tests** — deterministic `stress`/`stress:<n>` scene + `--time` timing mode;
  flow-field caching + spatial-hash acquisition took 200 units from ~30 ms to **~3.7 ms/tick median**
  on desktop; cross-arch `compare-stress` CI job.
- [x] **Deterministic lockstep netcode; input delay; per-tick checksum diffing in CI** — sans-I/O
  `core::lockstep` + wire codec + cross-client checksum agreement; RTT-adaptive `DelayChange` protocol
  + the `engine::net_tuning` estimator; UDP transport; `net-sim-runner` + `compare-net` CI
  ([D27](decisions.md)).
- [x] **Reconnect/snapshot handling** — authoritative `core::persist` serialize/deserialize sharing
  the checksum field-walk + `core::reconnect` snapshot-and-replay ([D28](decisions.md)).
- [x] **PvP attention mind-game — mechanism, HUD, honest AI consult** — the `core::detection` tell
  (`Hidden|Subtle|Marked`, default Subtle), the `render::detection` overlay, and the config-gated
  commander consult ([D33](decisions.md)).
- [ ] **Profiling on target hardware + the D21 dual-rate re-eval** — desktop numbers are in; the
  on-device thermal/sustained measurement (and thus the global-60-vs-dual-rate call) needs a physical
  mid-range device. Job-system (rayon) parallelism deliberately deferred — unjustified at ~3.7 ms/tick
  and would need its own decision (invariant #2).
- [ ] **Wi-Fi↔cellular handoff** — blocked on a QUIC `pal::Transport` (only UDP has landed); the
  reconnect policy is already transport-agnostic and ready for it.
- [ ] **Two-human PvP mind-game tuning** — needs the live net layer (sockets + matchmaking) so two
  networked humans actually face the dilemma.

## Phase 4 — Polish & ship

> **Status: OPENING (plan landed) — Phase 3 still IN PROGRESS.** Plan and workstream sequencing:
> **[`phase-4-plan.md`](plans/phase-4-plan.md)**. Per [D32](decisions.md) the out-of-match app shell is
> **native per-platform** (reached through a narrow shell↔sim seam), with the **in-session** shell
> in-engine. **The seam prerequisite has landed:** `core::shell` — a GPU-free, logic-free façade
> (intent in, presentation-safe view out; fairness structural via no `&World`), recorded in
> [D34](decisions.md). **The four buildable-now Rust workstreams have all landed** — A (seam ✅) · B
> (in-engine in-session shell ✅) · C (device tiers / dynamic-res / thermal ✅) · D (telemetry +
> consent gate ✅); full suite green dev+release. The **native out-of-match shells** are next —
> **"Boot & title" has now landed on both Android and desktop** — the **Android Compose landing
> screen** ([D35](decisions.md)) and the **desktop egui title screen** ([D36](decisions.md)), each
> the first native surface buildable once the seam landed; only the **iOS** Boot & title shell is
 still pending (no iOS target at all). **Desktop Settings is now partial** — the egui Settings /
> Profile / About screens landed with audio master/SFX volume + look sensitivity wired into the host
> ([D75](decisions.md)); **graphics-tier selection is now live** (Settings → `render::tiers` quality
> band) and **music volume now drives a real music bus** (a looping bed in the shared `pal::mix`
> mixer, desktop cpal sink; Android inherits the bed support, oboe wiring pending);
> accessibility + the rebind editor still owed. **The Android Compose out-of-match shell has since reached parity with that desktop
> shell** — Settings/Profile/About, the gunsmith, and the Operations-hub mission-select/briefing all
> re-authored in Compose ([D78](decisions.md)/[D79](decisions.md)) and a parity-gap sweep closing the
> last UI/content divergences; the product-of-record status (and the structural items still open — an
> Android campaign progress/unlock model, desktop-side shell-pref persistence) lives in
> [`plans/compose-shell-parity.md`](plans/compose-shell-parity.md) §12. The remaining surfaces stay
> pending — onboarding, match setup, lobby, store, and consent — deferred behind missing per-platform
> UI projects and the [Q5](open-questions.md)/[Q9](open-questions.md)/[Q11](open-questions.md)/Phase-3
> blockers.

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
> [D58](decisions.md)–[D61](decisions.md); execution plan: **[`pve-campaign-plan.md`](plans/pve-campaign-plan.md)**.

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
threaded through: co-op ([Q14](open-questions.md)) and narrative depth ([Q16](open-questions.md)); the
mission **authoring format** is now **resolved** (Q15 → [D76](decisions.md): RON data files behind a
host-side loader — build-out in [`content-tooling-plan.md`](plans/content-tooling-plan.md)), and the
**terrain representation** with it (Q22 → [D77](decisions.md): maps carry their own grid and `persist`
serializes a content-hash map id, so a mission's terrain travels in its data file with zero registry).

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
  point: **now landed** ([D86](decisions.md)) — `Command::SetCampRally` writes authoritative
  `Building.rally` sim state (checksum-folded, `SNAPSHOT_VERSION` 11 / `WIRE_VERSION` 10) and
  produced units inherit it as a literal-executor first `Move`; `engine::train_ui::rally_commands`
  emits it.*
- [x] **Camp upgrades** — a readable tier display + one-button level-up
  (`engine::upgrade_ui::upgrade_commands` seam; U-key on desktop, [D48](decisions.md)). *Linear
  camp-tier leveling today; a richer per-structure/per-unit prerequisite **tree** is a `core`
  follow-up (new sim state + a `Command` variant), not a presentation change.*
- [x] Resource/economy readout that makes cost and income legible at a glance
  (`render::readout::EconomyReadout` — banked credits + income rate)
- [ ] A full match a new player can complete start→finish unaided (the loop itself now
  *closes* — D64's two-base `seed_skirmish` is a live, winnable match booted by default and proven
  end-to-end in test; the remaining gap is unaided new-player onboarding/UX, not loop existence)

### Campaign & content — the first shippable product (PvE)

> Net-new content pillar ([D58](decisions.md)–[D61](decisions.md)); design in
> [`pve-campaign.md`](pve-campaign.md)/[`customization.md`](customization.md), build sequencing in
> [`pve-campaign-plan.md`](plans/pve-campaign-plan.md). WS-A, WS-D, and WS-E have landed; WS-B
> and WS-C are partial — see the plan for per-WS status. WS-A now ships **two** missions —
> *Seize* (mission 1) and the *Hold* archetype's *Hold the Line* (mission 2,
> `core::scenario::seed_hold_mission`) — and both are now **placed as nodes** in the shipped
> campaign graph: a two-node chain *Seize* → *Hold* (Hold unlocks once Seize is cleared), with the
> Android `CampaignModel` mirror moved in lock-step (see WS-B).

- [x] **Mission/objective core (WS-A)** — host-side `Objective`/`ObjectiveSet` off the `SimEvent`
  stream (generalizes [D38](decisions.md)'s `evaluate_outcome`); zero checksum surface; ships with
  `core`/`engine` tests + determinism matrix green (code landed — `engine/src/objectives.rs`,
  `core::scenario::seed_seize_mission`, `render::objective_hud`)
- [x] **Mission 1 — *Seize*** ("10 troops, take the enemy base"): the first playable mission and
  the going-dark teach beat (code landed — `core::scenario::seed_seize_mission`)
- [x] **Mission 2 — *Hold the Line*** (the *Hold* archetype — a dug-in firing line survives a
  scripted assault force for a fixed tick window) (code landed — `core::scenario::seed_hold_mission`,
  `ObjectiveSet::mission_hold`; directly playable via `Scene::Mission2`/`--scene hold`; now **placed
  as the second campaign node**, gated behind *Seize* — see WS-B)
- [ ] **Operations hub (WS-B)** — node-graph meta-progression, unlock state, mission-select +
  briefing (native shell, [D32](decisions.md)) (PARTIAL — host model `core/src/campaign.rs` +
  persistence built; the `MissionId→mission` registry has landed
  (`engine/src/mission_registry.rs`), holding both *Seize* and *Hold*; the shipped campaign graph is
  now the **two-node chain** *Seize* → *Hold* (`default_campaign()`), with the node→scene launch
  mapping (`Scene::for_mission`) wired on desktop + Android and the Android `CampaignModel` mirror
  moved in lock-step ([`compose-shell-parity.md`](plans/compose-shell-parity.md)); the egui
  mission-select/briefing hub reaches both nodes, native (Compose) shell chrome still BLOCKED on
  [D32](decisions.md))
- [x] **Gunsmith loadout (WS-C)** — fixed-point sidegrade attachment model, checksum-folded, +
  pre-match loadout UI ([D60](decisions.md)) (sim model `core/src/gunsmith.rs` + UI seam
  `engine/src/loadout_ui.rs`; the chosen loadout **is applied at live match start** — desktop
  `enter_match` + Android `android_backend` feed `Game::new_scene_with_loadout` → the
  `*_with_loadout` seeders → `Loadout::apply_to_weapon`; the GPU-free boot dispatch
  (`seed_scene_with_loadout`) is now covered by a test asserting the default path stays
  checksum-identical (invariants #1/#7))
- [x] **HUD layout editor (WS-D)** — per-layer drag/resize/opacity presets over the existing touch
  seams, presentation/input-only, invariant-#6-bounded ([D61](decisions.md); also tracked under
  *UI / UX polish* below) (code landed — `engine/src/hud_layout.rs`)
- [x] **Difficulty + modifiers (WS-E)** — deterministic `commander_orders` difficulty tier; rotating
  scenario-parameter modifiers (never balance-number hacks) (code landed —
  `core/src/mission_tuning.rs`, threaded into `core::commander`)
- [ ] **PvP fast-follow** — the multiplayer pillar on the same lockstep core (after the PvE loop is
  proven; Phase 3 netcode is the prerequisite)

### Test & feedback hardening — verify it *plays*, not just *computes*

> The deterministic-sim tooling is strong (sim-runner harnesses, the cross-arch checksum
> matrix, the unit-test floor) — but it proves the sim is *correct*, not that the game is
> *playable and readable*. The two combat bugs this cycle ("can't tell if anything's firing",
> "impossible to kill an enemy") lived in that unverified perception/input layer. Plan +
> sequencing: [`test-harness-plan.md`](plans/test-harness-plan.md). GPU-gated items stay local
> smoke tests (like `viz-runner` today), never the no-GPU CI matrix. **Execution order:** start
> with **TF-2** (don't build the combat viz on a red bar), then TF-1 → TF-4; TF-3 is independent.
> (The TF-*n* numbers map to the plan's WS-*n*, not to build order.) **Status: all four landed
> (2026-06-29)** — the viz suite now pixel-proves firing, killing, the dark-while-embodied
> fairness bar, and the hitmarker on a connecting shot; the input pipeline is covered headless.

- [x] **TF-1 — Visual combat verification.** `viz-runner` now renders two combat scenarios
  through the real `Game::frame` path: `combat_muzzle` (command-view muzzle flash draws during a
  skirmish, pixel-asserted against a clean pre-combat baseline) and `embodied_kill` (holding fire
  while embodied kills enemies, asserted on `Game::alive_unit_count`). PNGs land in `target/viz/`
  for eyeballing. GPU-gated/local. *(plan WS-1)*
- [x] **TF-2 — Fix the standing embodied-dark viz FAIL.** Root cause was a *scenario* bug,
  not a fog leak: the avatar died and the host auto-surfaced to command (invariant #5), so the
  asserted "embodied" frame was the post-ejection command view. The `viz-runner` scenario now
  re-embodies a live unit and asserts only on a genuinely-embodied combat frame (new
  `embodied_combat_frame_captured` guard); the `embodied_combat_strategic_map_stays_dark`
  fairness thresholds were **not** weakened (now 0 non-marker player-blue px). *(plan WS-2)*
- [x] **TF-3 — Input-pipeline integration tests.** Cover mouse/key → yaw → `Command::Fire`
  through the real seam (new pure `engine::embodied_input_commands`), incl. the load-bearing
  camera-forward == fire-dir "you hit what's under the crosshair" guarantee, the rightward-look
  → `−Y` convention, and crouch cone-tightening — plus desktop crouch/reload key edges. No
  GPU; ships in `cargo test` (dev + release). *(plan WS-3)*
- [x] **TF-4 — In-game hit feedback.** The "I hit him" signal the game never sent: a centered
  hitmarker "X" + a one-shot hit SFX (`SoundId::HitConfirm`), derived from the pure
  `engine::avatar_landed_hit` seam over the deterministic `SimEvent::Damaged` stream where the
  avatar is the `source` (presentation-only, invariant-#6-safe — feedback on your OWN shot, not
  intel). Pixel-asserted by the WS-1 `embodied_kill` scene (center hitmarker px ~0 → peak on a
  connecting shot). *Folds into the Game-feel polish item below.* *(plan WS-4)*

### UI / UX polish — make it read as a product

- [x] In-match command HUD, selection rim, embodied alert HUD ([D24](decisions.md)/[D26](decisions.md))
- [x] Native title screens — Android Compose ([D35](decisions.md)) + desktop egui ([D36](decisions.md))
- [~] **Visual-design pass** on the command HUD — consistent iconography, type scale, spacing,
  colour language (so it looks intentional, not greybox). **Foundation + first wave landed**
  ([D74](decisions.md): the `render::theme` palette/type/space source-of-truth + an anti-aliased font
  atlas; plus dimensional greybox lighting + a cinematic present grade, rounded-card panel chrome,
  ground detail textures, art-directed chamfered meshes, Inkscape-baked command-bar HUD icons, a
  live-backdrop landing screen, and a scripted launcher icon). **Second wave landed:** WS-C
  (command-HUD glanceability — unified state/colour language on `render::theme` + the
  `engine::panel_summary` seam), WS-D (accessibility — audio/haptic equivalents of the directional
  flash + a persisted `AlertCueMode`), and WS-E (embodied-dark tunnel-vision tonemap + detail maps +
  shell-palette unification). **CP-3 runtime skeletal playback has since landed** ([D87](decisions.md)):
  the generic trooper draws through the authored 7-bone rigid-part rig via the existing `MeshPipeline`
  in both the command and embodied passes, superseding the procedural pose. Remaining game-feel work
  (CP-2 human-feel playtest) is sequenced in the **[visual-design plan](plans/visual-design-plan.md)**.
- [ ] Touch-layout / rebind editor + correct touch-target sizing (the D14 scheme's settings surface).
  **Now scoped as the CoD-Mobile/MLBB HUD layout editor** — per-layer drag/resize/opacity presets,
  presentation/input-only, invariant-#6-bounded ([D61](decisions.md); PvE pillar WS-D)
- [ ] Onboarding / first-possession tutorial (teach the going-dark cost — invariant #6 lives here).
  **Lives in campaign mission 1** — the *Seize* "10 troops, take the base" beat scripts the
  overstay temptation ([`pve-campaign.md`](pve-campaign.md) §3; PvE pillar WS-A)
- [x] In-session shell — pause, surrender/leave, post-match summary. **Fully landed:** the
  post-match summary surface + its DISMISS button → leave-match → return-to-title transition
  ([D52](decisions.md)); pause overlay (Esc on desktop ([D53](decisions.md)), back-gesture on
  Android ([D54](decisions.md))) + in-match surrender are now wired ([phase-4-plan WS-B](plans/phase-4-plan.md)).
- [~] Settings — graphics tier, audio-mix levels, rebinds, **accessibility** (an equivalent
  cue for the directional-flash + audio alert channel). **Accessibility cue landed** (WS-D: a
  persisted `AlertCueMode` selecting audio/haptic equivalents of the flash + the existing colourblind
  ramps/shape glyphs); **graphics-tier selection now drives `render::tiers` live and music volume
  drives a real looping music bus** in the shared `pal::mix` mixer (desktop cpal sink; Android oboe
  wiring pending) ([D75](decisions.md) follow-up); **the desktop key-rebind editor now landed**
  ([D90](decisions.md)) — a pure winit-free `engine::keybind` seam (defaults / conflict-rejecting
  rebind / ordinal persistence) with a click-to-arm egui "KEY BINDINGS" section, covering the
  `app`-owned host toggles (pause/fullscreen/debug); rebinding the `pal-desktop` **gameplay** keymap
  is deferred to [Q27](open-questions.md), and the *touch*-layout editor is the separate item below.
- [ ] Game-feel polish — build/select/hit SFX + VFX, button states, screen transitions.
  **Hit feedback (the embodied "I hit him" cue) is tracked as TF-4** under *Test & feedback
  hardening* above ([`test-harness-plan.md`](plans/test-harness-plan.md) WS-4)

### Art & assets — AI-generated placeholders (skip custom 3D for now)

- [x] **Adopt AI-generated placeholder models** for units, structures, and the embodied
  weapon instead of commissioned art ([D41](decisions.md)) — this pulls the "AI-assisted"
  route that [`content-pipeline.md`](content-pipeline.md) §2 reserved for *hero* art forward
  to *everything*, sitting at the greybox/low tier of the production ladder. **Done:** eleven
  procedural greybox models, all now drawn — the trooper (also backing the D65 Medic kind), the tank
  hull + turret ([D55](decisions.md), backing the dedicated Tank kind of [D65](decisions.md)), the
  camp, the first-person weapon, the tracer, and the scenery/cover props (crate/tree/rock/barricade)
  ([D50](decisions.md))
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
- [x] **Mesh fidelity pass across the roster** — lift the remaining greybox models to the
  trooper's bar, ranked by how close the player gets: weapon viewmodels → tanks → structures →
  scenery. Per-*subject* technique (box-stacking stays right for the mechanical/architectural
  models — the lever is booleans + tuned bevels, not skinning) driven by a tight render→verify
  loop. The **trooper reskin** (an organic body via skeleton + Blender Skin modifier, commit
  `d7cced1`) is the landed pilot that proved the method. **All tiers landed** — tiers 1–3 (weapon
  viewmodels, tanks, `camp_hq`/`turret`/`barricade`) plus **tier 4 (scenery lift — crate/tree/rock)
  and the US/FR turret emplacement variants** (`gen_models.py`, cook→LOD chain + manifest sha256,
  golden mesh tests green). The `turret_us`/`turret_fr` assets are **now render-wired** —
  `ModelKind::TurretUs`/`TurretFr` + the pure `structure_turret_for(army)` selector, drawn as the two
  opposing armies' fortified-point emplacements in the embodied prop layout. *Scoped as **WS-F** of
  the [visual-design plan](plans/visual-design-plan.md).*

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
> **[`positioning.md`](positioning/positioning.md)**. The strategy in one line: **we lead on the four
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
> *enjoys* the core); CP-1 + CP-4 + CP-9 are launch-important (table-stakes for the shooter
> *and* the command audiences); CP-3/CP-5/CP-6/CP-8 + PC-5 ramp after the PvE loop is proven.
> The **LEAD** rows need *protection* (hold the determinism gates + the one-player-both-jobs
> symmetry), not new work.
>
> **Completeness (reconciled against the positioning scorecards).** Every scored capability in
> all three positioning docs — [`positioning.md`](positioning/positioning.md) §6,
> [`positioning-pc.md`](positioning/positioning-pc.md) §7,
> [`positioning-cross-platform.md`](positioning/positioning-cross-platform.md) §6 — maps to an
> item below, to a phase/PvE-pillar item above, or to the **LEAD-protection** callout at the end
> of this section. So **finishing this parity section + the phases + the PvE pillar reaches
> parity-or-better on every row the incumbents are scored on** (photoreal fidelity and BF-scale
> combined-arms spectacle are the two *consciously conceded* tiers — CP-3 and the LEAD-adjacent
> "but it's *your* battle" — not gaps to close). The conceded **roster/combined-arms breadth**
> (more unit/vehicle variety toward the "growing" spectacle row) rides the PvE content pillar +
> the scripted asset pipeline ([`content-pipeline.md`](content-pipeline.md)) — and is now also
> carried by the **factions pillar** ([D68](decisions.md), [`factions.md`](factions.md),
> [`factions-plan.md`](plans/factions-plan.md), [Q19](open-questions.md) RESOLVED via
> [D71](decisions.md)): real-army asymmetric rosters (US Army vs French Army) are a distinct,
> fairness-bounded workstream layered over `UnitKind`. Its WS-0 gate (the
> [combat rebalance](plans/combat-rebalance-plan.md), [Q18](open-questions.md)) has **cleared**, and
> **WS-A–E have all landed** (the `Army` tag + persist/lockstep codecs, per-faction rosters tilted on
> logistics rhythm not gun stats per [D71](decisions.md), per-faction cosmetic identity, the
> `core::shell` army-select seam, and per-faction gunsmith pools) — the **one remaining item** is the
> native army-select *screen*, blocked on the same D32 native-shell gap as the rest of the app shell.

- [x] **CP-1 — Gunsmith to mobile-expected depth.** Extend the WS-C sidegrade model
  ([D60](decisions.md)) to the attachment-category breadth a CoD-Mobile player expects (optics,
  barrel, stock, mag, grip, muzzle) — **horizontal only** (sidegrades, fixed-point,
  checksum-folded; never vertical power). *Builds on PvE WS-C.* **Design + fork resolved
  ([D85](decisions.md)):** Stock + Muzzle become sim slots (two new disjoint fixed-point axis pairs —
  move-speed↔aim-cone, suppression↔falloff), Grip is cosmetic-only (recoil is presentation, #4).
  **Landed ([D85](decisions.md) implemented):** the four new zero-default `Fixed` axes fold after
  `shell` (byte-neutral fast path preserved), `GunsmithPool`/`pool_for` + `loadout_ui` gained all six
  rows, and the fairness proof generalized to the full `3⁵ = 243`-build space per army — 2-peer
  checksum agreement + the D69/D70 Rifleman↔Heavy RPS re-validated green. All six categories now
  exist as sidegrades (Grip cosmetic-only). *The chosen loadout is now applied at live match start on
  both platforms, with the boot dispatch test-locked (PvE WS-C).*
- [ ] **CP-2 — Embodied game-feel bar (launch-critical).** A focused gunplay pass so a Delta
  Force player doesn't bounce in ten seconds: hit feedback (impact/hitmarker/damage-direction),
  recoil/kick readability, responsive ADS, audio-coupled firing. **Presentation/feel only — never
  sim state (#4).** Define a written "good-enough floor" and playtest against it. *Scoped as WS-A of
  the [visual-design plan](plans/visual-design-plan.md).*
- [ ] **CP-3 — Animation/fidelity floor (conceded tier).** A "not jarring" floor — coherent
  locomotion/fire/death anims on the greybox so the eye-level view reads as a *place* — via the
  scripted pipeline ([`content-pipeline.md`](content-pipeline.md), [D41](decisions.md)/[D46](decisions.md)).
  **Explicitly not UE5 parity**; we concede photoreal fidelity and compete on the hybrid. *Scoped as
  WS-B (animation) + WS-F (mesh fidelity) of the [visual-design plan](plans/visual-design-plan.md).*
  **WS-B landed ([D84](decisions.md) + [D87](decisions.md)):** the clip-selection seam + a procedural
  pose + rig authoring (`trooper_rig.glb`, 4 clips), and **runtime skeletal playback now drives the
  generic trooper** through the authored rigid-part rig via the existing `MeshPipeline`. **WS-F mesh
  fidelity is also complete** (all four tiers + US/FR turret variants). Remaining CP-3 work is
  per-faction rigs + runtime-driven death anim.
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
- [ ] **CP-9 — Command-layer readability + teach-fast pass.** The closing item for the PAR-ish
  *Strategic/command depth* row ([`positioning.md`](positioning/positioning.md) §6): the RTS half
  must read **at a glance on a small screen** and **teach itself fast** — a shooter-first audience
  won't learn it slowly, and a *Company of Heroes*/*StarCraft* veteran must respect it. This is the
  *information architecture + glanceability* of selection / orders / economy / territory (what the
  player can parse and act on in a second), **broader than and paired with** the *Visual-design pass
  on the command HUD* under UI/UX polish above (which is the icon/type/colour layer). Bounded by
  invariant #3 (depth lives in the **order/stance vocabulary**, never smarter unit AI) and #6 (no
  strategic intel leaks while embodied). *Launch-important — it gates whether the command half lands
  for the shooter-first audience the storefront sends us. Scoped as WS-C of the
  [visual-design plan](plans/visual-design-plan.md).*

**PC-facing parity** — meeting a seated, genre-literate player's expectations without forking the
game (full analysis: [`positioning-pc.md`](positioning/positioning-pc.md)):

- [ ] **PC-1 — Mouse-and-keyboard combat feel.** The embodied layer must feel right with a *mouse*,
  not just thumbs — precise aim, sensible defaults, FOV control. PC players notice instantly.
  (Pairs with CP-2.)
- [ ] **PC-2 — PC control & options surface.** Full rebinds, graphics options, ultrawide /
  high-refresh / high-DPI support — the settings depth a PC player expects.
- [~] **PC-3 — Replays & spectating (a determinism freebie).** A match is a seed + an input log
  (invariant #1), so replay + spectator view are *cheap* and a real PC / e-sports differentiator.
  **Foundation landed ([D89](decisions.md)):** the headless `replay-runner` crate records a scenario's
  seed + per-tick `Command` log and plays it back **bit-identical** by checksum (`pnpm desktop:replay`),
  the byte codec host-side so `core` stays serde-free. **Remaining:** multi-peer replay ordering and a
  *rendered* spectator view (both ride this same seed+log foundation); replay cross-version
  compatibility is [Q26](open-questions.md).
- [~] **PC-4 — Mods / data-driven content.** Missions/scenarios become external **RON data files**
  ([D76](decisions.md), [`content-tooling-plan.md`](plans/content-tooling-plan.md)) and maps carry
  their own content-addressed terrain ([D77](decisions.md)); exposing them as moddable content is how
  StarCraft/Total War lasted decades — a PC-only longevity lever. **Loader landed ([D91](decisions.md)):**
  `engine::mission_format`/`map_format` (float-airlock RON → `MissionSpec`/`MapSpec`), the objective
  archetype vocab, a content-lint harness, and a seed-deterministic procedural map generator. **Owed:**
  CT-D (a content-directory registry + between-match hot-reload) to make it moddable without a recompile.
- [ ] **PC-5 — RTS depth & mastery proof.** The closing item for the *RTS skill ceiling / mastery*
  row the PC scoreboard concedes as LAG "until we prove it's real" ([`positioning-pc.md`](positioning/positioning-pc.md)
  §7): the **order/stance vocabulary** + the **"when do I dare go dark" timing** must form a genuine,
  *masterable* skill curve a StarCraft/CoH veteran respects — a different skill from raw APM, but a
  real one. Validate by **structured playtests** plus the `--metrics` harness as a supporting signal
  (decision depth shows up as outcome variance, not coin-flips), and a higher-tier honest commander
  ([D39](decisions.md)) that punishes shallow play. Depth stays in the **vocabulary**, never in
  autonomous unit AI (invariant #3). **Pairs with CP-9** — a skill must be legible before it can be
  mastered. *Ramps after the PvE loop proves the core skill is fun.*

**Cross-platform parity** — keeping *one game* fair and coherent across phone/PC/console (full
analysis: [`positioning-cross-platform.md`](positioning/positioning-cross-platform.md)):

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
> commander-vs-grunts split that killed every prior hybrid ([`positioning.md`](positioning/positioning.md) §3).
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
| **FPS-fidelity gap vs incumbents** | We share a storefront with Delta Force / CoD Mobile and *will* lose any head-to-head on raw gunfeel, animation, and texture fidelity; a shooter player who bounces in 10 s never reaches the hybrid | Don't fight on their axis — reach a written *good-enough floor* (CP-2/CP-3), consciously concede photoreal tier, and let the command+embody hybrid be the reason to stay ([`positioning.md`](positioning/positioning.md)) |
| **Compared piecemeal, not whole** | Judged feature-by-feature (our FPS vs DF, our gunsmith vs CoD, our RTS vs CoH) a focused incumbent wins every isolated column | Keep the *intersection* legible — the command×embody×vision-cost square is empty; lead there, parity elsewhere ([`positioning.md`](positioning/positioning.md) §2/§6) |
