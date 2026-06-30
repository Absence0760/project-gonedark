# Phase 4 plan — Polish & ship

> **Status: OPENING — Phase 3 still IN PROGRESS.** Phase 1 (vertical slice, D22) and Phase 2
> (game systems, D23–D26/D29–D30, signed off D31) are done; **Phase 3 (scale & net) is still
> running** ([`phase-3-plan.md`](phase-3-plan.md)) — its codeable surface has largely landed
> (A: flow-field caching ~3.7 ms/tick; B: the full in-process→UDP lockstep stack + RTT-adaptive
> delay + the `engine::net_tuning` RTT-estimator host seam; C: authoritative snapshot D28 +
> reconnect-by-replay; D: the `core::detection` tell D33 + the detection HUD (`render::detection`,
> `engine::detection_markers`) + the honest AI consult (`CommanderConfig::hunt_embodied`)),
> but the **net-facing tail is not** — the live RTT *sample source* (transport ping/pong in
> `pal-desktop`), matchmaking/relay wiring, the Wi-Fi↔cellular **handoff** (blocked on a QUIC
> transport), and the *two-human* PvP mind game all still need the live net layer. **This matters
> for Phase 4:** several app-shell surfaces (lobby /
> matchmaking, the in-session reconnect prompt, the PvP half of match setup) sit directly on top
> of Phase 3 plumbing that hasn't shipped, so they are **blocked-by-Phase-3** here, not buildable
> now. This doc is the product-of-record plan; it is sequenced by what is unblocked *today* and
> updated as slices land.
>
> **Goal (from [`roadmap.md`](../roadmap.md) §"Phase 4 — Polish & ship"):** wrap the game in
> everything that ships *around* the match — the **app shell** (every screen before, between, and
> after a match) and the first-run teach — and **tune it to mid-range silicon** (device quality
> tiers, dynamic resolution, thermal/battery), with telemetry + live-ops scaffolding that is
> **consent-gated** ([`infrastructure.md`](../infrastructure.md)).

---

## 1. What Phase 4 is (and isn't)

Phase 2 built the **in-match** UI — the touch command UI, fog, the embodied alert HUD (D24/D25).
Phase 4 builds everything *else the player touches*: the **app shell** (the eight surfaces below),
plus the **mid-range tuning** the Phase 1/3 caveats deferred (validated only on a flagship S24 —
D22/D21). The shipping touch *gameplay* scheme (D14/Q4) is **not** in scope here — it lives in the
in-match layer; only the **settings surface that configures it** is Phase 4.

The load-bearing structural fact for this phase is **[D32](../decisions.md)**: the out-of-match
shells are **native per-platform** (SwiftUI / Jetpack Compose / a desktop shell), **not**
Rust-workspace code, reached through a narrow **GPU-free, logic-free shell↔sim seam**. Only the
**in-session** shell stays in-engine (`engine`/`render`), because it renders under avatar-only fog
(invariant #6). Two consequences set the whole sequence:

1. **The seam comes first.** D32 says the shell↔sim boundary must be fixed *before* shell work
   begins. It is the prerequisite for every shell surface, native or in-engine.
2. **The repo has no native UI projects yet.** There is no SwiftUI/Compose/desktop-shell scaffold
   in the workspace, and (per Phase 3) **no iOS build target exists at all**. Every native
   out-of-match surface is therefore *new platform scaffolding on top of an unbuilt seam* — which
   is why this run greenlights the Rust-side work (the seam, the in-engine shell, tuning,
   telemetry) and defers the native shells.

---

## 2. The eight app-shell surfaces — buildable-now vs blocked

From [`roadmap.md`](../roadmap.md) §"Meta-UI / app shell". For each: what it covers, its dependency,
and a verdict. **All seven out-of-match surfaces are native (D32) and gated on the seam (§4 WS-A)
landing first** — that prerequisite is implicit in every "BLOCKED" below; the *additional* blocker
is named explicitly.

| # | Surface | Covers | Depends on | Verdict |
|---|---|---|---|---|
| 1 | **Boot & title** | Splash, title/attract, build-channel + version stamp | — | **LANDED (Android [D35](../decisions.md) + desktop [D36](../decisions.md)).** First native surface built once the seam landed: a Jetpack Compose title/landing screen on Android (`MainActivity` is now the launcher; engine `NativeActivity` is Start-launched) and an **egui** title screen on desktop (`app` boots a `Title`↔`InMatch` state machine instead of straight into a match; egui binds to the app's single shared wgpu 29/winit 0.30). **iOS** is the only platform still pending (no iOS build target at all). "Start" launches the engine's *default* match; match-config handoff stays deferred with match-setup ([Q5](../open-questions.md)), and Settings is a placeholder until surface 3. |
| 2 | **Onboarding / tutorial** | Teach the going-dark cost; telegraph the blindness *before* it bites; a guided first-possession beat. The single most important screen — invariant #6 lives or dies on whether a loss reads as *"I stayed too long."* | **[Q5](../open-questions.md)** (PvE is the natural teach surface); invariant #6 | **BLOCKED — [Q5](../open-questions.md).** The teach surface *is* the PvE-vs-PvP-first call; can't author the first-run beat before Q5 picks the first shippable mode. |
| 3 | **Settings** | Graphics tiers (↔ device quality tiers, §4 WS-C), audio-mix levels, the touch-layout/rebind editor (configures the D14 scheme), desktop key/gamepad rebinds, **accessibility** | invariant #6 (accessibility) | **PARTIAL (desktop) — [D75](../decisions.md).** The desktop Settings screen has landed in the egui shell with **audio master/SFX volume + look sensitivity/invert-Y wired through to the host** (`pal`/`pal-desktop`), plus fullscreen and a (dormant) quality choice; a Profile + About/field-manual screen landed alongside. Still **BLOCKED for the rest**: the **accessibility cues** (§5) the going-dark channel's fairness depends on, the touch-layout/rebind editor, and key/gamepad rebinds — so it stays native UI on the seam (D32) and can't be a thin afterthought. Android has no Settings surface yet. |
| 4 | **Match setup** | Army/loadout composition, map + mode select; skirmish-vs-PvP entry | order/stance vocab (D25); **[Q5](../open-questions.md)** | **BLOCKED — [Q5](../open-questions.md) (PvP half).** The skirmish/PvE half rides shipped order/stance vocab (D25), but "PvP entry" depends on Q5 *and* Phase 3 net; mode select is undefined until Q5. |
| 5 | **Lobby & matchmaking** (PvP) | Party/invite, connection-quality readout, ready-up. *Seam:* the net plumbing is Phase 3; only the surface is Phase 4 | **Phase 3 netcode** (D27 lockstep, host-RTT, relay/matchmaking); **[Q5](../open-questions.md)** | **BLOCKED — Phase 3 netcode + [Q5](../open-questions.md).** The connection-quality readout and ready-up sit on the live RTT *sample source* (transport ping/pong — the one stub the landed `engine::net_tuning` estimator seam still needs) + relay/matchmaking that Phase 3 still owes; no PvP target until Q5. |
| 6 | **Progression & profile** | Persistence, stats, cosmetic inventory | account/persistence backend ([`infrastructure.md`](../infrastructure.md)) | **BLOCKED — accounts/persistence backend.** The Postgres-backed accounts/entitlements service is scaffolding-only today ([`infrastructure.md`](../infrastructure.md)); no server code yet. |
| 7 | **Store / IAP** | Cosmetic purchases, restore-purchases, receipts, refund paths | **[Q9](../open-questions.md)** (per-platform billing rails); **[Q11](../open-questions.md)** (hero cosmetics feed the catalog) | **BLOCKED — [Q9](../open-questions.md) + [Q11](../open-questions.md).** Billing rails (mandatory StoreKit/Play Billing on mobile; desktop Stripe-vs-Steam) are unresolved (Q9); the catalog has nothing to sell until the hero-asset source (Q11) is picked. Gated by surface 8 (consent) at runtime. |
| 8 | **Consent & legal** | Telemetry/privacy consent, age gate, ToS/EULA — **gates** store + telemetry, so it precedes them | [`infrastructure.md`](../infrastructure.md) | **PARTIAL — the *gate* is buildable now, the *UI* is native.** The **consent-gate seam** (the boolean that telemetry + store check) ships now in `server` (§4 WS-D); the consent *screen* itself is native chrome on the seam (D32), blocked with the rest. Build the gate first so telemetry is structurally consent-respecting from its first byte. |

**In-session shell (the carve-out) — BUILDABLE NOW.** Pause, surrender/leave, post-match summary,
reconnect prompt. **In-engine** (`engine`/`render`), under avatar-only fog (invariant #6). The
reconnect-handoff *half* leans on Phase 3 (D28 reconnect-by-replay landed; the Wi-Fi↔cellular
handoff is QUIC-blocked), but the **prompt and the pause/surrender/summary surfaces are buildable
now** against the shipped reconnect policy — see §4 WS-B.

---

## 3. Dependency order

```
WS-A (shell↔sim seam, core) ──> WS-B (in-session shell, engine/render)
   │  (D32 prerequisite; ✅ landed this run, D34)      ▲
   │                                                   │ uses C/D's reconnect policy + summary data
   └────────────────────────────────> [native out-of-match shells]  (BLOCKED — see §2)
                                            ▲
WS-C (device tiers / dyn-res / thermal) ────┘  feeds the Settings "graphics tiers"
WS-D (telemetry + consent gate, server) ───────  feeds Consent + live-ops; independent
```

- **WS-A is the prerequisite** (D32). It is GPU-free, logic-free, single-sourced (invariant #2) —
  the same boundary discipline as the PAL. **It landed this run as `core::shell` ([D34](../decisions.md))**,
  so the rest of the phase has a fixed contract to design against.
- **WS-B (in-session shell)** is the one shell surface buildable now; it consumes WS-A's
  query side for post-match summary data and the Phase 3 reconnect policy for the reconnect prompt.
- **WS-C and WS-D are independent** of the seam and of each other — pure `engine`/`render` tuning
  and pure `server`/consent scaffolding respectively. Both can run alongside A/B.
- **The native out-of-match shells are LATER** — each blocked per §2 (Q5/Q9/Q11/Phase-3/backend),
  and all gated on WS-A landing plus per-platform UI projects the repo doesn't have.

---

## 4. Workstream decomposition

Four **buildable-now** Rust workstreams are greenlit for this run, mirroring Phase 3's A/B/C/D
shape. Status legend matches the other plans: **✅ DONE** · **IN PROGRESS** · **☐ not started** ·
**BLOCKED**.

### Workstream A — The shell↔sim seam (`core`) — ✅ DONE (landed this run, [D34](../decisions.md))

**Goal:** the narrow, **GPU-free, logic-free** command/query boundary native shells (and the
in-session shell) drive the shared engine through — the D32 prerequisite, fixed before any shell
work. **Landed as `core::shell` and recorded in [D34](../decisions.md).**

**What landed (the contract — [D34](../decisions.md)):** a typed **façade / DTO** module, `core::shell`,
on the same footing as the PAL — *intent in, presentation-safe view out*, holding **no** game/sim
logic (invariant #2/#3) and mutating no sim state.

- **Read side (`core` → shell) — presentation-safe views, never `&mut`, never checksum-folded:**
  `MatchStatus`/`MatchPhase` (host-driven lifecycle), the integer/`Fixed`-only
  `MatchSummary`/`FactionStats` (no float money/ratios — invariant #1), the **order/stance
  vocabulary as data** (`OrderKind`/`StanceKind` + `order_vocabulary()`/`stance_vocabulary()`,
  single-sourced from `core::components` — invariant #2), `ConnectionStatus` (a pure projection of
  `core::lockstep` — no sockets), and the fairness-critical `InSessionView`.
- **Control side (shell → `core`):** a coarse `ShellIntent` resolved by the pure `resolve_intent`
  into `ResolvedIntent::{Command, Session}` — a sim `Command` (Embody/Surface) **or** a host-side
  `SessionAction` (Pause/Resume/Surrender/RequestReconnect) that never enters the lockstep stream.
- **GPU-free / platform-free:** lives in `core` with **no** new dependency — `wgpu`/`winit`/JNI/
  SwiftUI absent; native shells reach it via FFI the way the PAL backends do.

**Fairness (invariant #6) is structural:** `InSessionView::compose` takes already-derived
avatar-only fog/alerts/`detection` tells, **never `&World`**, so it cannot leak strategic intel
while the world is dark.

**Load-bearing guard (met):** the seam touches **no** checksum-folded state on the read path and
feeds **no** float/logic into the command path — wiring it leaves the per-tick checksum stream
byte-identical (the bar every Phase 2/3 derivation cleared). Verified: `core` tests 193 → 202 green
(dev + release), float-free guard clean, `code-reviewer` CLEAN.

*Owed addition delivered with the landing:* the [`architecture.md`](../architecture.md) shell↔sim
boundary note and [D34](../decisions.md) recording the seam shape (§6).

*Not in this slice (deferred to the consuming workstreams):* the broader command surface — start/
configure/abort a match, apply settings, store/progression refresh — arrives with WS-B/native
shells, several of those blocked on [Q5](../open-questions.md)/[Q9](../open-questions.md)/[Q11](../open-questions.md);
and no win-condition evaluator (the host fills `MatchSummary` today — a single-sourced win condition,
if wanted, is a `core` *system*, not this boundary).

### Workstream B — In-session shell (in-engine) — ✅ DONE (landed)

> **Landed:** `engine::session_shell` (the pause / surrender / post-match-summary / reconnect-prompt
> state machine + the host-side `MatchSummary` assembler) and `render::overlay` (the screen-space
> chrome drawn over the — possibly dark — match frame). Consumes the WS-A seam (`SessionAction`,
> `MatchSummary`, `ConnectionStatus`/`LinkState`); checksum-neutral (a guard test proves driving the
> shell + assembler every tick leaves the sim stream byte-identical); fairness held (the overlay is
> screen-space only, the full-info summary appears only once `Ended`, and a desync drained from
> lockstep supersedes a local pause). Engine 81 / render 52 tests; `code-reviewer` CLEAN after fixing
> the desync-drain + pause-guard wire-up. **Also landed:** post-match DISMISS → title transition
> ([D52](../decisions.md)); pause overlay (desktop Esc [D53](../decisions.md), Android back-gesture
> [D54](../decisions.md)) + in-match surrender are now wired — the in-session shell goal is fully
> satisfied. **Also landed:** the Android leave-to-title path — the in-session overlay's
> **Surrender** → post-match summary → **DISMISS** tap now finishes the `NativeActivity` over JNI
> (`Activity.finish()`, best-effort + exception-clearing, never fatal), returning to the Compose
> `MainActivity` title; the twin of D52's desktop `ExitToTitle`, mirroring its decision flow
> (`overlay_click` → `OverlayClick::{Session,Dismiss}`). The shared pixel→NDC hit-test step both
> hosts run before `overlay_click` is now the unit-tested `engine::pixel_to_ndc` seam, so the
> leave-to-title tap can't diverge across desktop/Android (invariant #2). **Owed:** the
> Wi-Fi↔cellular reconnect *handoff* half stays QUIC-blocked (Phase 3 C).

**Goal:** pause, surrender/leave, post-match summary, and the reconnect prompt — rendered
**in-engine** (`engine`/`render`) under the same avatar-only fog as the match (invariant #6,
D32 carve-out). This is the *only* shell surface buildable now.

**Grounding:** it consumes WS-A's query side for summary data and the **Phase 3 reconnect policy**
(`core::reconnect::resume_from_snapshot`, D28 — landed) for the reconnect prompt; pause/surrender
map to existing match lifecycle. Render-side, it follows the D24/D25 in-match HUD pattern (pure
presentation derivation, checksum-neutral).

**Sequence (each commit green dev+release, path-scoped):**
1. **Pause + surrender/leave** — an in-engine overlay state in `engine`; pure presentation, never
   mutates sim state. In single-player, pause may halt the tick accumulator; in lockstep it cannot
   stall the shared clock (it is a *local* overlay, not a sim pause) — the load-bearing constraint.
2. **Post-match summary** — read the match result via the WS-A query snapshot; draw it in `render`.
   No new sim state.
3. **Reconnect prompt** — surfaced from the Phase 3 reconnect path; offers resume (snapshot +
   buffered-command replay, D28) or leave. The handoff *half* (Wi-Fi↔cellular) stays
   **BLOCKED-by-QUIC** (Phase 3 C) — the prompt is built against the shipped UDP reconnect policy.

**Fairness guard (invariant #6):** every surface here draws under avatar-only fog while embodied
and **leaks no strategic intel** — no minimap, no off-screen unit state in a summary teaser shown
mid-match. Render-side logic extracted to testable seams like `interpolate_instances` (CLAUDE.md
testing rule). `/safe-edit` (embodiment/fairness blast radius).

### Workstream C — Device quality tiers + dynamic resolution + thermal/battery tuning (`engine`/`render`) — ✅ DONE (landed)

> **Landed:** `render::tiers` (the `QualityTier` Low/Mid/High enum → `TierParams`, plus the pure
> `next_resolution_scale` dyn-res and `thermal_backoff` policy fns) and `engine::tuning`
> (`RenderTuning`, the controller `Game` owns). The thermal/power signal crosses a new **PAL** seam
> (`pal::ThermalSensor` → `ThermalState`/`PowerState`). The real **`pal-android` reader has landed**
> (`pal-android/src/thermal.rs` — `PowerManager.getThermalStatus()` / `BatteryManager`) — this is
> where the on-device 200-unit numbers that may reopen the **[D21](../decisions.md) dual-rate**
> question come from (record via `/decision` when measured). The load-bearing guard test steps the
> same scripted sim across Low/Mid/High × {Nominal,Fair,Serious,Critical} and asserts a
> byte-identical checksum stream — a tier is a *rendering* choice, never a sim input (invariant
> #1/#4). `core` untouched + float-free; `code-reviewer` CLEAN. **Dyn-res glue landed** —
> `resolution_scale` is now wired to a real intermediate render target + upscale blit in
> `render/src/lib.rs`.

**Goal:** make the game hold frame rate and thermal budget on **mid-range arm64**, retiring the
Phase 1/3 flagship-only caveat (D22/D21: validated on S24 only; mid-range frame-rate/thermal and
the 200-unit power budget were explicitly deferred here).

**Sequence:**
1. **Quality tiers** — a small enum of render tiers (resolution scale, draw distance, effect
   density, instance budget) selected per device class; feeds the Settings "graphics tiers"
   surface (surface 3). **Render-only** — invariant #1: tiers never touch the sim; the sim runs
   the *same* fixed 60 Hz tick at every tier (changing render fidelity must leave the checksum
   stream byte-identical, the guard test).
2. **Dynamic resolution** — scale the render target to hold the frame budget; presentation-only,
   reads frame timing, never sim state.
3. **Thermal/battery tuning** — read platform thermal/power signals through the **PAL** (a new
   `pal` query, never in `core`), and back off render cost (cap FPS, lower dyn-res floor) under
   thermal pressure. This is where the **D21 dual-rate re-evaluation** gets its on-device numbers:
   if 200 units at 60 Hz blows the thermal budget on mid-range silicon, that reopens dual-rate
   (Phase 3's deferred Dn) — record the outcome via `/decision`.

**Guard:** all render-side; the determinism matrix must stay green across every tier (a tier is a
*rendering* choice, never a *sim* choice — invariant #1/#4). `/check` before each commit.

### Workstream D — Telemetry + consent-gated live-ops scaffolding (`server` + consent-gate seam) — ✅ DONE (landed)

> **Landed:** in `server` — `consent` (a `ConsentGate` whose `guard`/`guard_with` *move* the payload
> in and return it only on consent, so a non-consenting path holds nothing to send — consent is
> structural, default-deny), `telemetry` (a typed event schema + `ingest` that can only reach the
> `TelemetrySink` through the gate), `liveops` (public-always / personalized-gated config), and an
> axum `http` router (`POST /v1/telemetry`, `GET /v1/liveops/config`) with a 64 KiB body cap on the
> ingest route. Consent rides the `X-Consent-Analytics` header (default-deny). Invariant #8 held — no
> secret added; tests use an in-memory sink so the suite is green **without Docker/Postgres** (CI +
> clone-and-run stay green). 18 unit + 7 HTTP + 1 doctest; `code-reviewer` CLEAN after making the
> validation-ordering test load-bearing + adding the body cap. **Deferred:** the native consent
> *screen* (D32 chrome) and the accounts backend. The real **Postgres `TelemetrySink` has landed**
> (feature-gated, `server/src/postgres.rs`) — production wiring still needs the accounts backend.

**Goal:** stand up telemetry + live-ops scaffolding that is **consent-respecting by construction**,
per [`infrastructure.md`](../infrastructure.md) — built so a player who hasn't consented emits
**nothing**, not "emits then filters."

**Sequence:**
1. **Consent-gate seam** — a single authoritative consent boolean (the thing telemetry *and* the
   store check) in `server`, with a client-side gate the native consent screen (surface 8) will
   later flip. **Build the gate before any emitter** so telemetry is structurally gated from its
   first byte. This is the buildable-now slice of surface 8.
2. **Telemetry pipeline (scaffold)** — event schema + an ingest endpoint in `server`, every emit
   path passing through the consent gate (no-consent ⇒ no-op at the source). Runs against the local
   Docker Postgres/Redis ([`infrastructure.md`](../infrastructure.md): clone-and-run, non-secret
   defaults; prod secrets stay in the private estate repo, invariant #8).
3. **Live-ops scaffolding** — the config/flag surface live-ops will need (remote-tunable values,
   consistent with the data/scripting hot-reload lean in [`roadmap.md`](../roadmap.md)), consent-gated
   the same way. Scaffold only — no live-ops *content* this phase.

**Guard:** server code, not `core` — no determinism matrix concern, but the no-secrets invariant
(#8) and consent-by-construction are the bars. Tests cover the gate (no-consent ⇒ zero emission)
and the ingest path.

### LATER — native out-of-match shells (BLOCKED)

Built once WS-A lands *and* per-platform UI projects exist (SwiftUI / Jetpack Compose / desktop
shell — none in the repo today; **no iOS build target exists at all** per Phase 3). Each surface
carries its own blocker from §2:

> **Android Compose parity:** the desktop egui shell has since grown several surfaces (Settings/
> Profile/About [D75](../decisions.md), gunsmith, campaign mission-select) that Android's Compose
> shell still lacks — the gap, its linchpin (a Compose→`NativeActivity` launch-config seam), and the
> tiered build order are planned in [`compose-shell-parity.md`](compose-shell-parity.md)
> ([D78](../decisions.md)/[D79](../decisions.md)).

| Surface | Blocker (beyond the WS-A seam + missing native project) |
|---|---|
| Boot & title | **LANDED (Android [D35](../decisions.md) + desktop [D36](../decisions.md))** (Compose title/landing screen on Android; egui title screen on desktop); Android now trails desktop by several waves — parity planned in [`compose-shell-parity.md`](compose-shell-parity.md); only the **iOS** native shell still pending (no iOS target) |
| Onboarding / tutorial | **[Q5](../open-questions.md)** (PvE-vs-PvP-first defines the teach surface) |
| Settings | owns the accessibility cues (§5) — must ship *with* them, not after |
| Match setup | **[Q5](../open-questions.md)** (PvP half + mode select) |
| Lobby & matchmaking | **Phase 3 netcode** (live RTT sample source, relay/matchmaking) + **[Q5](../open-questions.md)** |
| Progression & profile | **accounts/persistence backend** ([`infrastructure.md`](../infrastructure.md)) |
| Store / IAP | **[Q9](../open-questions.md)** (billing rails) + **[Q11](../open-questions.md)** (catalog content) |
| Consent & legal | the *gate* ships in WS-D; the *screen* is blocked native chrome |

---

## 5. Cross-cutting constraints

These outrank surface convenience and apply to **every** workstream.

- **Fairness (invariant #6) outranks the shell.** No meta-UI element — not a notification, a
  reconnect toast, nor a post-match teaser — may leak strategic intel *while embodied*. The
  **in-session shell renders under the same avatar-only fog as the game** (WS-B). This is why the
  in-session shell is the in-engine carve-out in D32: a native overlay drawn over the match could
  break the fog. Any surface that can appear mid-match is bound by this; an out-of-match native
  screen is not (you are not embodied there), but it must never *peek* into a live match.
- **Accessibility is load-bearing, not polish.** The going-dark alert channel is a directional
  **flash + audio** (invariant #6). A colorblind or hard-of-hearing player needs an **equivalent
  cue** — a non-color-coded flash, a haptic pulse, a visual transcription of the directional audio
  tell — or the core mechanic is **unfair to them**. The **Settings surface owns this** (surface
  3), which is why Settings can't be a thin afterthought: it ships *with* the accessibility cues,
  and those cues must be designed alongside the alert channel, not bolted on. This is a fairness
  obligation under invariant #6, not an optional feature.

---

## 6. Decisions Phase 4 will need (record each via `/decision`)

Do **not** resolve the open questions here — leans only; each lock needs the input named.

- **[D34](../decisions.md) — shell↔sim seam shape/contract** ✅ **RECORDED** — the GPU-free, logic-free
  `core::shell` façade native shells (and the in-session shell) drive `core` through (WS-A), plus the
  [`architecture.md`](../architecture.md) boundary note D32 flagged as owed. Landed this run.
- **[D58](../decisions.md) — Q5 resolved: PvE-first** ✅ **RECORDED** — the teach surface and
  first shippable mode are now decided (the Operations campaign), unblocking the *PvE/skirmish*
  halves of onboarding (surface 2) and match setup (4); only the *PvP* half of match setup and the
  lobby (5) stay blocked, now on **Phase 3 netcode** rather than Q5. (The §2 table cells below still
  read "BLOCKED — Q5"; treat that as superseded by D58 — PvE halves are unblocked, PvP halves wait
  on net.)
- **Dn — [Q9](../open-questions.md) resolution (billing rails)** — gates Store/IAP (surface 7).
  *Lean:* **hybrid** — mandatory StoreKit/Play Billing on mobile, Stripe/Steam on desktop, behind
  a unified entitlement service keyed to the account; cross-store reconciliation cost needs scoping
  first.
- **Dn — [Q11](../open-questions.md) resolution (hero asset source)** — gates the store catalog
  (surface 7) and the eye-level content the embodied camera lingers on. *Lean:* **hybrid** —
  CC0/procedural for low/mid, a small *commissioned* hero set, AI-gen for iteration/greyboxing
  only until its license terms firm up; scope the hero-asset count/budget before locking.
- **Dn (conditional) — D21 dual-rate re-evaluation outcome** — Phase 3's deferred call, but WS-C's
  on-device thermal numbers may force it here: confirm global-60, or adopt dual-rate with the
  two-clock contract, informed by the mid-range 200-unit thermal measurement.

---

**Phase 4's four buildable-now Rust workstreams have all landed** (A: the `core::shell` seam,
[D34](../decisions.md); B: the in-engine in-session shell; C: device tiers / dyn-res / thermal; D:
telemetry + consent gate) — full suite green dev+release, `code-reviewer` CLEAN on each. What remains
in Phase 4 is the **native out-of-match shells** — and "Boot & title" (surface 1) has now landed on
**both** platforms with a native shell: the **Android Compose landing screen** ([D35](../decisions.md))
and the **desktop egui title screen** ([D36](../decisions.md)), each the first native surface buildable
once the WS-A seam landed. Only the **iOS** Boot & title shell is still pending (no iOS build target at
all). The rest of the surfaces are still pending: deferred behind the per-platform UI projects the repo
still lacks and the Q5/Q9/Q11/Phase-3/backend blockers in §2 — Settings, onboarding, match setup,
lobby, store, consent all remain blocked. See [`roadmap.md`](../roadmap.md) §"Phase 4 — Polish & ship",
[D32](../decisions.md), [D35](../decisions.md), and [D36](../decisions.md).
