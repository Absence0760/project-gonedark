# Going Dark *(working title)*

A mobile-first **RTS / FPS hybrid**. Command and grow your camps from above like
*Company of Heroes* — economy, territory, army-building, cover-and-suppression
tactics — then **drop into a single tank or trooper and fight in first person**.
The catch: while you're embodied, *the world goes dark*. You lose all sight of
the battlefield except what your unit can see. Stay in as long as you dare.

This repo holds the design, architecture, and roadmap, and — as of Phase 1 — the
**Rust engine workspace** (validated end-to-end on real arm64, D22; see Status). The
Phase 0/0.5 throwaway Godot prototypes that proved touch-feel and netcode feel have
been deleted on Phase 1 completion.

---

## The pitch in one line

> You are always the commander. Embodiment is a lens you put on — powerful, and
> blinding. The skill isn't whose AI plays better; it's *yours*: how well you set
> your army up before you dive, and whether you can read the board well enough to
> know when it's safe to go dark.

## The core loop

1. **Command** (top-down RTS) — build and upgrade camps, manage economy, train an
   army, capture territory, issue orders and stances to your units.
2. **Embody** (FPS) — possess any one of your living units. Your skill with that
   tank or soldier is now in play: precise aim, peeking cover, clutch moments the
   AI literally cannot do.
3. **Go dark** — the moment you embody, the strategic map blacks out. You see only
   what your unit sees. Thin alerts ("base under attack") are your one thread back.
4. **Surface** — pull out when you dare, or die and get ejected back to command.
   Re-read the changed board fast, re-issue orders, pick your next moment.

## What makes it different

Most RTS/FPS hybrids either split the two roles across different players
(*Eximius*, *Natural Selection 2*) or make the unit AI smart enough that leaving
your army alone is safe (which quietly lets the game play itself). **Going Dark
does neither.** One player does both jobs, the AI is a deliberately *literal*
order-executor, and embodiment costs you your sight. That turns information into
the game's real currency and makes "can I afford to be blind right now?" the
central, skill-based decision.

See [`docs/game-design.md`](docs/game-design.md) for the full design and
[`docs/decisions.md`](docs/decisions.md) for *why* each rule is the way it is.

## Repo layout

| Path | What's in it |
|---|---|
| [`docs/game-design.md`](docs/game-design.md) | The game design doc — concept, mechanics, the going-dark rule, unit AI philosophy |
| [`docs/positioning/positioning.md`](docs/positioning/positioning.md) | Competitive positioning (overview + **mobile / storefront** fight) — vs. Delta Force / CoD Mobile, the FPS/RTS-hybrid graveyard, and the CoH lineage; the moat, the honest exposure, the feature scorecard, and the **CP-n** parity plan |
| [`docs/positioning/positioning-pc.md`](docs/positioning/positioning-pc.md) | Positioning on **PC** — vs. Company of Heroes 3 / StarCraft / Total War (command), Call of Duty / Battlefield 6 / Halo (shooting), Destiny 2 (longevity), and the Hell Let Loose / Squad "so close" hybrids; the **PC-n** parity items |
| [`docs/positioning/positioning-cross-platform.md`](docs/positioning/positioning-cross-platform.md) | Positioning **across platforms** — one game everywhere (vs. Fortnite / Warzone / Genshin); why the deterministic core makes us cross-play-native, and the thumb-vs-mouse fairness problem ([Q17](docs/open-questions.md)); the **XP-n** parity items |
| [`docs/architecture.md`](docs/architecture.md) | Engine & systems architecture (native core, deterministic sim, Vulkan, netcode) |
| [`docs/platforms.md`](docs/platforms.md) | Cross-platform plan — Windows/Linux/Android/iOS, one shared core with platform-optimized backends |
| [`docs/content-pipeline.md`](docs/content-pipeline.md) | Asset production — quality tiers, open-source sourcing, license hygiene, the two-view filter, what Claude can/can't build |
| [`docs/maps.md`](docs/maps.md) | Real-world battlefield maps — the GIS ingest→bake→lint pipeline ([D80](docs/decisions.md)), the two-artifact split (integer sim cover grid vs. float render mesh), faithful-then-balance-pass, and map diagnostics (headless linter + in-engine cover overlay / `MapInspect`) |
| [`docs/pve-campaign.md`](docs/pve-campaign.md) | The PvE pillar — the Operations-hub campaign, mission archetypes, the host-side objective system, honest-AI difficulty ([D58](docs/decisions.md)/[D59](docs/decisions.md); first shippable product) |
| [`docs/customization.md`](docs/customization.md) | Customization — the horizontal gunsmith (fixed-point sidegrades), cosmetics (presentation-only), the mobile HUD layout editor ([D60](docs/decisions.md)/[D61](docs/decisions.md)) |
| [`docs/factions.md`](docs/factions.md) | Factions — real-army asymmetry, **US Army vs French Army** ([D68](docs/decisions.md)/[D71](docs/decisions.md)); the fairness-bounded roster model layered over `UnitKind`; WS-A–E built (army-select screen D32-blocked) |
| [`docs/infrastructure.md`](docs/infrastructure.md) | Local dev (clone-and-run via Docker), config/env files, Terraform infra, sops secrets |
| [`docs/roadmap.md`](docs/roadmap.md) | Build phases, milestones, and the top risks |
| [`docs/plans/phase-0.5-plan.md`](docs/plans/phase-0.5-plan.md) | Plan + record of the embodiment-over-network latency spike (resolved Q7/Q8 → D15/D16) |
| [`docs/plans/phase-1-plan.md`](docs/plans/phase-1-plan.md) | Detailed plan + sign-off record for the Phase 1 Rust engine vertical slice (DONE — exit criterion met, Galaxy S24, D22) |
| [`docs/plans/phase-3-plan.md`](docs/plans/phase-3-plan.md) | Phase 3 (Scale & net) plan — four-workstream sequencing (perf/lockstep/snapshot/PvP), per-slice sign-off in progress |
| [`docs/plans/phase-4-plan.md`](docs/plans/phase-4-plan.md) | Phase 4 (Polish & ship) plan — app-shell workstreams (seam ✅/in-session shell ✅/device tiers ✅/telemetry ✅); Boot & title landed on Android (D35) and desktop (D36); remaining surfaces pending |
| [`docs/plans/compose-shell-parity.md`](docs/plans/compose-shell-parity.md) | Compose shell parity plan (SUBSTANTIALLY COMPLETE — [D78](docs/decisions.md)/[D79](docs/decisions.md)) — brought Android's out-of-match Compose shell up to the desktop egui shell: the Compose→`NativeActivity` launch-config seam (Tier 0), Settings/Profile/About + title mode-split (Tier 1), gunsmith + campaign mission-select (Tier 2), and a §12 parity-gap sweep; structural items (Android campaign-progress model, desktop shell-pref persistence) still open, PvP/lobby/store/consent still blocked |
| [`docs/plans/playability-plan.md`](docs/plans/playability-plan.md) | Playability push (LANDED — D37–D40) — six parallel-worker workstreams that made the game *play* and *read* like a game: embodied combat, win/lose, enemy AI, in-match text, embodied FPS world, command-view polish |
| [`docs/plans/pve-campaign-plan.md`](docs/plans/pve-campaign-plan.md) | PvE campaign plan (IN PROGRESS — D58–D61) — the first shippable product: WS-A (objectives/seize mission), WS-D (HUD layout editor), WS-E (difficulty/modifiers) landed; WS-C (gunsmith) partial; WS-B (Operations hub) host model built, native shell D32-blocked |
| [`docs/plans/tank-embodiment-plan.md`](docs/plans/tank-embodiment-plan.md) | Tank-embodiment plan (IN PROGRESS — D55) — War Thunder-flavoured embodied tank: independent hull/turret, all-unit armour facing, dispersion gunnery; fixed-point/lockstep phasing P1–P9 (P1–P8 landed: trig slew math, hull/turret heading, ballistic projectile pool, all-unit armour facing, turret mesh + tracers, dispersion bloom, ShellKind, HUD; P9 partial) |
| [`docs/plans/combat-rebalance-plan.md`](docs/plans/combat-rebalance-plan.md) | Combat rebalance plan ([Q18](docs/open-questions.md) **closed**) — restored the Rifleman/Heavy rock-paper-scissors (WS-A/[D69](docs/decisions.md): Heavy 280→300 HP, 90→100 dmg) and made suppression bite at lethal speed (WS-B/[D70](docs/decisions.md): area suppression + pin 1/2→3/8); the now-satisfied prerequisite for the factions build |
| [`docs/plans/factions-plan.md`](docs/plans/factions-plan.md) | Factions plan (IN PROGRESS — [D68](docs/decisions.md)/[D71](docs/decisions.md)) — **US Army vs French Army**: WS-A/B/C/E all built; WS-D seam + PvE scenario seeding + army-tilted troops built; only the native army-select screen remains (D32-blocked) |
| [`docs/plans/test-harness-plan.md`](docs/plans/test-harness-plan.md) | Test & feedback hardening plan (COMPLETE — 2026-06-29) — four workstreams to close the gap between a correct sim and a *playable, readable* one: WS-1 combat viz (viz-runner extended to render/pixel-assert embodied firing + kills), WS-2 fix the standing embodied-dark viz FAIL, WS-3 input-pipeline integration tests, WS-4 in-game hit feedback; roadmap items TF-1..TF-4 |
| [`docs/plans/content-tooling-plan.md`](docs/plans/content-tooling-plan.md) | Content-tooling plan (PLANNED — [Q15](docs/open-questions.md) **closed** → [D76](docs/decisions.md)) — authoring infrastructure for *extensive* campaigns & battlefields: missions/maps become external **RON data files** behind a host-side `engine` loader (the float airlock) driving a serde-free `core::scenario::ScenarioBuilder`; CT-A builder, CT-B mission format, CT-C `*.map.ron` battlefields, CT-D data-backed registry + hot-reload, CT-E archetype vocabulary, CT-F content-lint CI, CT-G procedural map generator + PvP-symmetry validator (content-addressed terrain, [D77](docs/decisions.md)) |
| [`docs/plans/visual-design-plan.md`](docs/plans/visual-design-plan.md) | Visual-design plan (IN PROGRESS) — push the look & feel toward near-final: WS-0 foundation landed ([D74](docs/decisions.md) theme + font atlas; lighting/present-grade, panel cards, ground textures, art-directed meshes, Inkscape HUD icons, landing + launcher icon); remaining WS-A game-feel (CP-2), WS-B animation floor (CP-3), WS-C command readability (CP-9), WS-D accessibility cues (#6), WS-E world depth — presentation-only |
| [`docs/decisions.md`](docs/decisions.md) | Decision log — the choices we locked in and the reasoning |
| [`docs/open-questions.md`](docs/open-questions.md) | Unresolved design forks still on the table |
| `prototypes/` *(deleted)* | The two throwaway Godot prototypes (`phase0-controls/` → D14, `phase0.5-netfeel/` → D15) proved touch-feel and embodied netcode feel; deleted on Phase 1 completion (D22). Not the engine |
| `Cargo.toml` + `core/ pal/ render/ engine/ pal-desktop/ pal-android/ app/ sim-runner/ net-sim-runner/ viz-runner/ server/` | **The Rust engine workspace.** `core` = deterministic fixed-point sim (zero platform deps) — a real flow field, the Phase 2 game-systems modules (`combat`/`terrain` cover+LoS, `territory`, `economy`/camps (Camp + Barracks; Rifleman/Heavy/Tank/Medic with production routing, [D65](docs/decisions.md)), `heal` (Medic heal-over-time, [D65](docs/decisions.md)), `resupply` (all-unit ammo rearm at a friendly Camp/Barracks — the logistics half of finite carried ammo, [D67](docs/decisions.md)), `fog`, `orders`, `alerts`, `event`, `dispersion` (tank aim-bloom — settle-to-zero scatter, [D55](docs/decisions.md))), the scripted enemy `commander` that issues player-equivalent orders through the normal command path while units stay literal executors ([D39](docs/decisions.md)), the PvE campaign host model `campaign` (Operations-hub node-graph, `core::shell`-reachable) + `mission_tuning` (deterministic difficulty tier threaded into `commander_orders`, [D58](docs/decisions.md)–[D61](docs/decisions.md)), the deterministic `rng` + fixed-point `trig` (LUT) + render-state `snapshot` substrate, the sans-I/O `lockstep` command-exchange loop + wire codec with runtime cross-client checksum-agreement (peers exchange per-tick checksums to catch a live desync, [D27](docs/decisions.md)), and the authoritative-snapshot `persist` serializer (`Sim::serialize/deserialize` sharing the checksum field-walk, terrain by map-id, for reconnect/resume — [D28](docs/decisions.md)) plus the `reconnect` resume policy (snapshot + buffered-command replay) and RTT-adaptive lockstep delay (the agreed `DelayChange` protocol, Phase 3 B7), a deterministic `spatial` hash backing near-O(1) target acquisition (bit-identical to the old brute-force scan, Phase 3 A5), and the **checksum-excluded** `detection` "gone-dark" tell (a pure derivation like `fog`/`alerts`: `Hidden|Subtle|Marked`, default Subtle — [D33](docs/decisions.md)), all fixed-point (sim-state modules checksum-folded; `fog`/`alerts`/`detection` are excluded derivations) ([D23](docs/decisions.md)), plus the **GPU-free, logic-free** `shell`↔sim seam — the `core::shell` façade (intent in, presentation-safe view out: match lifecycle/`MatchSummary`, the order-stance vocabulary as data, lockstep `ConnectionStatus`, and the embodiment-fair `InSessionView` that takes already-derived fog/alerts/tells, never `&World`) every app shell reaches `core` through, native or in-engine ([D32](docs/decisions.md)/[D34](docs/decisions.md)); and `scenario` — named deterministic scene seeders: the debug sandboxes `seed_duel` (two-tank hitbox duel) + `seed_infantry` (hitscan range/cone/cover/LoS sandbox), and the first **real playable match** `seed_skirmish` (two operational bases, one starting troop each, three neutral capture posts; small scenario-local purse, D30 balance untouched) — consumed by the headless runners and `engine::Game::new_scene` (dispatched via `app --scene <name>`; the desktop host boots `skirmish` by default); `pal` = platform traits (incl. the touch-UI input intents, the `Audio` mix seam, [D24](docs/decisions.md), and the `Transport` opaque-byte-frame netcode seam, [D27](docs/decisions.md)) plus `pal::mix` — the one shared, host-tested audio *render* math (pan/gain/muffle/sum) every backend mixes through ([D29](docs/decisions.md)); `render` = real `wgpu` instanced renderer with fog-of-war filtering, the embodied directional alert HUD (`fog`/`hud`, [D24](docs/decisions.md)), `tank_hud` (hull-relative turret indicator + dispersion reticle + LEAD pip + shell selector + reload ring, [D55](docs/decisions.md)), `objective_hud` (in-match mission objective progress), a white selection rim ([D26](docs/decisions.md)), a cooked greybox-mesh loader (`render::mesh` + `mesh.wgsl`, [D44](docs/decisions.md)) that `include_bytes!`s the Blender-cooked `.mesh` (GDM1) files, uploads them to GPU, and draws them through one shared depth-tested 3D pass — the embodied first-person **weapon viewmodel** and the command-view **3D unit/structure tokens** (with the 2D health/selection/ring quads layered on top as UI decals), and the Android-only on-screen FPS touch HUD screen-space pass (`render::touch_controls`, [D51](docs/decisions.md)), a selection-contextual command panel (`render::command_panel`, [D62](docs/decisions.md)), a command-view gone-dark tell overlay (`render::detection` + `detection.wgsl`, [D33](docs/decisions.md)), and a dev debug overlay (`render::debug` + `debug.wgsl`) drawn command-view-only behind `Game::debug_hitboxes` (F3, on in debug scenes) — tanks get armour-facet hitbox rings + tracers, infantry get range-ring + firing-cone wedge, all units get Player→Enemy LoS connectors + a muzzle-flash marker when firing — as a pure presentation derivation, never the dark embodied frame (invariant #6); `engine` = the platform-agnostic game loop (sim+render+fixed-tick+cameras+command/embodiment) that both hosts drive ([D20](docs/decisions.md)), also driving the embodied audio mix, unit selection, the order/stance command vocabulary (`audio`/`selection`/`command_ui`, [D24](docs/decisions.md)), embodied locomotion + hitscan firing (`locomote`/`fire`, [D50](docs/decisions.md)/[D51](docs/decisions.md)), the command-view production-panel seams (`build_ui`/`train_ui`/`upgrade_ui`, [D42](docs/decisions.md)–[D48](docs/decisions.md)), `objectives` (host-side `Objective`/`ObjectiveSet` evaluated off the `SimEvent` stream), `hud_layout` (per-layer drag/resize/opacity HUD layout editor, [D61](docs/decisions.md)), the in-engine `session_shell` ([D34](docs/decisions.md)), balance `tuning` ([D30](docs/decisions.md)), the RTT-estimator host seam (`net_tuning` — feeds `Lockstep::propose_delay` via `Game::observe_rtt`, [D27](docs/decisions.md)), and the pure host-tested touch seam that maps raw `InputFrame.touches` to embodied intents + HUD geometry (`touch_controls`, [D51](docs/decisions.md)), the selection-contextual command panel derivation (`command_panel_view`, [D62](docs/decisions.md)), and the scene-dispatch entry (`new_scene`/`debug_hitboxes` — `app --scene <name>` routes to `core::scenario` seeders; F3 toggles the per-scene debug overlay, on by default in the duel and infantry scenes), with the fixed-tick loop now sourcing each tick's command set through `core::lockstep` (single-player = a 1-peer delay-0 session, [D27](docs/decisions.md)); `pal-desktop` = real `winit`+`wgpu` backend with optional `cpal` audio output (opt-in `audio` feature → `pnpm play:audio`, [D26](docs/decisions.md)) plus `pal::Transport` backends — an in-process `LoopbackTransport` for dev/test, a real-socket `UdpTransport` (UDP now, QUIC the documented future, [D27](docs/decisions.md)), and a `PingPongTransport` / `RttMeter` RTT sampler (`pingpong`) that multiplexes over any inner transport to feed `engine::net_tuning`; `app` = thin winit desktop host; `pal-android` = JNI/cargo-ndk backend whose `android_main` drives the same `engine` loop (builds an arm64 APK), with a real low-latency **AAudio** sink via `oboe` ([D29](docs/decisions.md)); `sim-runner` = headless single-client checksum driver, now also home to the deterministic **balance-metrics harness** (`--metrics[=open-duel|cover-duel|equal-cost|economy|summary]`: time-to-kill / equal-cost-trade / suppression-pin / economy-ramp series to **stderr**, stdout checksum stream untouched — the objective signal the [D30](docs/decisions.md) combat/economy re-tune was measured against), and a `duel` scene mode (headless hitbox-validation harness driven by `core::scenario::seed_duel`) and an `infantry` scene mode (hitscan scene driven by `core::scenario::seed_infantry`); `net-sim-runner` = headless **2-peer lockstep** checksum driver — runs both `core::lockstep` peers in-process over a seeded channel, asserts they agree + match a no-network reference, emits the agreed stream for the ADD-ONLY `compare-net` CI job (`pnpm desktop:sim:net`, [D27](docs/decisions.md)); `viz-runner` = headless **offscreen render** smoke test (renders `Game` to a texture, reads pixels back, asserts the command view draws + embodiment goes dark + the alert HUD draws, and writes PNGs — `pnpm desktop:viz`, needs a GPU so it's local-only, not CI); `server` = the Phase 4 backend scaffolding (WS-D) — telemetry ingest, a consent gate, and live-ops endpoints over an `axum` HTTP listener (`telemetry`/`consent`/`liveops`/`http`); pre-production, not yet wired to real infra. See [`docs/plans/phase-1-plan.md`](docs/plans/phase-1-plan.md) |
| `android/` | Gradle project that packages `pal-android` (via `cargo-ndk`) into the arm64 APK the Phase 1 device run was built from |
| `assets/` + `tools/` | The scripted asset pipeline — generator scripts in `tools/models/` (Blender/gltfpack, [D41](docs/decisions.md)/[D44](docs/decisions.md)/[D46](docs/decisions.md)) producing the cooked greybox meshes + `assets/models/manifest.json` (`source`/`license`/`sha256` per tier). Generator scripts are committed, not opaque binaries |
| `scripts/` | Dev/CI shell helpers behind the `pnpm` targets (`android.sh`, `android-checksum.sh`, `help.sh`) |

## Status

**Phase 1 — DONE (D22). Phase 2 (game systems) — SIGNED OFF systems-complete (D31). Phase 3
(scale & net) — lockstep + cross-client checksum agreement, `core::persist`/`reconnect`, and the
`spatial` index have landed. A playability push (D37–D40) turned the systems into an end-to-end
playable loop (command-and-grow UI, embodied FPS combat, win/lose, a scripted enemy commander),
and Phase 4 app-shell work is in flight (boot/title D35/D36, server telemetry/consent/live-ops
scaffolding, in-session + post-match shell); embodied FPS controls, 3D greybox assets, and
avatar-visible unit rendering run through D52.** Phase 0 (D14) and Phase 0.5
(D15) both passed (2026-06-23): touch-feel and embodied-combat-over-lockstep risks retired.
**Phase 1 exit criterion met on Galaxy S24, Adreno 750:** `pnpm android:checksum` confirmed
the device sim-runner checksum stream **bit-identical** to desktop over 300 ticks
(`4c34c6b5951edf57`); the `adb logcat` FPS heartbeat showed **120 fps** sustained at the locked
**60 Hz** sim tick — demonstrating sim/render decoupling (invariant #4) live on hardware. One
unit moves via a real deterministic flow field; tap-to-move works; the two-finger embody toggle
flips the world dark. The Rust engine workspace carries: a deterministic fixed-point `core`
(Q16.16 [D17](docs/decisions.md), hand-rolled SoA ECS [D18](docs/decisions.md)), the PAL trait
boundary, a real `wgpu` 29 + `winit` 0.30 renderer + `pal-desktop`/`pal-android` backends
([D19](docs/decisions.md)), and the shared `engine::Game` loop ([D20](docs/decisions.md)) that
both hosts drive. All three decide-first gates locked — sim rate closed by
[D21](docs/decisions.md): **global 60 Hz** (`core::sim::TICK_HZ = 60`; dual-rate deferred to
Phase 3). The **Unity/Godot fallback ([D8](docs/decisions.md)) is retired**; the custom Rust
engine is committed. **Honest caveat:** validated on a flagship; frame-rate/thermal on mid-range
silicon and the 200-unit power budget are Phase 3.

Target platforms: **Windows, Linux, Android, iOS** — one shared deterministic core with
platform-optimized backends (D3D12/Vulkan, Vulkan, Vulkan, Metal), developed on Linux desktop
first and shipping Android-first. See [`docs/platforms.md`](docs/platforms.md). Engine:
**custom native in Rust** (renderer via `wgpu`) — see [`docs/decisions.md`](docs/decisions.md)
D10 for the reasoning.

## Local development

A fresh clone runs against local Docker services with committed, non-secret defaults —
no cloud access or secrets needed:

```
docker compose up -d        # Postgres + Redis (backend deps)
cargo run                   # loads .env.development   (once engine code exists)
```

Production secrets are KMS-encrypted (sops) in the separate private estate repo
(`~/github/infra-secrets/gonedark/`, **not** in this repo — see D12) and cloud infra is
Terraform in `infra/` — neither is touched for local work. Full details in
[`docs/infrastructure.md`](docs/infrastructure.md).
