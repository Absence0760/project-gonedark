# Decision Log

Lightweight ADR-style record of the design choices locked in during pre-production,
and *why*. Newest decisions at the bottom. Open forks live in
[`open-questions.md`](open-questions.md).

---

## D1 — One player does both roles (not asymmetric commander + soldiers)

**Decision:** A single player is both the RTS commander and the FPS avatar, switching
between them — *not* the *Eximius* / *Natural Selection* model where the commander and
the soldiers are different people.

**Why:** The whole intended tension — opportunity cost, divided attention — only
exists if the *same* person owns both jobs. Splitting them across players removes the
core decision.

---

## D2 — Divided attention is the skill; the AI does not delegate

**Decision:** Command and embodiment are mutually exclusive in time. You cannot do
both at once.

**Why:** This is the game's primary skill expression — the macro/micro attention
split of high-level RTS, cranked to the extreme (embodying means *zero* command for
that whole duration).

---

## D3 — Unit AI is a literal executor, not a smart autopilot

**Decision:** Units hold their last order + a simple stance and do exactly that —
nothing more. No autonomous strategic decisions.

**Why:** Smart autopilot quietly lets the game play itself and makes "whose AI is
better" the contest instead of player skill. Dumb, obedient AI means leaving your
army is a *real* risk, which is the point. Bonus: it's cheaper and more deterministic
to build than smart LOD AI.

**Consequence:** Design depth moves into the **order/stance vocabulary** (patrol
routes, engagement ranges, retreat triggers, trigger zones, queued production) — that
is the intended home for "smart play": the human's planning, not the machine's
reactions.

---

## D4 — Switching is cheap; the absence is expensive

**Decision:** Entering/leaving a unit is fast and smooth. The cost of embodiment is
*time away from command*, never UI friction.

**Why:** Clunky menus are fake difficulty. The intended cost is opportunity cost; the
transition itself should never be the obstacle.

**Corollary:** Embodiment must be *mechanically better* at the local fight than AI
control, or players won't accept the cost and the FPS layer becomes a dead novelty.

---

## D5 — Pure opportunity cost: stay as long as you dare

**Decision:** No hard timer, fuel gauge, or cooldown forcing you out of a unit. You
stay embodied as long as you choose.

**Why:** It's the most elegant and most punishing option. The bound is emergent (risk
of death + neglect), not an artificial mechanic.

---

## D6 — Death is a demotion, not a game-over

**Decision:** Dying while embodied ejects you back to the command view. No FPS respawn
timer, no spectator downtime. You then pick a *different* living unit to embody. The
unit you were driving is gone for good.

**Why:**
- Zero dead time — death drops you into a fully active RTS, never an idle wait.
- Self-balancing — overstaying gets you killed, which costs you the unit *and* dumps
  you into a neglected base. Greed is auto-punished.
- Makes the embodied unit precious (you lose it permanently), tying the risk into the
  army economy.
- **Big tech win:** you always respawn as the commander, so there is **no FPS respawn
  system to build** — the avatar is just whichever entity you're currently driving,
  not a persistent character with lives. Deletes a pile of state/netcode complexity.

**Consequence — comeback loop:** if your whole army dies you're stripped to pure
base-builder until you produce a new embodiable unit. That's the intended low point.

---

## D7 — While embodied, the world goes dark (blind, not informed)

**Decision:** Embodiment reverts the player's vision to *avatar-only*. You do **not**
keep strategic map vision. (Chosen over the "informed dread" alternative where you
keep passive map sight and watch helplessly.)

**Why:** Turns information into the game's real currency. The central skill becomes
*"can I afford to be blind right now?"* — reading the board before you dive. The blind
version makes "stay as long as you dare" genuinely tense because you don't even know
what you're sacrificing.

**Guardrails (so it's fair, not robbery):**
- **Alerts, not intel** — a thin thread back: directional flash + audio ("base under
  attack"), telling you *something* is wrong but not what/how bad.
- **Audio is a primary system** — strategic-layer sound bleeds into the embodied mix;
  you *hear* your empire when you can't see it.
- **Visceral, constant feedback** — vignette / darkened edges so the cost is always
  *felt*; the player must own every death as "I stayed too long."

**Tech note:** this is a vision-culling toggle in the presentation layer, not a sim
change — it cannot cause desyncs and needs no special pause machinery.

---

## D8 — Pre-production is design-only; engine direction is custom-native with a live fallback

> **Language resolved in [D10](#d10--engine-language-rust): Rust.** The "C++ or Rust"
> openness below is superseded; the rest of D8 stands.

**Decision:** Target Android-first (arm64-v8a, Vulkan 1.1). Lead direction is a custom
native engine (C++20 or Rust — a performance wash; decide on engineering values).
Keep Unity DOTS / Godot+GDExtension as a live fallback until the vertical slice is
validated on real hardware.

**Why:** The RTS ceiling (deterministic many-agent sim in a fixed budget) needs
cache-line control that managed runtimes abstract away — but the build cost is real,
so we don't burn the fallback until Phase 1 proves the slice. See
[`architecture.md`](architecture.md) and [`roadmap.md`](roadmap.md).

---

## D9 — Four platforms: one shared deterministic core, platform-optimized backends

**Decision:** Ship on **Windows, Linux, Android, and iOS**. The game architecture (ECS,
deterministic fixed-point sim, game systems, AI, netcode) is **identical on every
platform**; only the **platform backend** (GPU API, audio, windowing, input, storage)
and the **presentation tier** (resolution, effects, refresh, controls) are optimized
per device — native paths: **D3D12/Vulkan on Windows, Vulkan on Linux, Vulkan 1.1 on
Android, Metal on iOS.** Develop on Linux desktop first; ship Android-first.

**Why:** "Optimized per device" must mean *native backends*, not *forked game logic*.
Forking the core would (1) kill cross-platform lockstep — deterministic play requires
every client to run the *same* sim — and (2) turn one game into four to maintain. The
float-free fixed-point sim already produces bit-identical results across x86-64 and
arm64, so cross-play across all four platforms is achievable *for free* — provided the
per-tick checksum CI runs across the whole platform/compiler/arch matrix from day one.

**Consequences:**
- A **Platform Abstraction Layer (PAL)** boundary is enforced from Phase 0; the core
  carries zero platform dependencies (`winit` collapses most of the non-GPU PAL).
- The renderer talks to one **RHI** — `wgpu` (native Vulkan/D3D12/Metal per device)
  hits all four targets with no per-platform renderer. *(This was a decisive factor in
  later choosing Rust as the engine language — see [D10](#d10--engine-language-rust).)*
- iOS carries the most external friction (macOS build host, signing, review, Metal) and
  is sequenced last.

See [`platforms.md`](platforms.md) for the full plan.

---

## D10 — Engine language: Rust

**Decision:** Build the engine in **Rust**, not C++. Resolves the language question left
open in [D8](#d8--pre-production-is-design-only-engine-direction-is-custom-native-with-a-live-fallback).
Renderer via **`wgpu`** (native Vulkan/D3D12/Metal, auto-selected per device); ECS via a
mature crate (Bevy/hecs/legion) or hand-rolled; FFI to Kotlin/JNI (Android) and
Swift/Obj-C (iOS) for platform services.

**Why:** Performance is a wash (both are LLVM-native), so the call is on engineering
values over a long horizon — and for *this* project (greenfield, small/AI-assisted team,
custom-native, mobile+desktop, determinism-critical) Rust's strengths hit the load-
bearing needs while its weaknesses miss them:

- **Cross-platform GPU is solved by `wgpu`** — native backends per device, which is
  exactly the D9 goal. C++ would mean hand-rolling an RHI or adopting The Forge/bgfx.
- **Fearless concurrency.** The deterministic lockstep sim is heavily threaded (job
  system, parallel culling/pathfinding/AI). Its worst bugs are silent, non-reproducible
  data races and determinism leaks. Rust eliminates that entire class at compile time —
  a correctness guarantee C++ cannot give. This is the decisive factor.
- **Determinism is more enforceable** via the type system (newtype fixed-point, no
  implicit float conversions, no UB, no uninitialized reads).
- **Toolchain simplicity** — `cargo` vs CMake + vcpkg/Conan + per-platform toolchains;
  compounds across four platforms over years.

**The maturity question, answered:** Rust's remaining gaps don't intersect this project.
- *Engine-code hot-reload* (no stable ABI) — the one real cost; mitigated by doing
  game-feel iteration in scripting/data + the automated build loop (roadmap). Architect
  a reloadable module only if it hurts.
- *Commercial middleware / hiring pool / console toolchains* — not load-bearing for a
  custom-native, mobile+desktop, small-team project; platform services cross into
  Kotlin/Swift via FFI regardless of engine language.

**When this should be revisited:** if priorities shift to shipping in ~6 months, hiring a
C++-veteran team, targeting consoles, or building on a commercial engine/middleware
stack — none currently true.

**Consequences:**
- The risk to actively manage is **engine-iteration speed** — lean on scripting/data hot-
  reload and the automated edit→build→test loop.
- iOS scripting caveat stands: no JIT, so any embedded VM (e.g. Lua) runs interpreted.
- Architecture/platforms docs now treat Rust as the language; C++ remains a noted
  fallback only if D10 is ever reversed.

---

## D11 — Local-first dev (Docker + committed defaults); prod secrets via sops; infra via Terraform

**Decision:** Three locked conventions for config, services, and secrets:
- **Clone-and-run local development.** `.env.development` is committed with safe,
  non-secret defaults; local services (Postgres, Redis) run via Docker (`compose.yaml`).
  A fresh clone runs with no cloud access, no secrets, no manual config. Personal
  overrides go in gitignored `.env.local`.
- **Production secrets in `./infra-secrets/`, KMS-encrypted with sops.** Only
  `*.sops.yaml` (ciphertext) is committable; `.gitignore` blocks all plaintext in that
  directory. Consumed by Terraform via the `carlpett/sops` provider (`data "sops_file"`
  → `local.secrets[...]`), or decrypt-to-tfvars as the alternative.
- **All cloud infrastructure is Terraform** (`infra/`), in this project's own AWS
  account + estate baseline; tfenv-pinned (1.15.0). No click-ops.

**Why:** Clone-and-run removes the credential dance from every contributor's (and AI
agent's) first experience, which compounds over the project's life. Encrypted-by-default
secrets + Terraform-only infra make the prod path auditable, reproducible, and safe to
keep in git — matching the rest of the personal estate.

**Estate caveat:** the global convention keeps prod secrets in the *separate private*
`Absence0760/infra-secrets` repo, not in (often public) project repos. This project
keeps them in-repo per the explicit `./infra-secrets` instruction; KMS encryption makes
that safe (same pattern meryl-green-designs uses). If this repo is published and shipping
ciphertext is undesirable, lift `infra-secrets/` into the private repo and re-point
`infra/secrets.tf` — nothing else changes.

**Status:** scaffolding ahead of code (no backend/services exist yet). The conventions
and `.gitignore` guards are in place so nothing is retrofitted. See
[`infrastructure.md`](infrastructure.md), [`../infra/README.md`](../infra/README.md).

> **Superseded in part by [D12](#d12--production-secrets-move-to-the-private-estate-repo-not-in-repo).**
> The secrets-*location* bullet ("in `./infra-secrets/`") no longer holds: secrets moved
> to the separate private estate repo. The clone-and-run, Terraform-only, and
> encrypted-by-default conventions stand.

---

## D12 — Production secrets move to the private estate repo (not in-repo)

**Supersedes:** the secrets-*location* portion of [D11](#d11--local-first-dev-docker--committed-defaults-prod-secrets-via-sops-infra-via-terraform).
Everything else in D11 stands unchanged.

**Decision:** This project's KMS-encrypted production secrets live in the **separate
private estate repo** `Absence0760/infra-secrets` (checked out at `~/github/infra-secrets/`,
a sibling of this repo), under a `gonedark/` subdirectory — **not** in a
`./infra-secrets/` folder inside this repo. The in-repo folder created by D11 is deleted.
Terraform's `carlpett/sops` data source now reads
`${path.module}/../../infra-secrets/gonedark/prod.sops.yaml`. The estate repo's
`.sops.yaml` gains a `^gonedark/.*\.sops\.yaml$` rule keyed to `alias/gonedark-sops`
(this project's own KMS key), and encrypts **every value** (no `encrypted_regex`) —
strictly safer than the per-key regex the in-repo `.sops.yaml` used.

**Why:** D11 kept secrets in-repo and justified it by citing "the explicit
`./infra-secrets` instruction" and the meryl-green-designs precedent. Both are weak: the
"instruction" traced back to this project's own earlier docs (circular), and meryl
committing its `*.sops` into a *public* repo is the exact anti-pattern the estate is
**migrating off**, not a model to copy. The global estate convention is
defense-in-depth — GitHub *private-repo* access control **and** KMS — and this game repo
is likely to be made public. Even though KMS ciphertext is safe to expose, shipping it
from a public repo throws away one of the two layers for no benefit. One subdir per
project in the shared private repo is the established estate shape (`flakey/`,
`running/`), so gonedark now matches it.

**Cost of the move:** near zero — no real secrets existed yet, so nothing had to be
re-encrypted. It was a scaffolding + path + docs change: repoint `infra/secrets.tf`,
delete the in-repo folder, drop its `.gitignore` block, and update cross-references.

**Status:** done, ahead of any backend code. `~/github/infra-secrets/gonedark/` holds
`prod.sops.yaml.example` (template) and the `.sops.yaml` rule with a placeholder ARN;
the real `alias/gonedark-sops` key + encrypted `prod.sops.yaml` get created when
`new-project-account.sh gonedark` runs and the backend needs its first secret. See
[`infrastructure.md`](infrastructure.md), [`../infra/README.md`](../infra/README.md),
and the estate repo's own `README.md` (`~/github/infra-secrets/README.md`).

---

## D13 — Monetization: cosmetic-only (weapon & player/unit skins)

**Decision:** The game monetizes **only through cosmetics** — weapon skins and
player/unit skins. **No pay-to-win:** nothing purchasable touches stats, balance, or
capability. Cosmetics are **presentation-layer only** and never enter the simulation.

**Why:**
- It's the one revenue path that doesn't corrode the competitive core. The entire design
  is a fairness argument — literal-executor AI (D3), "going dark" must read as *"I stayed
  too long"* (D7), divided attention as the only skill (D2). Pay-to-win detonates all of
  it. Skins sell **identity, not advantage.**
- It rides the decoupled sim/render split for free: a skin is a render-asset swap, so it
  *cannot* affect determinism or lockstep (invariants #1, #4). Monetization adds no sim
  surface area.

**Guardrails (non-negotiable — so cosmetics can't become pay-to-win by accident):**
- **Sim-identical.** A skin must never change hitbox, collision, unit
  silhouette/footprint/size, or readability/visibility. The sim is bit-identical
  regardless of equipped cosmetics — two players with different skins compute the same
  world.
- **No tell tampering.** A skin must not suppress, fake, or alter any "embodied unit"
  tell. [Q2](open-questions.md) resolved ([D33](#d33--going-dark-detection-a-tunable-three-mode-tell-default-subtle))
  to a tunable tell whose `Marked`/`Subtle` markers are engine-owned (`core::detection`) — skins
  render *under* them, never over them.
- **Out-of-band loadout.** Cosmetic choice travels as a non-sim-affecting tag alongside
  player identity/entitlements, **never** as sim state in the lockstep order stream. A
  peer missing a skin asset falls back to the default model with zero sim divergence.

**Consequences:**
- Validates the planned **accounts + entitlements** backend (and the Stripe hint in the
  infra scaffolding): purchases map to per-account cosmetic entitlements, resolved at
  load and applied in the render layer only.
- Asset-pipeline load: cosmetic variants are extra cooked assets under the mobile
  texture/download budget (ASTC/atlas discipline — see `architecture.md` §asset
  pipeline). Skins are the first real content-volume driver; budget for them.
- Opens a **new billing-rails fork → [Q9](open-questions.md):** mobile-first means
  digital cosmetics on iOS/Android generally **must** route through Apple StoreKit /
  Google Play Billing (platform IAP policy + revenue share), so Stripe/Steam apply only
  to desktop/web storefronts. Which rails per platform — and whether entitlements unify
  across them — is undecided.

---

## D14 — Phase 0 control prototype passes; touch-feel risk retired

**Resolves:** [Q4](open-questions.md) — the touch-control product risk (the project's #1
non-engine risk).

**Decision:** The Phase 0 control prototype is a **pass.** The embody↔command loop feels
good in hand on a touchscreen, validated hands-on on real hardware (Samsung Galaxy S24,
SM-S921U1). The risk that *CoH*-style command **plus** a competent FPS scheme **plus** an
instant swap between them couldn't be made to feel good on a small touchscreen is
**retired.** Greenlight to proceed past Phase 0 — the next gate is the **Phase 0.5**
embodiment-over-network latency spike ([`roadmap.md`](roadmap.md)), *not* Phase 1 engine
work.

**What was tested:** a throwaway **Godot 4.6** build (`prototypes/phase0-controls/`,
explicitly disposable — **not** the engine; the real engine is Rust/`wgpu` per D10, built
fresh at Phase 1). It modelled **feel only** and stayed faithful to the locked design: one
unit; tap-to-move literal-executor command layer (D3) with drag-pan and pinch-zoom;
**embody = swap the same entity's input source + flip vision to avatar-only**, no
character/respawn system (invariant #5, D6/D7); world-goes-dark with a constant vignette +
"BLIND" tell and **alerts-not-intel** (directional banner + haptic buzz, never a map
reveal — §6/D7); embodied scheme = left-thumb virtual stick, right-drag look, FIRE
hitscan, instant SURFACE.

**Why this is enough to proceed:** Phase 0's sole job per the roadmap was to answer *"does
the embody↔command loop feel good in hand?"* before building any systems. On a real device
the answer is yes — the control scheme and the instant swap, the two things that *had* to
feel right, do. The existential framing Q4 carried ("if it isn't fun in hand, the concept
reworks or dies here") is settled in the concept's favour. Detailed shipping touch UI
(multi-unit selection, the full order/stance vocabulary surfaced on a touchscreen) is
downstream Phase 2 design work, not a reopening of this question.

**Caveats carried forward (on the record, not blockers):**
- **Audio is still unproven.** The prototype faked it with haptics + visuals, but D7/§6
  make audio a *primary* system for going-dark — "hear your empire when you can't see it."
  The full blind-but-hearing feel is not validated until real audio exists; revisit before
  Phase 0 is considered fully closed.
- **Single-unit and local only.** The prototype cannot surface the *next* risk — embodied
  FPS feel **over** lockstep + input-delay netcode (Q7/Q8). That is exactly what Phase 0.5
  exists to answer, before the engine spine is committed.
- **Throwaway.** Keep `prototypes/phase0-controls/` as a reference artifact through Phase
  0.5, then delete it. It carries none of the engine invariants (fixed-point sim, PAL
  boundary, sim/render split) by design — do not grow it into the game.

**Consequence:** the next concrete step is the Phase 0.5 latency spike (two networked
clients, one embodied unit each, real input delay at the real 30 Hz tick), which must clear
before the Rust engine spine (Phase 1) is committed.

---

## D15 — Embodied combat over lockstep: avatar-local prediction (Phase 0.5 passes)

**Resolves:** [Q7](open-questions.md) (netcode model for embodied combat). Advances —
but does **not** close — [Q8](open-questions.md) (tick rate).

**Decision:** Embodied first-person combat rides the deterministic lockstep + input-delay
netcode **with avatar-local prediction**: the client predicts *only the player's own
embodied entity* locally and reconciles it against the authoritative tick, while everything
else stays pure lockstep. The Phase 0.5 latency spike **passes** — with prediction on,
embodied 1v1 combat feels good across every tested connection quality (up to a simulated
"cellular, worst" preset); with prediction off (raw lockstep + input delay) it feels laggy.
Greenlight the **Phase 1** engine spine.

**Why:** Phase 0.5 existed to prove embodied feel over the **RTS-optimal / FPS-hostile**
netcode *before* committing the engine (see [`architecture.md`](architecture.md) §"Embodied
combat over lockstep", roadmap). The spike — phone (host) vs laptop (client) over real
Wi-Fi, with a tunable RTT/jitter/loss injector — showed prediction delivers responsive aim
and movement in every condition while raw lockstep does not. This **confirms the prior
lean**: predict the one entity you're twitch-controlling, keep the ~200-unit sim on pure
lockstep, and the deterministic core stays intact.

**Hard rule for Phase 1 (non-negotiable):** the prediction lives in the
**presentation/input path only** and must **never feed back into deterministic sim state**,
or it desyncs lockstep silently. Authoritative hit resolution and the remote view still
resolve at tick **T+D** inside the fixed-point sim. The prediction/reconciliation boundary
must be designed in from the **first netcode commit** — retrofitting it into a finished sim
is far costlier, which is the entire reason this spike preceded Phase 1.

**Q8 (tick rate) — still open, leaning hold-30 Hz.** The harness ran at 30 Hz by default
and prediction made it feel good in all conditions, so "hold 30 Hz, lean on prediction"
remains the lean. But the 30↔60 Hz toggle was **not** A/B'd in this pass, so 30 Hz is *not*
yet rigorously confirmed — **close Q8 early in Phase 1.**

**Caveats (on the record, not blockers):**
- The harness is throwaway and uses **floats** — it is **not** a determinism test; Phase
  1's fixed-point sim is still unproven (invariant #1 stands).
- 1v1 over LAN/Wi-Fi with an **idle opponent** — remote-avatar motion under jitter/loss
  driven by a *second human* was not stress-tested.
- **Audio still faked** (carry-forward from D14); the going-dark audio feel remains unproven.

**Consequences:**
- [`architecture.md`](architecture.md) §"Embodied combat over lockstep" flips from *open
  tension* to a **settled approach** (avatar-local prediction + the presentation-path-only
  rule). The determinism checklist gains the "prediction never writes sim state" guard.
- Phase 0.5 is **done** in the roadmap; **Phase 1 (Rust engine spine) is unblocked.**
- Both throwaway prototypes (`prototypes/phase0-controls`, `prototypes/phase0.5-netfeel`)
  have served their purpose and may be deleted (or kept briefly as reference).

---

## D16 — 30 Hz is too coarse for embodied combat; embodied layer needs ~60 Hz

**Resolves:** [Q8](open-questions.md) (is a 30 Hz tick enough for embodied combat?).
**Opens:** [Q10](open-questions.md) (how to deliver the higher rate — global vs dual-rate).

**Decision:** **30 Hz is not enough for embodied first-person combat** — it must run at a
**higher tick (target 60 Hz)**. In the Phase 0.5 harness the player A/B'd the 30 Hz↔60 Hz
toggle during embodied gunfights and the difference was **dramatic**: 30 Hz felt
"chunky/bad", 60 Hz was the only acceptable rate for first-person aim/fire. This held
**even with avatar-local prediction ([D15](#d15--embodied-combat-over-lockstep-avatar-local-prediction-phase-05-passes)) on** — prediction removes input *latency*
but cannot mask the *granularity* of hit/aim resolution, which happens at the sim tick.
Direction: prioritize the higher-rate embodied path in Phase 1 and accept/optimize the cost.

**Why:** Phase 0.5 existed to settle exactly this before the engine. Prediction (D15) fixed
*lag*; this fixes *coarseness* — a distinct axis. The two together are what make embodied
combat feel good, and both had to be proven by hand before committing the sim loop.

**What this changes — and what it does NOT:** this revises the long-assumed **"fixed 30 Hz
sim" figure** (architecture Targets; the *rate parameter* of invariant #4). Only the number
moves. Still standing:
- **Invariant #4's core** — sim/render **decoupling** and a **fixed deterministic tick**.
  The render rate still floats and interpolates; the sim is still a fixed-step lockstep
  clock. Only its rate is up.
- **Invariant #1** — embodied aim/recoil/raycast/ballistics still resolve **inside the sim**
  and stay **fixed-point with LUT trig**, at whatever rate. A faster tick does not admit
  floats.

**Mechanism is deferred to Q10 (needs real-arm64 profiling, early Phase 1):**
- **(a) Global 60 Hz** — one tick rate, simplest. ~2× total sim CPU for ~200 units and
  ~2× battery/heat; per-tick work (~8.5 ms) still fits a 60 Hz (16.6 ms) tick, so it's
  *feasible*, just power-hungry. Viable given the "accept the cost" direction if (b) proves
  too complex.
- **(b) Dual-rate** — heavy RTS/unit sim at 30 Hz, embodied-combat resolution (avatars, aim,
  hit reg) at 60 Hz. Far cheaper at scale, but two deterministic clocks that **both** must
  stay lockstep-deterministic — real added complexity.
- **(c)** The old "aim-sampled-at-render, committed-at-tick" idea is **insufficient alone** —
  the chunkiness *is* the 30 Hz commit granularity; committing at 30 Hz won't fix it.

**Consequences:**
- Architecture **Targets** ("Sim 30 Hz"), the **simulation-loop** section, and the
  **frame/sim budget** table all assumed 30 Hz and are now marked *to be finalized in
  Phase 1* (the budget table's per-tick headroom math changes at 60 Hz).
- **Mobile thermal/battery budgeting moves earlier** — it was a Phase 4 concern, but the sim
  rate now drives it, so it's a Phase 1 profiling input.
- Q8 is closed; the implementation fork lives in **Q10**, to settle on real hardware
  **before the Phase 1 sim loop is locked**.

**Caveat:** the spike measured *feel* on a throwaway float harness with an idle opponent. It
proves 30 Hz feels too coarse — **not** the exact rate ceiling or the per-device cost, which
Phase 1 profiling establishes (60 Hz is the working target, not yet a locked floor).

---

## D17 — Fixed-point sim scalar: a hand-rolled Q16.16 `Fixed` newtype

**Decision:** The simulation's only scalar type is a **hand-rolled `Fixed` newtype**
(`i32`, Q16.16; i64 intermediates for mul/div; explicit wrapping arithmetic) living in
`core::fixed` — **not** the `fixed` crate. It deliberately implements **no** conversion to
or from `f32`/`f64`; the renderer converts at its own boundary via `Fixed::to_bits()`.
Transcendentals are LUT/integer (`core::trig`: build-time-baked sine table, integer
`isqrt`), never `libm`. This closes the first Phase-1 decide-first gate
([`phase-1-plan.md`](plans/phase-1-plan.md) §2).

**Why:** invariant #1 is that the sim is bit-identical across arch/compiler, and a float
leak desyncs lockstep *silently*. Owning the type makes "no floats in the sim" a **compile
error** rather than a lint: with no `From<f32>`, a stray float simply does not typecheck in
`core`. The `fixed` crate ships float conversions (so a float *would* compile) and would put
a determinism-critical dependency in the core's hot path; and since the LUT trig has to be
hand-built regardless (the crate gives no deterministic transcendentals), it saves little.
The structural guarantee beats the convenience. Cost — getting overflow/division exactly
right — is covered by unit tests incl. a cross-arch checksum in CI from day one (invariant
#7). The build script that bakes the LUT may use host `f64` at compile time (its output is
pure integer data, never executed in the sim); that one spot carries a `// noqa` rationale.

---

## D18 — ECS storage: hand-rolled struct-of-arrays (not an off-the-shelf ECS)

**Decision:** `core`'s world is a **hand-rolled struct-of-arrays** store (`core::ecs`):
parallel dense `Vec`s per component, entity = index + generation handle, systems iterate by
index. **Not** Bevy/hecs/legion. Closes the second Phase-1 decide-first gate
([`phase-1-plan.md`](plans/phase-1-plan.md) §2).

**Why:** determinism needs a **stable iteration order** (invariant #1/#7), and an archetype
ECS does not contract its iteration order across spawns/despawns or versions — adopting one
means pinning a version and *auditing* order on every bump, fighting the library to
guarantee something it doesn't promise. Index iteration is stable **by construction** and
never touches a randomised `HashMap`. It also gives full control of the SoA memory layout
that the 200-agent hot path needs ([`architecture.md`](architecture.md)). Phase 1 needs one
unit and ~5 components, so the initial store is small and grows with the game; the cost is
weaker query ergonomics (more per-system boilerplate), which is an acceptable trade for
determinism + layout control. Same principle as D17: **own the load-bearing thing, make the
guarantee structural.**

**Still open — not decided here:** the **sim tick rate** (global-60 vs dual-rate, Q10/D16)
is parameterized as `core::sim::TICK_HZ` and must be profiled on real arm64 before locking;
the choice of off-the-shelf crates for *non-sim* layers (wgpu/winit in render/PAL) is
unaffected by D18.

---

## D19 — The GPU device crosses into the renderer at the concrete wiring layer, not through the abstract PAL trait

**Decision:** `wgpu` enters the build at `render`, `pal-desktop`, `pal-android`, and `app`
only. The **abstract `pal` crate stays GPU-free** — its `Rhi`/`Window`/`Input` traits name
no `wgpu` type. The concrete desktop backend (`pal-desktop`) owns the `winit` window plus
the `wgpu` `Instance`/`Adapter`/`Device`/`Queue`/`Surface`, and exposes them through
**concrete accessors** (`device()`, `queue()`, `format()`, `acquire()`, `present()`). The
`app` wiring layer — which already depends on the concrete backend — hands that `&wgpu::Device`
to `render::Renderer::new(device, format)` and calls `renderer.render(device, queue, view,
&camera, world_dark)` each frame. `core` and `pal` depend on **no** GPU/windowing crate
(invariant #2 holds verbatim). This unblocks Phase-1 build-order steps 4–5 without routing a
GPU handle through the portable seam.

**Why:** the renderer genuinely needs a `wgpu::Device` to build pipelines and buffers, but
invariant #2 / D9 forbid `core` and the *abstract* PAL from seeing a GPU API. Two ways to
give `render` a device were possible: (a) widen the abstract `pal::Rhi` trait with
`wgpu`-typed methods, or (b) let the device cross only at the **concrete** layer where the
backend and renderer are already platform-specific. (a) would drag `wgpu` into the trait
crate that `core`-adjacent code links and pin every backend to wgpu's trait shape — the
opposite of a *thin* seam. (b) keeps the portable boundary exactly as narrow as D9 intended:
the abstract traits stay an engine-neutral vocabulary, and the wgpu coupling lives entirely
in the three crates that are per-platform anyway (`render` + each `pal-<platform>` + `app`).
The renderer talks to `wgpu` (→ Vulkan/D3D12/Metal per device) directly; it never talks to a
*specific* GPU API or to `winit`, so the RHI-over-many-APIs property (glossary) is intact.

**Consequences:**
- `render`, `pal-desktop`, `pal-android`, `app` gain `wgpu` (and `winit`/`pollster`/
  `raw-window-handle` on the desktop backend, `glam` for render-side float matrices). `core`
  and `pal` keep an **empty** dependency list — the invariant-#2 tripwire stays armed.
- The `render::Renderer` API freezes to `new(&Device, TextureFormat)` + `prepare(prev, curr,
  alpha)` + `render(&Device, &Queue, &TextureView, &Camera, world_dark)`. Q16.16→`f32`
  conversion stays inside `render` (invariant #1).
- `pal-desktop` freezes a concrete `DesktopRenderSurface` (window + surface + device/queue)
  and a `DesktopInput` that maps `winit` events onto the engine-neutral `pal::InputFrame`.
- No change to invariants #1/#4: the sim stays fixed-point and decoupled; `render` only ever
  reads a `Snapshot` and never calls back to mutate sim state.

---

## D20 — The platform-agnostic game loop is a shared `engine` crate both hosts drive

**Decision:** The per-frame game loop — the deterministic fixed-tick sim advance, render
interpolation (invariant #4), the camera/unproject math, the command-layer tap→`Move`
mapping, the embodiment input-source swap (invariant #5), and the avatar-local-prediction
seam (D15) — lives in a new **`engine`** crate (`gonedark-engine`) that exposes one entry
point, `Game::frame(input, dt, viewport, device, queue, view)`. **Both** hosts drive it: the
desktop `app` (a thin `winit` `ApplicationHandler`) and Android's `android_main` (the
`android-activity` loop in `pal-android`). `engine` depends on `core` + `render` + `pal`
(plus `wgpu`/`glam` — the render-side wiring layer, D19) but on **no** windowing/platform
crate. Each host owns only its surface/input/lifecycle and feeds the engine an `InputFrame`,
a wall-clock `dt`, and the acquired surface view. Both seed the sim with the same
`DEFAULT_SEED`, so desktop and Android run the **bit-identical** deterministic scene.

**Why:** the loop had been written inline in the desktop winit host, and Android originally
had only a present-only clear. Wiring the real sim+render through `android_main` by
*duplicating* that logic would fork the game loop per platform — the exact failure invariant
#2 / D9 exist to prevent (it kills cross-play and doubles maintenance). Extracting it into a
shared crate keeps **one** loop on every platform while the genuinely per-platform parts
(window/surface/input/lifecycle) stay in the `pal-*` backends. A new crate (rather than a lib
target on `app`) is required to avoid a dependency cycle: `app → engine`, `pal-android →
engine`, `engine → {core, render, pal}` — all acyclic, whereas `pal-android → app` (for the
android entry) plus `app → pal-android` (its android-target dep) would not be.

**Consequences:**
- New `engine` crate in the workspace; `app` slims to a thin desktop host (drops its direct
  `core`/`render`/`glam` deps — `engine` re-owns them). `pal-android` gains an
  android-target `engine` dep and drives `Game::frame`.
- `AndroidRhi` becomes a **concrete** surface (`device()`/`queue()`/`format()`/`acquire()`/
  `present()`, mirroring `pal-desktop::DesktopRenderSurface`) and no longer implements the
  abstract `pal::Rhi` trait — consistent with D19 (the device crosses at the concrete wiring
  layer, which `engine` now is).
- No invariant changes: the sim stays fixed-point and decoupled; floats remain render-side;
  the tap target is still quantized to `Fixed` at the input boundary (invariant #1).

---

## D21 — Sim rate: a single global 60 Hz tick for Phase 1 (dual-rate deferred, not killed)

**Resolves:** [Q10](open-questions.md) (how to deliver the ~60 Hz embodied rate — global vs
dual-rate). Settles the last open Phase-1 decide-first gate
([`phase-1-plan.md`](plans/phase-1-plan.md) §2).

**Decision:** the simulation runs **one global 60 Hz** deterministic tick for Phase 1
(`core::sim::TICK_HZ = 60`). The **dual-rate** split (heavy RTS/unit sim at 30 Hz, embodied
combat at 60 Hz) is **not adopted now**. `TICK_HZ` stays a single named constant so the rate
is trivially re-tunable, and dual-rate is explicitly **deferred, not discarded**.

**Why:** [D16](#d16--30-hz-is-too-coarse-for-embodied-combat-embodied-layer-needs-60-hz) settled
that embodied combat needs ~60 Hz but deferred the *delivery mechanism* to **profiling on real
arm64**, with the lean "start global-60; fall to dual-rate only if the 200-unit power/thermal
projection forces it." Phase 1 carries **one** unit, and the slice now runs on real arm64 (an
Adreno 750, Galaxy-class), where a global 60 Hz tick has enormous headroom. At that scale the
dual-rate machinery — **two** deterministic clocks that *both* must stay lockstep-deterministic
([D15](#d15--embodied-combat-over-lockstep-avatar-local-prediction-phase-05-passes)) — is
unjustified complexity. This follows D16's explicit lean exactly.

**The 200-unit question is real, but it is a *scale* concern, not a Phase-1 one.** Whether
global-60 wrecks the mobile power/thermal budget only shows up under the full ~200-unit load,
which Phase 1 deliberately does not have ([`phase-1-plan.md`](plans/phase-1-plan.md) §8). So the
dual-rate re-evaluation belongs to **Phase 3** (200-unit stress + thermal profiling on target
hardware — [`roadmap.md`](roadmap.md)), not here, and is not a reason to add a second clock now.

**Consequences:**
- [Q10](open-questions.md) **closes**; `TICK_HZ = 60` is the Phase-1 lock.
- **Phase 3** owns the 200-unit thermal re-evaluation that could reopen a dual-rate split; the
  named constant keeps the door open for a cheap re-tune or split if profiling demands it.
- No invariant changes: the sim stays **fixed-point** (invariant #1) and **decoupled** from
  render (invariant #4) at whatever rate; a faster tick admits no floats.

---

## D22 — Phase 1 vertical slice PASSED on real arm64; custom Rust engine validated, fallback retired

**Resolves:** the Phase 1 exit criterion ([`roadmap.md`](roadmap.md), [`phase-1-plan.md`](plans/phase-1-plan.md)) and the build-cost de-risk bet of [D8](#d8--pre-production-is-design-only-engine-direction-is-custom-native-with-a-live-fallback).

**Decision:** **Phase 1 is complete.** The custom **Rust** engine ([D10](#d10--engine-language-rust)) is validated end-to-end on real hardware, so the **Unity DOTS / Godot+GDExtension fallback (D8) is retired** and the two throwaway Godot prototypes (`prototypes/phase0-controls` → D14, `prototypes/phase0.5-netfeel` → D15) are **deleted**. Phase 2 (game systems) begins.

**Evidence (Galaxy S24, Adreno 750, `aarch64-linux-android`):**
- **Determinism — bit-identical to desktop.** `pnpm android:checksum` ran the headless
  `sim-runner` on-device and diffed its per-tick checksum stream against the x86_64 desktop
  run over 300 ticks: **identical** (final `4c34c6b5951edf57`). The fixed-point sim is
  bit-identical across arch on real silicon (invariant #1/#7), on top of the CI matrix that
  already covers x86_64 win/linux + aarch64 darwin + native arm64 linux.
- **Commandable + embodiable on device.** One unit moves via the flow field; tap-to-move
  works; a (provisional) two-finger-tap embody toggle flips the world dark (invariant #5).
- **At target frame rate.** The on-device FPS heartbeat showed a sustained **120 fps** (the
  panel rate) with the sim on its locked **60 Hz** tick ([D21](#d21)) — frames advancing
  ~120/s while ticks advance ~60/s, demonstrating the sim/render decoupling (invariant #4)
  live on hardware.

**Why retire the fallback now:** D8 kept Unity/Godot live *only* until the slice was validated
on real hardware — that condition is now met. The slice proves the load-bearing risks the
fallback hedged (determinism on real arm64, the embodiment model, the PAL boundary, that a
custom native engine runs at frame rate on a phone). Carrying the fallback further is dead
weight.

**Scope / honest caveats (NOT blockers — they are later phases by design):**
- Validated on a **flagship** (S24), not a mid-range device. Determinism is arch-level and
  therefore device-independent (a mid-range chip yields the identical checksums by
  construction); **frame-rate/thermal headroom on mid-range silicon, and the 200-unit power
  budget, are explicitly Phase 3** ([D21] defers the global-60-vs-dual-rate re-evaluation to
  the 200-unit thermal projection). One unit at 120 fps has enormous headroom regardless.
- The Android control scheme is a **provisional dev binding** (two-finger embody); the shipped
  mobile controls (sticks/gyro) are a Phase 2 design call. iOS (`pal-ios`) is still sequenced
  last (D9) and was never required for the slice.

**Consequences:**
- [`roadmap.md`](roadmap.md) marks **Phase 1 done**; the "keep the fallback live" guidance is
  retired; **Phase 2 (game systems)** is the active phase.
- The `prototypes/` tree and its `phase0:*` task-runner scripts are removed; docs that pointed
  at the prototypes are updated to past tense (their decision records D14/D15 stand as history).
- D8's "live fallback" clause is superseded; the custom-native direction is now committed.

---

## D23 — Phase 2 game systems: the deterministic model and its module decomposition

**Status:** a first, fully-deterministic implementation of the Phase 2 (game-systems) bullets
from [`roadmap.md`](roadmap.md) has landed in `core` — combat/suppression/cover/line-of-sight,
territory capture, resources/economy/camps + production, fog of war, the order/stance
vocabulary with a literal-executor and a retreat trigger, and an alert channel. It is the
game-systems *spine*, not the balanced/host-wired finished game (see **Deferred** below).

**Decision:**

1. **Factions are a component, not a player object.** A new `components::Faction`
   (`Player`/`Enemy`/`Neutral`) plus `EntityKind` (`Unit`/`Building`) tag every entity. Combat
   engages only across distinct, non-neutral factions; resources and territory income are
   per-faction. This keeps everything in the one SoA `World` (no parallel player graphs).

2. **Each system is a pure function over the `World`, run in one fixed order per tick**
   (`Sim::step`): **orders → combat → territory → economy**. The order is arbitrary but
   *fixed* (determinism), and chosen so movement resolves before fire, capture is counted on
   post-combat survivors, and production/income closes the tick. (Later widened to **move →
   collide → orient → fight → capture → economy** as D55 added `orient` and
   [D57](#d57--buildings-are-solid-a-fixed-point-footprint-push-out-in-the-sim-step) added
   `collide`; see those entries for the canonical order.) New modules:
   `core::{terrain, combat, economy, territory, fog, orders, alerts, event}`.

3. **Combat is fixed-point hitscan with cover, suppression, and literal stances.** Units
   acquire a target by stance (`HoldFire` never; `ReturnFire` only its `last_attacker`;
   `FireAtWill` the nearest enemy, ties to the lowest index), within weapon range (squared
   compare — never a sqrt) and `terrain` line of sight, fire on cooldown for cover-mitigated
   damage, accumulate **suppression** (which pins firing and, in `orders`, slows movement),
   and despawn at zero health. The literal-executor rule (invariant #3) holds: combat acts on
   the stance the player set, it never invents targets or chases.

4. **Terrain is a static cover grid sharing `flow_field`'s exact cell mapping.** `Cover`
   (`None`/`Light`/`Heavy`) gives a damage multiplier; `Heavy` also blocks sight. Line of
   sight is an integer supercover DDA, **symmetric by construction** (the endpoint pair is
   canonicalised) and corner-tight (a wall corner cannot be peeked through). Terrain is set
   once and never mutated, so it is **not** in the checksum.

5. **Territory, resources, economy, and camps ARE checksummed sim state.** Control points
   capture to the sole contesting faction (contested → stalls); held points feed integer
   per-tick income; camps build over time, upgrade, and run a FIFO production queue that spawns
   units from a fixed `UnitKind` stat table (so every peer spawns the bit-identical unit).
   Production spawns are deferred to a second pass so `world.spawn()` never reallocates under a
   live index loop.

6. **The order vocabulary is where the depth lives (game-design §8).** `Order` widens to
   `Idle`/`MoveTo`/`AttackMove`/`Patrol{a,b}`/`HoldPosition`/`FallBack`. The **retreat trigger**
   (`retreat_below` fraction) lets a player pre-program "fall back below X% HP" — the unit's
   order is *replaced* with a terminal `FallBack(rally)` (rally = nearest friendly building, else
   origin); the unit never decides for itself. Movement is the one shared
   `systems::step_toward(_speed)`; for an unsuppressed unit `MoveTo`/`Idle` is bit-identical to
   the Phase 1 mover (the old `movement_system` is deleted as dead code).

7. **Fog of war and alerts are pure, presentation-side derivations — NOT sim state.** Per the
   netcode design ([`architecture.md`](architecture.md) §Netcode) every client holds the full
   world, so fog is a client-side filter: `fog::{command_visibility, embodied_visibility}` are
   pure functions over `World`+`terrain` that never mutate sim state and are excluded from the
   checksum. `embodied_visibility` is the vision half of "the world goes dark" (invariant #6).
   The `alerts` channel turns the per-tick `event::SimEvent` stream into directional alerts
   ("alerts, not intel", game-design §6) for the embodied HUD — also excluded from the checksum.

8. **The per-tick checksum (invariant #7) folds all new sim state, and the RNG state.** Every
   new per-entity component, plus per-faction resources and the territory points, is folded;
   the RNG `(state, inc)` is now folded too so a divergence in the *number* of draws surfaces
   immediately rather than only through its downstream effect. The headless `sim-runner` CI
   scenario was rebuilt to exercise combat, territory, economy, and the order vocabulary, so
   the cross-arch determinism matrix actually covers Phase 2.

**Why:** the load-bearing risk in Phase 2 is the same as Phase 1 — a float or an unstable
iteration leaking into the now-much-larger sim desyncs lockstep **silently** (invariants #1,
#7). Making *every* system a pure, fixed-point, index-ordered function over one SoA world,
with one fixed system order and a checksum that folds everything that mutates, keeps the
determinism guarantee **structural** rather than a thing to remember — the same principle as
D17/D18. Splitting fog/alerts out as pure derivations (not sim state) is the other half: it
keeps the client-side "world goes dark" presentation rule from ever touching the
competitive-integrity boundary the netcode owns, so it cannot desync. The work was built
behind a frozen cross-module contract so the systems could be implemented in parallel without
any one of them editing the shared determinism glue (`ecs`/`sim`/`checksum`).

**What this does NOT decide (open questions stay open):** `fog`/`alerts` are implemented as a
**mechanism**, deliberately not settling **how thin the thread back is** ([Q1](open-questions.md))
or **whether the enemy can tell you've gone dark** ([Q2](open-questions.md)); possession remains
unconstrained, not settling the **leash** question ([Q3](open-questions.md)). The current
"alerts-only" lean is the mechanism shipped, not a lock.

**Consequences:**
- `core` gains eight modules and ~nine new per-entity component arrays; its dependency list
  stays **empty** (invariant #2 tripwire armed) and it is `f32`/`f64`-free (invariant #1).
- The suite grew from 57 to 128 `core` tests, green in **both** dev and release profiles; the
  `sim-runner` stream is bit-identical dev↔release (a determinism check across overflow-check
  profiles). The Phase 1 checksum literal recorded in [D22](#d22--phase-1-vertical-slice-passed-on-real-arm64-custom-rust-engine-validated-fallback-retired) is now historical — the new
  `sim-runner` scenario produces a new value; what the matrix asserts is cross-arch *equality*,
  not a fixed literal.
- `World` gains an O(1) `entity(i)` accessor (an index→handle inverse the systems need to put a
  real generational handle into an event/`last_attacker`).
- [`roadmap.md`](roadmap.md) Phase 2 is updated to reflect the systems-code landing with its
  honest caveats; [`README.md`](../README.md) repo-map notes the new `core` systems.

**Deferred (honest scope — NOT done here):** host/presentation wiring of fog rendering, the
alert HUD, and the embodied audio mix; the shipping touch UI for multi-unit selection and the
order/stance vocabulary on a small screen; gameplay **balance** of the cost/time/damage tables
(values are placeholders chosen for testable behavior, not tuned); and the netcode/lockstep
layer itself (Phase 3). Avatar-local prediction ([D15](#d15--embodied-combat-over-lockstep-avatar-local-prediction-phase-05-passes)) is still the Phase 1 stub.

---

## D24 — Phase 2 host wiring: fog/HUD/audio/touch-UI behind a frozen presentation contract

**Status:** the host/presentation wiring [D23](#d23--phase-2-game-systems-the-deterministic-model-and-its-module-decomposition)
deferred has landed — fog rendering, the embodied alert HUD, the embodied audio mix, and the
shipping touch UI (multi-unit selection + the order/stance vocabulary on screen). All four are
**pure presentation derivations** over the existing deterministic `core`; none is sim state, and
none mutates the sim.

**Decision:**

1. **Built behind a frozen cross-crate contract — the same technique as [D23](#d23--phase-2-game-systems-the-deterministic-model-and-its-module-decomposition).**
   The shared glue (the PAL seam, the `engine` frame loop, the renderer) was extended and committed
   first so the four subsystems could be filled in **parallel** without any one of them editing the
   determinism-adjacent glue. The contract adds: `pal::InputFrame` touch intents (`pointer_up`,
   `long_press`, `command_slot`) and `pal::Audio::submit_mix` + `AudioCue`/`SoundId`; `engine::Game`
   driving fog/alerts/audio/selection/command-UI in `frame` (which now takes a `&mut dyn Audio`);
   and `render` choosing its draw set through a fog filter plus a HUD `LOAD` pass. Each subsystem
   lives in a **disjoint leaf module** — `render::{fog, hud}` (+ `hud.wgsl`),
   `engine::{audio, selection, command_ui}`.

2. **Fog rendering** (`render::fog::visible_instances`) applies `core::fog` visibility at the float
   boundary. Embodied → the map collapses to the avatar's own sight (the avatar always draws);
   command view → fog of war hides units/buildings outside the player's union vision, while control
   points stay drawn as known objective markers (invariant #6).

3. **Alert HUD** (`render::hud`) is a separate screen-space, alpha-blended `LOAD` pass: one
   directional marker per recent alert, placed by bearing relative to the avatar's yaw and faded
   over a 120-tick (~2 s) window. The thin thread back — *alerts, not intel* (invariant #6). The
   pure placement/fade math (`marker_for`) is unit-tested; `hud.wgsl` is naga-validated.

4. **Embodied audio mix** (`engine::audio::mix_cues`) turns the per-tick `SimEvent` stream into
   positioned `AudioCue`s: event → `SoundId`, `azimuth = yaw - world_bearing` (0 = ahead, + = right,
   the *same* right-handed convention as the HUD), `1/(1+dist/FALLOFF)` attenuation, and a `muffled`
   flag for strategic sound bleeding into the embodied view. The **mix** (which sounds, where, how
   loud, what's ducked) is the system and is platform-free + tested; the actual output path stays a
   per-backend no-op sink for now (real AAudio / desktop output is plumbing, not the system, and is
   left without pulling an audio crate). **(Superseded — desktop `cpal` output landed in
   [D26](#d26--phase-2-polish-round-real-opt-in-audio-output-a-selection-highlight-and-a-first-pass-balance-baseline),
   the Android `oboe`/AAudio sink in [D29](#d29--android-audio-sink-oboe-low-latency-aaudio-mixing-through-a-shared-host-tested-seam).)**

5. **Touch UI** is two pure layers: `engine::selection` (command-view tap-pick / drag-band select;
   presentation state only, a no-op while embodied) and `engine::command_ui` (the on-screen
   vocabulary slot → `Move`/`AttackMove`/`SetStance` for the selection, world target quantized via
   `world_to_fixed` at the input boundary). The desktop backend binds the new intents to
   left-release / `F` / number keys `1`–`6` for dev iteration.

**Why:** the Phase 2 risk is unchanged from D23 — anything that lets the presentation layer feed
back into the sim desyncs lockstep **silently** (invariants #1, #4, #7). Keeping every new subsystem
a read-only derivation that only *reads* the world/events and emits pixels, audio cues, or
`Command`s the sim already accepts makes that guarantee **structural**: the headless `sim-runner`
checksum stream is **byte-identical** before and after this work (`8cfc2b25ab17a128`) and dev ==
release. The frozen-contract technique kept five parallel workstreams off the shared determinism
glue, exactly as D23.

**What this does NOT decide (open questions stay open):** fog and alerts ship as a **mechanism** —
the current alerts-only thinness ([Q1](open-questions.md)), the "enemy can't tell you're dark"
posture ([Q2](open-questions.md)), and the avatar-only dark are an *implementation*, not a lock; the
touch UX (gesture grammar, slot layout) is a working scheme, not settled design. ~~`Patrol`/
`HoldPosition`/`FallBack` exist as sim `Order`s but have no `Command` to set them — exposing them
through the command UI is a small, determinism-sensitive `core`-surface follow-up.~~ **(Correction,
[D25](#d25--the-orderstance-command-vocabulary-was-already-in-the-sim-touch-ui-now-reaches-it-corrects-d24)):
this was wrong — `Command::SetOrder`/`SetRetreatThreshold` already exist in `core::sim` and are
already checksummed; only the touch vocabulary hadn't wired them. D25 does, presentation-only.**
Gameplay **balance** and the netcode/lockstep layer remain as deferred in D23.

**Consequences:**
- `render` gains `fog`/`hud` modules (+ `hud.wgsl`); `engine` gains `audio`/`selection`/
  `command_ui`; `pal` gains the touch intents + `AudioCue`/`SoundId`. `core` is **untouched** (its
  dependency list stays empty; no float entered the sim).
- The suite grew **149 → 190** tests (fog 5, hud 11, audio 10, selection 8, command_ui 7), green in
  **both** dev and release; `clippy -D warnings` is clean; `aarch64-linux-android` type-checks.
- A `code-reviewer` pass caught and fixed an **inverted audio azimuth sign** (it disagreed with the
  HUD's bearing convention and the cue contract) before the commit — a sound from the right would
  otherwise have panned left.
- [`roadmap.md`](roadmap.md) Phase 2 status and [`README.md`](../README.md) repo-map are updated;
  balance + netcode remain the open Phase 2 / Phase 3 items.

---

## D25 — The order/stance command vocabulary was already in the sim; touch UI now reaches it (corrects D24)

**Status:** the touch UI's command vocabulary now issues the full Phase-2 order set —
`HoldPosition`, `Patrol`, `FallBack`, and the retreat trigger — through the sim's **existing**
`Command::SetOrder` / `Command::SetRetreatThreshold`. This **corrects a factual error in
[D24](#d24--phase-2-host-wiring-foghudaudiotouch-ui-behind-a-frozen-presentation-contract)**: that
entry claimed these orders had "no `Command`" and that exposing them needed a `core`-surface
change. That was wrong — the commands already existed in `core::sim::Command`, are handled in
`Sim::apply`, and were already folded into the per-tick checksum (`write_order` serializes
`Patrol`/`HoldPosition`/`FallBack`). The follow-up was therefore **presentation-only**.

**Decision:**

1. `engine::command_ui` gains slots **5–9**: `HoldPosition`; `Patrol` (leg `a` = the unit's CURRENT
   position, leg `b` = the tapped point — so each selected unit patrols from where it stood to the
   tap); `FallBack` to the tapped rally; and **arm/disarm the retreat trigger**
   (`SetRetreatThreshold` at a placeholder **30%** / `0`). `commands_for` now takes the selection as
   `(handle, world_pos)` pairs so Patrol can anchor a per-unit leg; every world coordinate — the tap
   *and* each unit's leg `a` — is quantized via `world_to_fixed` at the input boundary (invariant #1).
2. The desktop backend maps number keys `7`/`8`/`9`/`0` → slots `6`/`7`/`8`/`9` for dev iteration
   (joining the existing `1`–`6` → `0`–`5`).
3. A `core` test now directly covers `SetOrder` + `SetRetreatThreshold` application — the command
   surface had no direct test before this.

**Why:** the depth layer (game-design §8) must be *reachable* by the player or it isn't real, and
the only thing missing was the UI mapping — not a sim change. Keeping it presentation-only means the
D23/D24 guarantee holds unchanged: nothing here mutates sim state, the `sim-runner` checksum stream
is **byte-identical** (`8cfc2b25ab17a128`), and dev == release. The earlier mis-scoping (treating
this as a determinism-sensitive `core` change) was a false alarm from an incomplete read of the
`Command` enum; the lesson recorded here is to grep the *whole* enum before declaring a surface gap.

**What this does NOT decide:** the **30%** retreat default and the slot numbering/layout are a
working mechanism, not tuned/settled (balance stays open, per D23/D24); `long_press` remains a
reserved no-op gate for a future context/radial menu.

**Consequences:**
- No `core` logic change — only a test added; `core` stays float-free with an empty dependency list.
- The suite grew **190 → 195** tests (the new `core` command test + the expanded `command_ui` slot
  coverage), green in **both** dev and release; `clippy -D warnings` clean; `aarch64-linux-android`
  type-checks; `code-reviewer` clean.
- [D24](#d24--phase-2-host-wiring-foghudaudiotouch-ui-behind-a-frozen-presentation-contract)'s
  "no `Command`" caveat is struck through with a pointer here; [`roadmap.md`](roadmap.md) drops that
  caveat from the Phase 2 status. Balance and the netcode/lockstep layer remain the open items.

---

## D26 — Phase 2 polish round: real (opt-in) audio output, a selection highlight, and a first-pass balance baseline

**Status:** three follow-ups closing the gaps left after the [D24](#d24--phase-2-host-wiring-foghudaudiotouch-ui-behind-a-frozen-presentation-contract)
host wiring landed. All are presentation/tuning; the deterministic sim model is unchanged.

**Decision:**

1. **Desktop audio output is real, but opt-in.** `engine::audio::mix_cues` already computed the
   positioned cues; `pal-desktop`'s `DesktopAudio` now *renders* them through a `cpal` output
   stream — **procedural per-`SoundId` synthesis** (no audio assets yet: a noise burst for gunfire,
   a falling tone for a unit lost, a low thud for a base hit, a rising chime for a capture, a blip
   for production), equal-power panned by `azimuth`, scaled by `gain`, one-pole low-passed when
   `muffled` (the off-map strategic bleed, invariant #6). It lives behind a default-OFF `audio`
   cargo feature: a bare build / clone-and-run pulls **no** audio system libs (invariant #8);
   enabling it pulls `cpal` → links ALSA (`alsa-lib-devel`). Without the feature `DesktopAudio` is
   a silent no-op, and any device/stream failure degrades to silent rather than panicking — audio
   is never load-bearing. Run with `pnpm play:audio`.

2. **Command-layer selection is now visible.** Selection state existed but nothing drew it. The
   renderer gains `FLAG_SELECTED`; `UnitSnapshot` carries its world `entity_index` (presentation
   data, not sim state, not checksummed) so the renderer can rim selected units in bright white.
   The rim is a *command-view* affordance — the engine passes no selection while embodied.

3. **A first-pass balance baseline.** The combat lethality (weapon damage halved — troops were
   deleting each other on contact) and the economy tables (camp/unit costs, build/production
   times, territory income, camp HP, upgrades) are tuned into an internally-coherent baseline,
   reasoned in seconds at 60 Hz against the demo's 500-resource seed. It is **explicitly a playtest
   baseline, NOT a locked design** — the numbers are expected to move once real playtests exist.

**Why:** these were the honest "NOT done" items after D24. Keeping audio opt-in preserves the
zero-setup local build (invariant #8) while making the embodied mix audible on demand; the
selection rim is pure render state (invariant #4 — no sim mutation); and the balance pass is a
sim-input change only — determinism is untouched (dev == release; the per-tick checksum changes,
which is correct for a balance change — the matrix asserts cross-arch *equality*, not a fixed
literal). A new headless **offscreen render harness** (`viz-runner`, `pnpm desktop:viz`) renders
the real `Game` to a texture and asserts the command view draws, embodiment goes dark, the alert
HUD draws, and the selection rim shows — so these presentation behaviors are now checked with
actual pixels, not just unit tests (it needs a GPU, so it is local-only, not CI).

**What this does NOT decide:** the procedural sounds are placeholders (real audio assets/design
are later); the balance numbers are a baseline, not tuned; Android's AAudio sink is still a
documented no-op. Q1/Q2/Q3 remain open. **(Superseded in part: the AAudio sink is now real —
[D29](#d29--android-audio-sink-oboe-low-latency-aaudio-mixing-through-a-shared-host-tested-seam);
the balance baseline was measured into [D30](#d30--a-measured-combateconomy-balance-baseline--a-deterministic-balance-metrics-harness).)**

**Consequences:**
- `pal-desktop` gains an optional `cpal` dep behind the `audio` feature; `app` forwards it; the
  default workspace build/clippy stay header-free (no `cpal`). `core` is untouched by audio +
  selection; the balance change is confined to `core::economy`/the lethality table.
- Tests: combat/economy balance + the selection rim are covered; the `viz-runner` scenarios grew
  to include `selected`. Full suite green dev + release; `clippy -D warnings` clean (default and
  `--features audio`); `aarch64-linux-android` still type-checks.
- [`roadmap.md`](roadmap.md) Phase 2 caveats updated (audio output done/opt-in; selection visible;
  balance has a baseline); [`README.md`](../README.md) repo-map notes the audio feature + the
  selection rim.

---

## D27 — Netcode topology: deterministic lockstep in `core`, transport behind a PAL trait

**Status:** topology decided, and the **first slice has landed** — `core::lockstep` (the
in-process deterministic 2-client loop + wire codec, no sockets;
[`phase-3-plan.md`](plans/phase-3-plan.md) §"Workstream B" step 1). This entry fixed *where each piece
lives* before the wire code, exactly as [D19](#d19--the-gpu-device-crosses-into-the-renderer-at-the-concrete-wiring-layer-not-through-the-abstract-pal-trait)/[D20](#d20--the-platform-agnostic-game-loop-is-a-shared-engine-crate-both-hosts-drive)
fixed the PAL boundary before the renderer. The deterministic substrate it builds on already
exists: `core::sim::Command` is the lockstep "order" unit (`Copy`, float-free), `Sim::step(&[Command])`
already applies a per-tick command set in stable order, and `Sim::checksum` already folds all sim
state incl. the RNG ([D23](#d23--phase-2-game-systems-the-deterministic-model-and-its-module-decomposition)).

**Refinement from the implementation (qualifies bullet 2 below):** `core::lockstep` is
**sans-I/O** — it *produces* opaque byte frames (`Lockstep::drain_outbound`) and *consumes*
received ones (`Lockstep::deliver`), but does no transport and holds **no** `&mut dyn Transport`.
The host (a `pal::Transport` impl) moves the bytes. This is strictly better than a trait object in
`core`: it keeps `core` from depending on `pal` at all (the empty-dep tripwire stays armed), and
makes the whole protocol testable in-process against a simulated lossy channel with zero sockets —
which is exactly how the landed slice is verified.

**Decision:**

1. **The lockstep loop and the wire codec live in `core`, in a new platform-free
   `core::lockstep` module.** It deals only in already-`core` types — `Command`, `tick`,
   `checksum` — so `core`'s dependency list stays **empty** (invariant #2 tripwire armed). It owns:
   the **command-delay buffer** (an input sampled at tick `T` is stamped to execute at `T + D`),
   the **per-tick command-set assembly** (merge every peer's commands for tick `T` in a **fixed
   peer order**, preserving the stable application order `Sim::apply` already guarantees), the
   **gate/stall** (advance the sim only when every peer's slot for `T` is present; an empty slot is
   the explicit "I have nothing, proceed" signal, so quiet ticks don't stall), and the **wire
   codec** (serialize `Command`/tick/checksum to bytes, reusing the little-endian discipline of
   `core::checksum` so the bytes are byte-identical across arch — itself a determinism concern).

2. **Transport is abstracted behind a new `pal::Transport` trait — opaque byte frames, no
   socket type in the signature.** It mirrors `pal::Audio` exactly: an abstract seam in `pal`
   (`fn send(&mut self, &[u8])` / `fn poll(&mut self) -> Vec<Vec<u8>>`-shaped), named after *what*
   not *how*. `core::lockstep` consumes a `&mut dyn Transport`; it never names UDP/QUIC/a socket.

3. **Concrete transport lives in the platform/infra layer — `pal-desktop` and `server`.** A
   loopback/in-process impl in `pal-desktop` for dev; real sockets, matchmaking, and relay in
   `server`. The boundary rule (the load-bearing one): **the transport never understands a
   `Command`; `core` never understands a socket.** This is the precise analogue of D19's "the
   device crosses at the concrete wiring layer."

4. **Avatar-local prediction ([D15](#d15--embodied-combat-over-lockstep-avatar-local-prediction-phase-05-passes)) stays in the `engine` presentation path — reaffirmed, not
   moved.** Prediction lives in new `engine::Game` fields (parallel to the existing `yaw`), reads
   the snapshot/world by shared reference, reconciles against the authoritative tick, and **never**
   takes `&mut Sim` (invariants #4/#5; the determinism-checklist item "avatar prediction never
   writes sim state"). It is not part of `core::lockstep`.

5. **Lockstep is testable without sockets, and a desync is a CI failure.** A seeded in-process
   `Transport` double (latency/jitter/reorder/loss driven by `core::rng`, so the test is itself
   deterministic) drives a **two-instance lockstep test** asserting both peers agree on every
   per-tick checksum and match a no-network single run. CI gains a new headless `net-sim-runner`
   job that runs this across the existing arch matrix — **ADD-ONLY** to `determinism.yml`; a
   cross-client desync is a real bug, never silenced by narrowing the matrix (invariant #7).

**Why:** the load-bearing risk is unchanged from D17/D18/D23 — a platform detail or an unstable
ordering leaking into the deterministic sim desyncs lockstep **silently** (invariants #1, #2, #7).
Putting the loop + wire codec in `core` and the sockets behind a `pal` trait makes that boundary
**structural**: the type system forbids `wgpu`/`winit`/a socket from reaching the sim, and a
desync surfaces as a checksum mismatch rather than a heisenbug. It reuses the proven D19/D20 PAL
pattern (abstract in `pal`, concrete in `pal-desktop`/`server`) rather than inventing a new seam.
Building and validating against the in-process double *first* puts the cheapest-to-be-wrong code
(the protocol) in the cheapest place to test it, before real sockets are added.

**What this does NOT decide (deliberately left open):**
- **The concrete transport (UDP vs QUIC).** The **lean is QUIC** because its connection migration
  survives a Wi-Fi↔cellular handoff without a full reconnect ([`architecture.md`](architecture.md)
  §Netcode gotcha) — but that is a transport-layer choice for a later entry, and it sits entirely
  behind `pal::Transport`, so it changes nothing in `core`.
- **The reconnect/snapshot serialization format** — its own forthcoming `Dn` (workstream C); it
  consumes `core::lockstep`'s command buffer but is a separate concern.
- **Dynamic input-delay tuning, and the stalled-peer recovery policy** (drop / AI-substitute,
  `architecture.md` §Netcode) — protocol details deferred to the implementation slices.
- **Matchmaker / accounts / relay service split** ([Q9](open-questions.md)) — untouched.
- It does **not** reopen the tick model: `core::lockstep` is parameterized on `sim::TICK_HZ`, so a
  future dual-rate split ([D21](#d21--sim-rate-a-single-global-60-hz-tick-for-phase-1-dual-rate-deferred-not-killed) re-evaluation) would not change this topology.

**Consequences:**
- New `core::lockstep` module (platform-free; `core` deps stay empty) and a new `pal::Transport`
  trait with concrete impls in `pal-desktop`/`server`. The implementation lands incrementally,
  each slice under `/safe-edit`, per [`phase-3-plan.md`](plans/phase-3-plan.md) §"Workstream B".
- [`architecture.md`](architecture.md) §Netcode flips from design prose to **decided** (topology),
  referencing this entry; [`README.md`](../README.md) repo-map will note `core::lockstep` and
  `pal::Transport` when the first slice lands (not before — they do not exist yet).
- `determinism.yml` will gain an ADD-ONLY networked-checksum job; the existing single-client
  matrix is never narrowed.

---

## D28 — Authoritative snapshot format: a hand-rolled LE serialization sharing the checksum walk

**Status:** format decided; **no code yet.** This entry fixes the *serialization format* for
an authoritative, bit-identical-resume snapshot before the wire/persistence code is written —
exactly as [D27](#d27--netcode-topology-deterministic-lockstep-in-core-transport-behind-a-pal-trait)
fixed the netcode topology before the lockstep code. It opens Phase 3 workstream C
([`phase-3-plan.md`](plans/phase-3-plan.md) §"Workstream C — Reconnect / snapshot / handoff"). The
first code slice (`core::persist` + `Sim::serialize`/`deserialize` + `Rng::from_state` + the
round-trip-replay test) is forthcoming under `/safe-edit`, **not** landed here.

**The two-snapshots distinction is the whole reason this exists.** [`core::snapshot`](../core/src/snapshot.rs)
is the **render** snapshot: lossy by design — alive units only, `health.fraction()` collapsing
`cur`/`max`, no RNG, no free-list, no dead slots — taken for interpolation (invariant #4) and
deliberately **not** checksummed. It is **unfit for resume**: deserializing it could never
reproduce the exact world the checksum hashes, so a peer rebuilt from it would desync on the
next tick. D28 defines a *second*, **authoritative** serialization: every bit needed to resume
a peer such that its checksum stream stays **bit-identical** to a never-interrupted run.

**Decision:**

1. **A new authoritative serialization, distinct from the render snapshot.** It captures the
   exact deterministic state `Sim::checksum` hashes — not a presentation copy. Render snapshot =
   lossy / interpolation-only / not-for-resume; authoritative snapshot = complete / byte-exact /
   the only thing a reconnecting peer may resume from. The two never share a type.

2. **Format: a hand-rolled little-endian `Writer`/`Reader`, generalizing the existing
   `core::checksum` byte discipline — no serde/bincode in `core`.** The `Writer` emits the same
   LE byte stream `Checksum` already folds (`write_u8`/`i32`/`u32`/`u64` → `to_le_bytes`); the
   `Reader` is its exact inverse. This keeps `core`'s dependency list **empty** (invariant #2
   tripwire armed) — pulling serde/bincode would put a determinism-critical, version-sensitive
   dependency in the sim's resume path for zero benefit, since the byte discipline is already
   written and proven. **`Fixed` crosses as `to_bits()` / `from_bits()`, never as a float**
   (invariant #1) — identical to how the checksum and the `core::lockstep` wire codec
   ([D27](#d27--netcode-topology-deterministic-lockstep-in-core-transport-behind-a-pal-trait))
   already treat it. The `Reader` rejects malformed input (bad length / trailing bytes / unknown
   tag) rather than silently producing a divergent world, mirroring the lockstep codec's
   never-panic decode.

3. **What is captured** (enumerated from the code; the *why* given for the non-obvious ones):
   - **Every `World` component array — including dead slots.** The checksum already walks
     `0..world.capacity()`, alive or not; the snapshot walks the same range so a deserialized
     world has byte-identical component arrays, not just identical *live* entities.
   - **The liveness triple — `generation` / `alive` / `free` (the free-list, in order).** This is
     the subtle one: `World::spawn` pops the **free list** to reuse a slot, so **free-list order
     decides which slot the next spawn lands in**. Serialize it in the wrong order and the very
     next production spawn picks a different slot on the resumed peer than on the others — an
     **instant desync**. The free list is sim state, not a derivable cache.
   - **`Resources`** (per-faction, in fixed `Faction::ALL` order) and **`Territory`** (control
     points, stable vector order) — both already checksummed, both required to resume income and
     capture state.
   - **`Rng(state, inc)` — flagged as the single most important non-obvious field.** Omit it and
     the resumed peer's PRNG stream diverges by exactly the draws that happened before the
     snapshot: a **guaranteed draw-count divergence**, the classic lockstep desync the checksum's
     RNG fold ([D23](#d23--phase-2-game-systems-the-deterministic-model-and-its-module-decomposition))
     exists to catch. The first code slice adds a `Rng::from_state(state, inc)` reconstructor
     (paired with the existing read-only `checksum_state`) so the generator round-trips exactly.
   - **`tick`** — the resume clock; `cmds[T..]` must replay from the right `T`.
   - **Excluded: `events`** — the per-tick `SimEvent` stream is **transient** (cleared at the top
     of every `step`, never checksummed); it is regenerated by the next tick and must not be
     serialized.
   - **Terrain → serialize a `map_id`, not the grid.** `Terrain` is **static map data**, set once
     at scenario build and never mutated by a system (which is exactly why it is *not* in the
     checksum). Serializing the `GRID×GRID` cell array would bloat every snapshot with constant
     data; instead the snapshot carries a small `map_id` and the resuming peer rebuilds the same
     terrain from it. (Both peers already agree on the map out-of-band; the snapshot only needs to
     name it.)

4. **Structural safeguard: `Sim::checksum` and `Sim::serialize` share one field-walk.** Refactor
   the field traversal into a single generic walk (e.g. `fn fold<S: StateSink>(&self, sink: &mut S)`)
   that both a `Checksum` sink and a `Writer` sink drive. Then **anything added to the checksum is
   serialized for free**, and the two can never silently drift — a new component that gets
   checksummed-but-not-serialized (or vice versa) becomes structurally impossible rather than a
   thing to remember. This is the same "make the guarantee structural, don't rely on memory"
   principle as [D17](#d17--fixed-point-sim-scalar-a-hand-rolled-q1616-fixed-newtype)/[D18](#d18--ecs-storage-hand-rolled-struct-of-arrays-not-an-off-the-shelf-ecs)/[D23](#d23--phase-2-game-systems-the-deterministic-model-and-its-module-decomposition).
   **This refactor of `Sim::checksum` is the one determinism-sensitive change** in workstream C —
   the checksum is the lockstep tripwire itself — so it lands under `/safe-edit` with the
   `sim-runner` stream verified byte-identical before/after the refactor.

5. **The headline invariant test (the load-bearing guard): serialize → deserialize → replay is
   bit-identical.** serialize@`T` → deserialize → replay `cmds[T..L]` through a plain `step` loop
   yields a checksum stream **bit-identical** to the never-interrupted run, on **every arch**.
   Because the test lives in `core`'s test module, it rides the existing determinism matrix
   ([`determinism.yml`](../.github/workflows/determinism.yml)) automatically — no new CI job
   needed for the format itself (invariant #7). Once this round-trip holds, **reconnect = snapshot
   + replay-buffered-commands** (the lockstep command buffer from D27 supplies `cmds[T..L]`),
   correct **by construction** — there is no separate reconnect algorithm to get wrong.

**Why:** the load-bearing risk is identical to every prior determinism decision — a missing or
mis-ordered field leaking into a resumed world desyncs lockstep **silently** (invariants #1, #7).
A reconnecting peer that is even one free-list slot or one RNG draw off computes a different world
on its first tick back, with no error — just divergence. Making the snapshot capture *exactly*
what the checksum hashes, through *one shared walk*, makes "the snapshot is complete" a
**structural** property rather than a checklist: the checksum is already the authority on what sim
state *is*, so binding serialization to it means the resume path inherits that authority for free.
Owning a hand-rolled LE codec (rather than serde) keeps the empty-dep guarantee and reuses the
byte discipline already validated by the checksum and the lockstep wire codec — the same "own the
load-bearing thing" call as D17/D18/D27. And expressing correctness as a single round-trip-replay
test that rides the arch matrix means a format regression fails CI the same way a sim desync does.

**What this does NOT decide (deliberately left open):**
- **The on-wire transport for shipping a snapshot.** Moving the serialized bytes from peer to peer
  sits entirely behind [`pal::Transport`](#d27--netcode-topology-deterministic-lockstep-in-core-transport-behind-a-pal-trait)
  (D27) — `core` produces/consumes opaque bytes and never names a socket. QUIC's connection
  migration (the Wi-Fi↔cellular input to the D27 transport lean) is a transport concern, not a
  format one.
- **The reconnect *policy* / handoff specifics** — when to snapshot, how far back the command
  buffer must reach, the stalled-peer recovery choice (drop / AI-substitute, `architecture.md`
  §Netcode), and the Wi-Fi↔cellular handoff pause behavior. D28 decides only the *format* and the
  *round-trip invariant*; the policy that drives it is a later workstream-C concern.
- **Snapshot cadence, versioning across game updates, and on-disk save persistence** — beyond the
  format + round-trip, untouched here.

**Consequences:**
- A forthcoming first slice adds `core::persist` (the `Writer`/`Reader`), `Sim::serialize`/
  `deserialize`, `Rng::from_state`, the shared `fold<S: StateSink>` walk (refactoring
  `Sim::checksum` onto it), and the round-trip-replay determinism test — all under `/safe-edit`,
  `core` deps staying **empty**, `f32`/`f64`-free. The `core::snapshot` render snapshot is
  **untouched** (the two coexist).
- No invariant changes: the serialization is fixed-point (`Fixed` via `to_bits`, invariant #1),
  lives in platform-free `core` (invariant #2), and its correctness is asserted on the existing
  cross-arch matrix (invariant #7). The render snapshot stays the only thing the renderer reads
  (invariant #4).
- [`phase-3-plan.md`](plans/phase-3-plan.md) §"Workstream C" is unblocked (the first slice can land
  alongside workstream A); the "Decisions Phase 3 will need" snapshot-format bullet flips to
  DECIDED. [`architecture.md`](architecture.md) §Netcode notes the format is now decided (D28),
  code pending. [`README.md`](../README.md) repo-map will note `core::persist` when the first slice
  lands (not before — it does not exist yet).

---

## D29 — Android audio sink: `oboe` (low-latency AAudio), mixing through a shared host-tested seam

**Status:** the Android `pal::Audio` backend, which was a documented no-op (the last gap flagged in
[D26](#d26--phase-2-polish-round-real-opt-in-audio-output-a-selection-highlight-and-a-first-pass-balance-baseline)),
is now a real low-latency AAudio output stream. Presentation/PAL only; the deterministic sim is
untouched.

**Decision:**

1. **The Android backend uses `oboe` 0.6 directly (not `cpal`).** `oboe` is Google's official
   AAudio/OpenSL ES wrapper — the literal "low-latency AAudio" path
   [`platforms.md`](platforms.md) §2 asks for. The alternative, `cpal`'s Android backend (which
   internally uses oboe anyway), needs an `ndk-context` JavaVM handle wired through
   `android-activity` and negotiates whatever latency it picks; going to oboe directly skips that
   and lets us request `PerformanceMode::LowLatency` + `SharingMode::Exclusive` explicitly. The dep
   is added to **`pal-android/Cargo.toml` only**, under its `cfg(target_os = "android")` table,
   pinned `oboe = "0.6"` (already in `Cargo.lock` transitively under cpal — this promotes it to a
   direct dep, no version invention). The DEFAULT feature set is used deliberately: we do **not**
   add `java-interface` (the JavaVM/`ndk-context` path) — we talk straight to AAudio. (`cpal`
   already activates oboe's `java-interface` transitively, so `ndk 0.8` + `jni` are in the lock
   regardless; they coexist with our `ndk 0.9`, so this direct dep adds **no new version
   conflict**.) `oboe-sys` builds its C++ shim via `cc` against the NDK clang the build already
   needs.

2. **The per-voice render math is extracted to a shared, host-tested seam — `gonedark_pal::mix`.**
   The pan/gain/muffle/sum/eviction math (equal-power pan from `azimuth`, gain clamp, the one-pole
   low-pass that makes `muffled` off-map bleed read as distant — invariant #6 — voice summation +
   soft-clamp, `MAX_VOICES` eviction) previously lived **inline and untested** in `pal-desktop`'s
   `audio` backend. It now lives in `pal/src/mix.rs` (the host-safe trait crate, pulling only `std`
   + `Arc`), exposing `Voice`, `Mixer`, `voice_from_cue`, `synth_bank`, `oneshot_sound`. **Both**
   backends mix through it; `pal-desktop` was refactored to consume it (behavior-identical), and the
   Android backend uses the same functions. This is the CLAUDE.md "extract the pure logic to a
   testable seam" pattern — same as `render::interpolate_instances` and `engine`'s free fns: the
   realtime stream callback (cpal/oboe, un-constructible in a host test) is the only thin glue left
   per platform, and the math is unit-tested on the host with no audio device (16 tests).

3. **Failure degrades to silence, never a panic (invariant #8).** `AndroidAudio::new()` opens the
   stream; any device/builder/stream-open failure logs `[audio] disabled (silent)` to logcat and
   sets `inner: None` so every `submit_mix`/`play_oneshot` is a no-op. The realtime callback
   `try_lock`s the shared `Mixer` and emits a frame of silence if the game thread holds it — it
   never blocks the audio thread (oboe's realtime-callback contract; mirrors the desktop cpal rule).

**Why:** audio is the *primary* directional-awareness system while the map is dark (invariant #6),
so the Android sink has to actually pan by `azimuth` and muffle the strategic bleed — playing
centered would break the "alerts, not intel by ear" model. Rendering through the SAME `mix_cues`
output the desktop renders keeps the embodied audio model identical across platforms (invariant
#2 — the *render* is legitimately per-platform, which is what the PAL boundary is for; the *mix
derivation* stays shared in `engine::audio`). Lifting the render math into `pal::mix` is the
load-bearing move: it makes the only non-trivial logic host-testable (the tests-ship rule), kills a
silently-untested copy in `pal-desktop`, and guarantees the two backends can't drift. Floats here
are sanctioned — this is the platform/presentation side, never the sim (the determinism guard
scopes its float greps to `core`/`sim`, deliberately excluding `pal/`).

**What this does NOT decide / what is owed:**
- **On-device audibility.** The crate **compiles for `aarch64-linux-android`** with the NDK
  (oboe-sys's C++ shim builds), but audible output, the negotiated low-latency path, and the
  muffled-bleed audibility are device-judgment calls — shake out with `pnpm android:dev` and listen
  / read logcat for the fallback line. The glue is marked `# NOT device-verified` in the file's
  existing honest style.
- **Real audio assets.** Sounds are still the placeholder procedural cues (now shared in
  `pal::mix::synth_bank`); a real asset pass is later, unchanged from D26.
- **The two other Android phase-2 TODOs are explicitly NOT bundled here** (one-commit-one-
  workstream): the shipped mobile control scheme (on-screen virtual sticks / gyro → `move_axis`/
  `look_axis`) is an unsettled **design** call (must not be silently decided — `open-questions`/
  roadmap), and the resume snapshot (`MainEvent::SaveState`) depends on `AndroidStorage` being real
  (also stubbed) plus the [D28](#d28--authoritative-snapshot-format-a-hand-rolled-le-serialization-sharing-the-checksum-walk)
  serialize/restore path landing. Both TODOs keep sharpened notes in `pal-android/src/lib.rs`.

**Consequences:**
- New module `pal/src/mix.rs` (shared render math + 16 host tests, dev + release green);
  `pal-desktop/src/audio.rs` slimmed to consume it (its inline `Voice`/`Mixer`/synth removed);
  `pal-android` gains a real `AndroidAudio` + `oboe` dep and `android_main` opens the stream.
- `core` untouched; `pal` trait surface (`Audio`, `AudioCue`, `SoundId`) unchanged and still
  audio-API-free. No checksum impact (presentation only).
- [`platforms.md`](platforms.md) §2's "AAudio sink" is now real; [`README.md`](../README.md)
  repo-map gains `pal::mix`; `android/README.md` notes the audio sink is implemented (on-device
  audibility still owed).

---

## D30 — A measured combat/economy balance baseline + a deterministic balance-metrics harness

**Status:** supersedes [D26](#d26--phase-2-polish-round-real-opt-in-audio-output-a-selection-highlight-and-a-first-pass-balance-baseline)
§3's first-pass balance numbers. Still a **playtest baseline, NOT a locked design** — but the
numbers are now backed by an objective, deterministic metric rather than by feel. Sim-input +
test/harness change only; the deterministic *model* is unchanged.

**Decision:**

1. **A deterministic balance-metrics harness lands first** (`sim-runner`'s `metrics` module +
   `--metrics[=<which>]` mode). It scripts canonical fights/economy runs and reads integer/`Fixed`
   metrics straight off fully-observable sim state — alive-count and summed-HP (as raw Q16.16
   bits) per faction, resource purse, controlled-point count — and derives the headline numbers:
   **time-to-kill**, **equal-cost army trade**, **suppression pin-vs-kill timing**, and the
   **economy ramp**. Floats appear *only* at the stderr print boundary (seconds = ticks/60, for
   humans), exactly like `--time`; the stdout `<tick> <checksum>` stream is untouched, so the mode
   cannot affect determinism (invariants #1, #7). This makes balance an objective,
   regression-testable signal *before* any number moves.

2. **Combat re-tune (measured against the harness):**
   - **Rifleman** 6 dmg / 30-tick cooldown (12 DPS), range 14 — a symmetric open 1v1 now resolves
     in **~8.0 s** (down from the old ~12 s: decisive without being a contact-delete), and the
     longer reach means rifle *mass* wins at range.
   - **Heavy** reworked from a strictly-dominated stat-line into a short-range **bruiser**: 280 HP,
     18 dmg / 48-tick cooldown (~22 DPS), range 11 (shorter than the Rifleman), cost 220
     (down from 250), production 660 ticks (11 s, down from 12 s). The harness proved the old Heavy
     *lost every equal-cost trade* (rifle-mass wiped heavy-mass 0-for); now the matchup is
     **range-dependent rock-paper-scissors** — measured: at point-blank an equal-cost Heavy blob
     out-trades the rifles (eq-cost 500 sep5 → heavies win), at rifle range the cheaper,
     longer-reaching rifles kite and win (eq-cost 1000 sep9 → rifles win). Neither strictly
     dominates.
   - **Suppression pin** lowered 3/4 → **1/2** (`SUPPRESSION_PER_HIT` 1/8 and `SUPPRESSION_DECAY`
     1/64 unchanged). The harness showed the old 3/4 pin **never triggered before a kill** —
     suppression was cosmetic. At 1/2 a unit pins once four shots land before they decay: a
     4-shooter focus-fire pins the target on the first burst (*before* it dies, the D26 goal),
     while a lone shooter never accumulates enough (the clean 1v1 still resolves by damage). So
     suppression is now specifically a "concentrate fire to pin" lever.

3. **Economy** tables left at the D26 values (camp 250, rifle 100, camp HP 1000, income 1 + 2/pt,
   build/production times, upgrades) — the harness's economy ramp confirmed them coherent (holding
   one point triples income; a camp pays back in ~2 s of holding). Only `HEAVY_COST` (250 → 220)
   and `HEAVY_BASE_TICKS` (720 → 660) moved, as part of the Heavy rework above.

**Why:** D26 shipped a balance baseline reasoned in seconds but never *measured* — and measurement
exposed two real degeneracies (the Heavy was a trap pick that lost every cost-equal fight, and
suppression never mattered before death). Standing up a deterministic metrics harness turns
"is the balance good?" into integer numbers the sim produces bit-identically on every arch, so the
re-tune is justified by a TTK band / win-rate / ramp target instead of vibe — and any future
balance regression is caught by a test, not a playtest. The harness mirrors the `--time` discipline
(stderr only, stdout checksum untouched) so it rides the existing determinism guarantees for free,
and every tuned number stays fixed-point (`Fixed::from_int`/`from_ratio`, all power-of-two-clean
ratios — `1/2`, `1/8`, `1/64` are exact in Q16.16) so invariant #1 holds and dev == release.

**Honest caveat (carried from D26):** this is a *more-justified* baseline — the numbers are backed
by measured TTK / equal-cost-trade / pin-timing / ramp targets — but final *feel* still requires
human playtests. The framing stays "playtest baseline, not locked design."

**What this does NOT decide:** the target TTK band (settled at ~6–10 s here) and the exact
equal-cost win-rate split are first measured targets, not final; no new unit types, weapons, or
stances; the touch-UI retreat default (30%, `engine::command_ui`) is unchanged.

**Consequences:**
- `core::economy::unit_stats` (Rifleman/Heavy stats), `HEAVY_COST`, `HEAVY_BASE_TICKS`, and
  `core::combat::SUPPRESSION_PIN` move. The lockstep-coupled tests update **in the same commit**:
  `economy::balance_baseline_reads_in_seconds` re-anchored (heavy 11 s, heavy cost 220), a new
  `unit_stats_match_measured_d30_baseline` locks the tuned stats + the bruiser relationship, and
  `orders::half_suppressed_unit_moves_slower_than_clean_one` now uses a 1/4 fixture (below the new
  1/2 pin). New `sim-runner::metrics` tests assert metric determinism plus the measured properties
  (TTK band, Heavy-not-dominated-at-close-range, rifles-win-at-range, pin-before-kill, cover
  extends survival, one-point-triples-income). Full suite green dev + release.
- **Checksum impact (expected and correct):** the balance change moves the per-tick checksum
  stream. The `sim-runner` 300-tick `phase2` final checksum is now **`41e4d81992787504`** (was
  `4c34c6b5951edf57` under the D26 numbers — the value recorded in D22's *historical* on-device
  validation, left intact as a record of that run). There is **no committed golden checksum
  literal** anywhere, so [`determinism.yml`](../.github/workflows/determinism.yml) /
  `android-checksum.sh` still pass: they diff streams across the arch matrix for *equality*, which
  a balance change preserves (all arches compute the new value identically). **Owed elsewhere:** an
  on-device arm64 re-confirmation (`pnpm android:checksum` should now agree on `41e4d81992787504`)
  and human playtests for final feel — neither is doable in this environment.
- [`README.md`](../README.md) repo-map notes the `sim-runner` `--metrics` harness;
  [`roadmap.md`](roadmap.md) Phase 2 balance note updated (baseline is now measured, not first-pass).

---

## D31 — Phase 2 sign-off: game systems complete, automated-verified; device-audio + feel-playtests carried forward

**Status:** Phase 2 (game systems) is **signed off as systems-complete**. Every roadmap Phase 2
bullet is implemented, deterministic, and verified by every means available without a human or a
physical device. The items that remain are **not unbuilt systems** — they are a human/device
*confirmation* layer (audio audibility by ear, balance feel by hand) that by nature cannot be
discharged in this environment; they are carried forward explicitly rather than faked.

**What Phase 2 delivered (the body of work this entry signs off):**

1. **Systems spine** ([D23](#d23--phase-2-game-systems-the-deterministic-model-and-its-module-decomposition)) — the eight fixed-point `core` modules (`terrain, combat,
   economy, territory, fog, orders, alerts, event`): combat with suppression/cover/line-of-sight,
   territory capture, resources/economy/camps + production, fog of war (client-side derivation),
   the widened literal-executor order/stance vocabulary + retreat trigger, and the alert channel.
2. **Host/presentation wiring** ([D24](#d24--phase-2-host-wiring-foghudaudiotouch-ui-behind-a-frozen-presentation-contract)) + the reachable full order/stance vocabulary
   ([D25](#d25--the-orderstance-command-vocabulary-was-already-in-the-sim-touch-ui-now-reaches-it-corrects-d24)) — fog render, the embodied alert HUD, the embodied audio mix, multi-unit
   selection + on-screen order/stance UI; all pure presentation derivations (checksum-neutral).
3. **Polish made it real and checkable** ([D26](#d26--phase-2-polish-round-real-opt-in-audio-output-a-selection-highlight-and-a-first-pass-balance-baseline)) — desktop `cpal` audio output, the drawn
   selection rim, and the `viz-runner` offscreen render harness.
4. **The two Phase-2-close engineering gaps, now closed:** the Android **AAudio sink** via `oboe`
   + the shared host-tested `pal::mix` seam ([D29](#d29--android-audio-sink-oboe-low-latency-aaudio-mixing-through-a-shared-host-tested-seam)), and a **measured** combat/economy
   **balance baseline** backed by a deterministic `sim-runner --metrics` harness that fixed two
   degeneracies the first pass hid (a strictly-dominated Heavy, cosmetic suppression)
   ([D30](#d30--a-measured-combateconomy-balance-baseline--a-deterministic-balance-metrics-harness)).

**Verified here, by automation (the ceiling of in-environment testing):**
- Full workspace test suite green **dev + release** — `core` 162 + `sim-runner` 12 (incl. 7 new
  metrics tests) + the new `pal::mix` 16, plus the rest of the workspace.
- **Determinism:** the changed sim files are float-free (invariant #1); the 300-tick `sim-runner`
  stream is single-arch stable at the new `41e4d81992787504`; the **cross-arch checksum matrix**
  (`determinism.yml`: `x86_64-linux`, `x86_64-windows-msvc`, `aarch64-apple-darwin`,
  `aarch64-unknown-linux-gnu`) runs **automatically in CI on push** — this is the invariant-#7
  net, and it is not manual.
- **Real-pixel behavior:** `viz-runner` asserts all Phase 2 presentation behaviors offscreen —
  command view draws units, band-select rims the squad, embodiment collapses the map to **96.6%
  dark** (invariant #6), and the alert HUD draws markers as a thin overlay over the dark.
- **Balance signal:** the `--metrics` digest confirms the D30 targets — ~8 s rifle TTK,
  range-dependent rifle/heavy rock-paper-scissors (close-range heavies win, at-range rifles win),
  and focus-fire pinning before the kill while a lone shooter never pins.

**Explicitly OWED — needs a human or a physical device, deliberately not faked or skipped silently
(carried into Phase 3/4, not Phase-2 blockers):**
- **On-device audio audibility** — the AAudio sink builds for arm64 (NDK 28.2, dev+release) but
  audible/low-latency output must be confirmed by ear with `pnpm android:dev` (listen for
  panned/muffled cues while embodied; confirm logcat does *not* print `[audio] disabled (silent)`).
- **On-device arm64 checksum re-confirmation** — `pnpm android:checksum` should now agree on
  `41e4d81992787504`. (The *cross-arch* equality is already covered by the CI matrix above; only
  the on-device `adb` leg is device-specific.)
- **Human balance/feel playtests** — D30 is a *measured* baseline, not final tuning; the numbers
  (incl. the 30% retreat default) still expect to move from play. The audio asset/design pass
  (sounds are procedural placeholders) is likewise a creative task, not an engineering gap.

**Open design forks Q1/Q2/Q3 stay deliberately open.** A Phase-2-close analysis of each reaffirmed
its lean — Q1 *alerts-only with killer audio* (high confidence), Q2 *no-signal/pure-inference as
default with the soft-tell marker held as a cheap deferred knob*, Q3 *ship unconstrained, leash
deferred (camp-proximity over a cooldown)* — but each lock genuinely depends on something that does
not yet exist (real designed audio for Q1; live PvP for Q2/Q3). They ship as a **mechanism, not a
lock** (matching the roadmap), and remain in [`open-questions.md`](open-questions.md) with the lean
reaffirmed; reopening them is a playtest-driven Phase 3+ task. This is consistent with not deciding
open questions without the evidence the decision needs.

> **Q2 update — superseded by [D33](#d33--going-dark-detection-a-tunable-three-mode-tell-default-subtle):**
> the Phase-3 resolution took the **opposite default** to this entry's lean — shipping the *soft tell*
> (`Subtle`) as the default rather than no-signal — as a **tunable** `Hidden|Subtle|Marked` mechanism
> so the "mechanism, not a lock" posture above still holds (`Hidden` remains one config field away).
> Q1 and Q3 stay open with the leans recorded here.

**Why:** Phase 2's goal was "the actual game" — the systems — and those are built, tested,
deterministic, and demonstrated with real pixels and objective balance metrics. Holding the phase
open indefinitely for confirmation that *requires* a tester's ears, a player's hands, or a live PvP
opponent would conflate "systems complete" with "tuned and shipped," which are different
milestones (Phase 4 is polish/ship). Signing off the systems while naming the owed
human/device confirmation honestly — rather than faking a pass or silently dropping it — keeps the
decision log truthful and lets Phase 3 (scale & net) proceed on a complete systems base. Unlike
[D22](#d22--phase-1-vertical-slice-passed-on-real-arm64-custom-rust-engine-validated-fallback-retired)
(Phase 1, which *was* validated live on a Galaxy S24), this sign-off is explicitly
"automated-verified, device/feel confirmation owed" — the honest state.

**What this does NOT decide:** it does not declare the balance tuned, the audio designed, or the
forks locked; it does not claim on-device audio or the on-device checksum leg has run. Those are
named above as owed.

---

## D32 — Meta-UI / app shell: native per-platform shells (out-of-match), in-engine in-session

**Status:** resolves **[Q12](open-questions.md)**. The **app shell** — the out-of-match screens
scoped in [`roadmap.md`](roadmap.md) Phase 4 (title, onboarding, settings, match setup, lobby,
profile, store, consent/legal) — is built as **native per-platform shells**: SwiftUI on iOS,
Jetpack Compose on Android, and a native desktop shell on Windows/Linux. The **in-session** shell
(pause, reconnect prompt, post-match summary) is the one carve-out: it stays **in-engine**
(`render`/`wgpu`), because it renders *while embodied* and is therefore bound by invariant #6.

**Decision:**

- **Out-of-match surfaces → native** (Q12 option **b**). Each platform owns its shell in the
  native UI toolkit, getting OS-native text input/IME, scroll, the accessibility tree, deep links,
  the back-stack, and — decisively — first-class store/billing sheets (StoreKit, Play Billing) and
  account flows for free, rather than re-implementing them in `wgpu`.
- **In-session surfaces → in-engine** (forced, not chosen). Pause/reconnect/post-match draw under
  the same avatar-only fog as the match (invariant #6); a native overlay there could leak
  strategic intel or break the fog. This is the constraint Q12 already named as holding "either
  way," so it is not a reopening of #6.
- **The shell↔sim boundary is a new, explicit seam.** Native shells drive the shared engine
  through a narrow, **GPU-free, logic-free** command/query interface (start/configure/abort a
  match, read settings + progression, surface store state). The shell holds **no** game/sim logic
  — exactly like the PAL holds no game logic. This keeps the native fork *above* a thin boundary,
  consistent with invariant #2: game logic stays single-sourced in `core`; only the chrome forks.

**Why:** the two places this fork actually bites — the **store** ([Q9](open-questions.md): mandatory
StoreKit/Play Billing on mobile) and **accessibility** (a colorblind/hard-of-hearing equivalent for
the going-dark flash+audio alert channel, invariant #6) — are precisely where native toolkits give
for free what an in-engine shell would have to re-earn, and where store-policy compliance is least
negotiable. The cost native shells carry is a per-platform UI fork — the very thing invariant #2
exists to prevent — but #2 forbids forking **game logic**, not chrome. A title screen or a settings
sheet is platform glue, the same category as the PAL; building it natively does not multi-source the
sim, the netcode, or the order/stance vocabulary, which remain identical on all four platforms. The
real obligation the decision takes on is keeping the shell↔sim boundary disciplined enough that no
game logic leaks up into a shell — enforced the same way the PAL boundary is.

**What this does NOT decide:** it does not settle the billing rails themselves
([Q9](open-questions.md) stays open — *which* rails per platform, and desktop Stripe-vs-Steam), the
onboarding/PvE-first sequencing ([Q5](open-questions.md)), or the hero-asset source feeding the
store catalog ([Q11](open-questions.md)). It does not pick the desktop toolkit concretely
(native-vs-egui is a Phase-4 implementation call). It does not reopen invariant #6 — it is bounded
by it. No shell code exists yet; this fixes the *approach* before Phase 4 shell work starts, so the
shell↔sim boundary can be designed for it.

**Consequences:**
- [`open-questions.md`](open-questions.md) **Q12** migrates to RESOLVED (header + closing lean now
  point here); the original in-engine-vs-native analysis is retained beneath the resolution.
- [`roadmap.md`](roadmap.md) Phase 4 gains the **Meta-UI / app shell** scope subsection, and its
  "One shell or four?" constraint now resolves to this decision instead of an open fork.
- **Owed when Phase 4 shell work begins** (not now): define the **shell↔sim boundary** seam (the
  narrow GPU-free, logic-free command/query interface native shells drive `core` through), likely a
  short [`architecture.md`](architecture.md) addition; pick the concrete desktop toolkit
  (native-vs-egui); and keep the boundary under the same no-leak discipline as the PAL so no game
  logic climbs into a shell. [Q5](open-questions.md)/[Q9](open-questions.md)/[Q11](open-questions.md)
  still feed the onboarding, store, and catalog surfaces respectively and remain open.

---

## D33 — "Going dark" detection: a tunable three-mode tell, default Subtle

**Status:** resolves **[Q2](open-questions.md)** (can the enemy tell you've gone dark?) and gates
**workstream D** ([`phase-3-plan.md`](plans/phase-3-plan.md)). We ship a **tunable mechanism, not a locked
design** (the D31 house style): a `core::detection` module with a `tell_mode: Hidden | Subtle |
Marked` switch, **defaulting to `Subtle`** — a soft, line-of-sight-gated, *aging* tell on the
embodied unit. One build covers all three modes for A/B playtesting; the default ships ON so the
embodied-PvP mind-game has its bite, but the final lock stays playtest-driven.

**Decision:**

- **`core::detection::DetectionConfig { tell_mode, tell_range, tell_linger_ticks }`** drives a
  **pure, checksum-excluded derivation** `detectable_embodiment(...)` — same side of the line as
  fog/alerts ([`fog`](../core/src/fog.rs), [`alerts`](../core/src/alerts.rs)): it reads `&World`/
  `&Terrain`, **never** mutates sim state and is **never folded into the per-tick checksum**, so it
  cannot desync lockstep (invariants #1/#7).
- **The three modes:**
  - **`Hidden`** — no tell, ever. Pure inference: the enemy sees the embodied unit only as a normal
    unit (basic LoS targeting already works), with no marker distinguishing the player's avatar.
  - **`Subtle`** (default) — the embodied unit is revealed to an observer **only when that observer
    has a living unit within `tell_range` *and* in line of sight**, and the tell then **lingers and
    ages** for `tell_linger_ticks` after sight is lost (a fading, last-known-position marker). The
    tell is *earned* by proximity + sightline, not free intel.
  - **`Marked`** — a persistent marker on the embodied unit (the strongest tell), for the
    fairness/feel end of the A/B range.
- **Fairness (invariant #6) is built in:** in `Subtle` the tell costs the observer a unit in range
  with a sightline, and decays once they lose it — so a loss reads as *"I stayed embodied too long,
  too close,"* never *"the game robbed me."* `Hidden` is the floor (no tell); `Marked` is the
  ceiling; `Subtle` is the tuned middle we default to and validate.
- **No omniscient AI (invariant #3, [D2](#d2--unit-ai-is-a-literal-executor-not-a-strategist)) is
  structural:** `detectable_embodiment` is the **only** sanctioned channel for "which unit is the
  hero." In `Hidden` it returns nothing, so a PvE AI consulting it gains zero knowledge — and
  because the derivation takes `&World` it can never feed back into the sim, the AI's orders stay
  literal-executor. The load-bearing test: **computing detection every tick leaves the checksum
  stream byte-identical, and in `Hidden` the derivation is empty even with a unit embodied in plain
  sight.**

**Why:** Q2's lean was always *"soft tell is the most interesting, but it needs playtest."* Locking
one design now would either foreclose the mind-game (`Hidden`) or freeze a fairness-sensitive choice
unvalidated (`Marked`). Shipping the **mechanism** with `Subtle` as the default gives the embodied
mind-game its intended tension *immediately* while keeping `Hidden`/`Marked` one config field away
for A/B — exactly the "mechanism, not lock" posture D31 reaffirmed for Q1/Q2/Q3. Putting the tell on
the same pure-derivation footing as fog/alerts means it can never desync lockstep, and routing all
"is this the hero" knowledge through one checksum-excluded channel makes the no-omniscient-peek
invariant a structural property rather than a discipline.

**What this does NOT decide:** it does not *lock* the final tell mode (the default `Subtle`, the
`tell_range`, and `tell_linger_ticks` are a starting point to move from play, like the D30 balance
baseline); it does not resolve **[Q1](open-questions.md)** (how thin the thread back to command;
lean: alerts-only) or **[Q3](open-questions.md)** (possession leashed vs global) — both stay open
with their leans, and the detection mechanism is compatible with either. The **host/HUD wiring**
(`render::detection` + the pure `engine::detection_markers` seam, invariant-#6-guarded) and a
**scripted/PvE enemy that consults the channel** (`CommanderConfig::hunt_embodied`, default OFF,
range+LoS-honest) were the net-facing follow-ups to this `core` slice — both have since landed
(see [`phase-3-plan.md`](plans/phase-3-plan.md) §"Workstream D").

**Consequences:**
- [`open-questions.md`](open-questions.md) **Q2** migrates to RESOLVED, pointing here.
- A new `core::detection` module ships (config + `detectable_embodiment` + tests); the
  [`README.md`](../README.md) repo-map and [`phase-3-plan.md`](plans/phase-3-plan.md) workstream D are
  updated. The mechanism landed single-client first; the HUD wiring (`render::detection` +
  `engine::detection_markers`) and the scripted enemy that reads the channel
  (`CommanderConfig::hunt_embodied`, default OFF) have since landed as Phase 3 workstream-D
  follow-ups. The *actual* two-human mind game still needs the live net layer.

---

## D34 — The shell↔sim seam: a GPU-free, logic-free `core::shell` façade (intent in, view out)

**Status:** decided + landed (Phase 4 workstream A — the [D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session) prerequisite).

**Context:** [D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)
split the meta-UI — **native per-platform** shells for the out-of-match surfaces, the **in-engine**
in-session shell — and required *both* to reach the shared `core` through "a narrow GPU-free,
logic-free shell↔sim seam," fixed **before** any shell work begins
([`phase-4-plan.md`](plans/phase-4-plan.md) workstream A). This decides that boundary's shape.

**Decision:** the seam is a typed **façade / DTO** module, `core::shell`, on the same footing as the
PAL and the `fog`/`alerts`/`detection` derivations: it carries no game logic, makes no unit
decisions, runs no AI, touches no GPU, and mutates no sim state — it only *shapes* the shell's coarse
intents into existing `core` operations and *exposes* `core` state as presentation-safe data. Two
directions:

- **Read side (`core` → shell) — presentation-safe views, never `&mut`, never folded into the
  per-tick checksum:**
  - `MatchStatus` / `MatchPhase` — host-driven match lifecycle (the sim has no "phase" field; the
    host drives it from session events).
  - `MatchSummary` / `FactionStats` — the post-match summary, **all integer / `Fixed`, never a
    float** (invariant #1): no float money (the economy purse is `i64`) and no precomputed ratios (a
    K/D is the shell's own presentation math above the seam).
  - The **order/stance vocabulary as data** — `OrderKind` / `StanceKind` + `order_vocabulary()` /
    `stance_vocabulary()`, **single-sourced** from `core::components` (invariant #2): `OrderKind::of`
    is the one mapping point, so adding an `Order` variant is a compile error until the vocabulary is
    updated — a native shell lists the palette without re-declaring it.
  - `ConnectionStatus` — a **pure projection** of `core::lockstep` state (link state / input delay /
    next tick) for the reconnect-prompt HUD; **no sockets, no wall-clock** (the transport lives in
    the PAL, D27).
  - `InSessionView` — the **fairness-critical** embodied-HUD bundle (see Why).
- **Control side (shell → `core`):** a coarse `ShellIntent` enum resolved by the pure
  `resolve_intent` into `ResolvedIntent::{Command, Session}` — either an existing sim `Command`
  (`Embody` / `Surface`) the host feeds the lockstep stream, or a host-side `SessionAction`
  (`Pause` / `Resume` / `Surrender` / `RequestReconnect`) that **never enters the lockstep stream**.

**Why:**
- **One shared core, single-sourced (invariant #2, [D9](#d9--four-platforms-one-shared-deterministic-core-platform-optimized-backends)).**
  Routing every shell — native or in-engine — through one `core::shell` façade keeps the sim,
  netcode, and order/stance vocabulary single-sourced; only the *chrome* forks per platform, never
  the game. The vocabulary-as-data export is what lets four native shells list the order palette
  without four copies of it drifting apart.
- **Fairness is STRUCTURAL, not disciplined (invariant #6).** `InSessionView::compose` does **not**
  take `&World`. It takes the *already-derived* presentation state — the avatar's `fog::Visibility`
  (the host's contract: `embodied_visibility`, avatar-only, while embodied), the `alerts` channel
  (the only thread back to command — "alerts, not intel"), and the `detection` tells (themselves
  fog/LoS-gated, [D33](#d33--going-dark-detection-a-tunable-three-mode-tell-default-subtle)). With no
  raw world in scope the view *cannot* leak strategic intel even by accident — there is no world to
  read. A test proves a far friendly unit's and a far enemy's areas stay dark in the in-session view
  while the *command* view lights them.
- **Session-control can never desync (invariants #1/#7).** Splitting `ResolvedIntent` into a sim
  `Command` arm and a `SessionAction` arm makes it structurally impossible for pause/surrender/
  reconnect to enter the deterministic stream: pause is a host *stop stepping*, not a sim mutation,
  so a paused peer is bit-identical to a never-paused one once stepping resumes. A test asserts these
  intents never become sim commands.
- **Checksum-neutral by construction (invariant #7).** The seam adds no checksum-folded state and
  feeds no float/logic into the command path; every read view is a derivation on the same footing as
  `fog`/`detection`/`alerts`. A test composes the full in-session view every tick and asserts the
  checksum stream stays byte-identical to a sim that never calls the seam.

**What this does NOT decide:** the broader command surface the plan sketches (start/configure/abort a
match, apply settings, store/progression refresh) is *intended* but **not yet built** — this slice
lands the in-session + lifecycle + vocabulary + connection contract; the
match-setup/settings/store commands arrive with their workstreams, several of them blocked on
[Q5](open-questions.md)/[Q9](open-questions.md)/[Q11](open-questions.md). It adds **no** win-condition
evaluator (that is game logic for a `core` *system*, not this boundary — the host fills `MatchSummary`
today) and **no** `PartialEq` on `sim::Command` (a clean follow-up if `ResolvedIntent` ever needs
comparison). Native UI scaffolding (FFI, SwiftUI / Jetpack Compose / a desktop shell) is out of
scope — D32's "native shells" remain deferred behind this seam and the
[Q5](open-questions.md)/[Q9](open-questions.md)/[Q11](open-questions.md)/Phase-3 blockers.

**Consequences:**
- A new `core::shell` module ships (the façade types + `resolve_intent` / `ConnectionStatus::project`
  / `InSessionView::compose`, plus tests incl. the load-bearing fairness + checksum-neutrality
  guards); `core` tests grow 193 → 202 (green dev + release; float-free guard clean; `code-reviewer`
  CLEAN).
- [`architecture.md`](architecture.md) gains the shell↔sim boundary note D32 flagged as owed; the
  [`README.md`](../README.md) repo-map lists `shell`; [`phase-4-plan.md`](plans/phase-4-plan.md)
  workstream A is marked landed.

---

## D35 — First native app-shell surface: the Android Compose "Boot & title" landing screen

**Status:** decided + landed (Android only). The **first native out-of-match shell surface** —
"Boot & title" ([`phase-4-plan.md`](plans/phase-4-plan.md) §2 surface 1) — ships as a native **Jetpack
Compose** layer in the existing [`android/`](../android) Gradle project, realizing
[D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session) (native
per-platform out-of-match shells) and consuming the
[D34](#d34--the-shellsim-seam-a-gpu-free-logic-free-coreshell-façade-intent-in-view-out) `core::shell`
seam. This is the first surface to become buildable *the moment the seam landed*.

**Decision:**

- **The Android launcher is now a native Compose shell, not the engine.** A new
  `MainActivity` (`ComponentActivity`) is the **LAUNCHER** and draws the title screen: the **GOING
  DARK** title, the **COMMAND · EMBODY** tagline, **START / SETTINGS / QUIT** actions, and a
  build/version stamp, in a dark "going-dark" Material3 palette. The shared Rust engine's
  `android.app.NativeActivity` (`libgonedark_pal_android.so`) is **demoted** to a non-launcher,
  non-exported activity that `MainActivity` starts via an explicit `Intent` on **Start**.
- **Compose/Kotlin are enabled in the Gradle build.** Kotlin 2.0.21 + the Compose compiler plugin +
  Jetpack Compose (Compose BOM 2024.10.01, Material3, `activity-compose` 1.9.3), `buildFeatures {
  compose = true; buildConfig = true }`, Java/Kotlin 17. New Kotlin sources live under
  `android/app/src/main/java/com/jaredhoward/goingdark/` (`MainActivity`, `TitleScreen`,
  `ui/theme/Theme`, `BuildStamp`), plus a NoActionBar dark theme (`res/values/themes.xml` +
  `colors.xml`) so there is no white launch flash. `android:hasCode="false"` is removed — the app now
  carries Kotlin bytecode.
- **The shell holds NO game/sim logic.** Per [D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)/[D34](#d34--the-shellsim-seam-a-gpu-free-logic-free-coreshell-façade-intent-in-view-out)
  it reaches `core` only through the `core::shell` seam — exactly like the PAL holds no game logic.
  Today **Start** launches the engine's **default** match; match-configuration handoff across the
  seam (army / map / mode) is **deferred** with match-setup, itself **[Q5](open-questions.md)**-blocked.
- **`abiFilters` stays arm64-v8a only** (Phase-1 stance unchanged). The Compose launcher is pure JVM
  bytecode and renders on the x86_64 emulator; only pressing **Start** into an embodied match needs
  the matching native ABI.

**Why:** [D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)
chose native shells for the out-of-match surfaces, and
[D34](#d34--the-shellsim-seam-a-gpu-free-logic-free-coreshell-façade-intent-in-view-out)'s
`core::shell` seam — the named prerequisite for *every* shell surface — has now landed. Per
[`phase-4-plan.md`](plans/phase-4-plan.md) §2, "Boot & title" carries **no** design/net blocker; its *only*
remaining gate was the missing per-platform native UI project. This change creates that project, so
"Boot & title" is the first native surface buildable once the seam landed — exactly the sequencing the
plan called. Building it in Compose buys OS-native text input/IME, scroll, the accessibility tree, the
back-stack, deep links, and first-class store/billing sheets for free
([D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)),
rather than re-earning them in `wgpu`. Invariant #2 holds — the fork is *chrome*, not game logic; the
sim/netcode/order vocab stay single-sourced in `core`.

**What this does NOT decide:** it does **not** settle the match-config handoff (army/map/mode) across
the seam — that ships with **match-setup**, **[Q5](open-questions.md)**-blocked (surface 4). **Settings**
is a no-op placeholder until the Settings surface (surface 3) lands. It does **not** build the
**desktop** native shell (the `app` crate still boots straight into a match — no desktop shell yet) or
any **iOS** surface (no iOS build target exists at all per Phase 3). **Onboarding** (surface 2) stays
**[Q5](open-questions.md)**-blocked. This is **Android only**.

**Consequences:**
- [`android/`](../android) gains a Kotlin/Compose source set and Gradle Compose enablement; the engine
  `NativeActivity` is now **Start-launched**, not the launcher.
- [`phase-4-plan.md`](plans/phase-4-plan.md) §2 surface 1 + the §"LATER" table flip from BLOCKED to
  **LANDED (Android)** (the deeper menu behind it — match setup / lobby — stays blocked per its own
  rows); [`roadmap.md`](roadmap.md) §"Meta-UI / app shell" "Boot & title" row notes the Android Compose
  landing screen has landed.
- **Tested where it has logic:** the pure `buildStamp()`/`buildChannel()` seam is covered by a JVM unit
  test (`BuildStampTest`); the Compose UI itself is device-gated chrome
  ([D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)) and
  exempt per CLAUDE.md's testing rule (thin, un-constructible glue).
- **Still owed:** the match-config handoff (Q5), Settings content, the desktop + iOS native shells, and
  onboarding (Q5) — each tracked against its own blocker in [`phase-4-plan.md`](plans/phase-4-plan.md) §2.
- The in-engine in-session shell (Phase 4 workstream B) and the native out-of-match shells now have
  a fixed contract to build against.

---

## D36 — The desktop app shell: an egui "Boot & title" title screen (desktop sibling of D35)

**Status:** decided + landed (desktop). The **desktop** counterpart of
[D35](#d35--first-native-app-shell-surface-the-android-compose-boot--title-landing-screen): the
"Boot & title" surface ([`phase-4-plan.md`](plans/phase-4-plan.md) §2 surface 1) now also ships on the
desktop host — an **egui** title screen in the `app` crate. The desktop binary previously booted
straight into a match (`Game::new`); it now boots into a native title screen, and **Start** enters
the match. This realizes
[D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session) (native
per-platform out-of-match shells), consumes the
[D34](#d34--the-shellsim-seam-a-gpu-free-logic-free-coreshell-façade-intent-in-view-out) `core::shell`
seam, and — decisively — **makes the desktop-toolkit call D32 explicitly left open**.

**Decision:**

- **The desktop launcher is now an egui shell, not the engine.** `app/src/main.rs` becomes a
  host-level `Screen` state machine — `Title` (the egui shell) ↔ `InMatch(Game)`. The window,
  surface, and egui shell are created in `resumed`; `Game` is created **lazily on Start**. Input is
  routed *by screen* (egui on `Title`, the game input accumulator in-match), so nothing leaks
  between the shell and the sim. **Quit** exits; **Settings** is a no-op placeholder.
- **The title screen is egui, bound to the app's existing single wgpu/winit stack.** `egui` /
  `egui-wgpu` / `egui-winit` (all 0.34) are added to `app/Cargo.toml` under the **not-android**
  target. They bind to the **same single `wgpu 29.0.3` + `winit 0.30.13`** the app already pins (one
  `wgpu` in the dep tree — no conflict), so egui renders through the **same device + window** the
  engine uses: **no second window, no second event path.** New `app/src/shell.rs` draws the title
  screen — the **GOING DARK** title, the **COMMAND · EMBODY** tagline, **START / SETTINGS / QUIT**
  buttons, and a build/version stamp, in the same dark "going-dark" palette as the Android shell
  ([D35](#d35--first-native-app-shell-surface-the-android-compose-boot--title-landing-screen)) — and
  reports the clicked action.
- **The shell holds NO game/sim logic.** Per
  [D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)/[D34](#d34--the-shellsim-seam-a-gpu-free-logic-free-coreshell-façade-intent-in-view-out)
  it reaches `core` only through `core::shell` — exactly like the PAL holds no game logic. The real
  logic is pushed into a **pure, unit-tested seam** (`resolve_title_action` / `build_stamp` /
  `build_channel`); the egui glue (`EguiShell`) is device-gated host chrome, exempt from unit tests
  per CLAUDE.md (thin, un-constructible glue). Today **Start** creates the engine's **default**
  match; match-configuration handoff (army / map / mode) is **deferred** with match-setup, itself
  **[Q5](open-questions.md)**-blocked.

**Why:**
[D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session) chose
native shells for the out-of-match surfaces but **explicitly did not pick the desktop toolkit** —
"it does not pick the desktop toolkit concretely (native-vs-egui is a Phase-4 implementation call)."
This change makes that call: **egui**. egui is the **Rust-native immediate-mode GUI** that
integrates with the app's existing wgpu/winit stack through a **single shared wgpu device** — no
second window, and no per-OS native-desktop UI fork (GTK on Linux, WinUI on Windows) to build and
maintain. It keeps the shell **above the `core::shell` seam** (invariant #2): the fork is *chrome*,
not game logic; the sim/netcode/order vocab stay single-sourced in `core`. It mirrors the Android
Compose shell
([D35](#d35--first-native-app-shell-surface-the-android-compose-boot--title-landing-screen)): native
out-of-match chrome, host-side, through the seam — and like that landing, **Start is one-way** (the
in-engine Surrender/leave flow is how you leave a match, the
[D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)
carve-out), so no return-to-title path is added here.

**What this does NOT decide:** it does **not** settle the match-config handoff (army/map/mode) across
the seam — that ships with **match-setup**, **[Q5](open-questions.md)**-blocked (surface 4).
**Settings** is a no-op placeholder until the Settings surface (surface 3) lands. *(Superseded by
[D75](#d75--desktop-settings--profile--about-surfaces-land-phase-4-surface-3-partial-audio--look-prefs-wired):
the desktop Settings/Profile/About screens have since landed and master/SFX volume + look-sensitivity
are wired — surface 3 is now PARTIAL on desktop.)* It adds **no**
return-to-title path (leaving a match is the in-session shell's Surrender/leave flow, the
[D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)
carve-out under avatar-only fog, not this screen) and does **not** change the in-session shell (still
in-engine — D32 carve-out). It builds **no iOS** shell (no iOS build target exists at all, per Phase 3).

**Consequences:**
- `app/Cargo.toml` gains desktop-only (not-android target) `egui`/`egui-wgpu`/`egui-winit` 0.34 deps,
  bound to the already-pinned `wgpu 29` / `winit 0.30` (a single `wgpu` in the dep tree); `app/src`
  gains `shell.rs` and a `Title`↔`InMatch` host loop in `main.rs`.
- `pnpm play` now opens on the title screen instead of booting straight into a match; **Start** enters
  the engine's default match.
- **Tested where it has logic:** the pure `resolve_title_action`/`build_stamp`/`build_channel` seam is
  covered by `app` unit tests; the egui chrome is device-gated host chrome
  ([D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)) and
  exempt per CLAUDE.md. The shell compiles against the pinned wgpu 29 / winit 0.30 with one `wgpu` in
  the dep tree; egui is desktop-only host chrome.
- **"Boot & title" is now landed on BOTH Android ([D35](#d35--first-native-app-shell-surface-the-android-compose-boot--title-landing-screen))
  and desktop (D36)**; **iOS** still has no build target. [`phase-4-plan.md`](plans/phase-4-plan.md) §2
  surface 1 + the §"LATER" table and [`roadmap.md`](roadmap.md) §"Meta-UI / app shell" "Boot & title"
  row note both. The deeper menu behind it — match setup / lobby — stays blocked per its own rows
  (**[Q5](open-questions.md)** / Phase-3).

---

## D37 — Embodied firing model: a fixed-point cone hitscan, sim-authoritative via the lockstep stream

**Status:** decided + landed. Part of the **playability push**
([`playability-plan.md`](plans/playability-plan.md), worker W1). The FPS half of the hybrid was
non-functional — `core::combat` skipped `InputSource::Embodied` units and there was no fire command,
so an embodied player could move, look, and *die* but deal no damage. This closes that gap while
holding invariants #1/#4/#5/#7.

**Decision:**

- An embodied (possessed) unit fires **only** through a new `Command::Fire { entity, dir }` — it never
  auto-fires (the combat pass keeps skipping `InputSource::Embodied`). Design depth stays in the
  player's aim/timing, not in unit autonomy (invariant #3/#5).
- **The firing direction crosses into `core` as `Fixed` bits, never a float.** The host integrates aim
  as a presentation-only `f32` yaw (D15), then a new pure seam `engine::fire::fire_command` quantizes
  `cos/sin(yaw)` to a `Fixed` unit vector **at the input boundary** — exactly the `world_to_fixed`
  pattern used for tap targets. No float leaks into the sim (invariant #1).
- The shot resolves **sim-side** in `core::combat::resolve_fire`: a fixed-point **cone hitscan** picking
  the nearest living hostile unit (ties → lowest index) with `dir·(t−p) ≥ cos_half·|t−p|` — evaluated by squaring both
  sides after rejecting a negative projection, so there is **no `sqrt`/normalize** — within weapon
  `range²` and passing `terrain.line_of_sight`, taking the same cover-mitigated damage / suppression /
  cooldown writes as the auto-resolver. The aim cone is `cos(30°)` (a 60° hip-fire arc); a clean miss
  deliberately does **not** spend cooldown.
- The command rides the **lockstep command stream** like player taps, and writes only
  already-checksummed fields, so the per-tick checksum `fold()` is **unchanged** and the cross-arch
  matrix stays comparable (invariant #7). The `core::lockstep` wire codec gained a `Fire` tag.

**Why:** Resolving the shot from a lockstep command on every peer (not on the firing host) is the only
way to keep it bit-identical for netcode; doing it in fixed-point with no transcendental keeps invariant
#1. A cone — not a perfect ray — makes a hip-fired shot read as "I pointed at him and hit" while still
demanding the player face the target.

**What this does NOT decide:** weapon variety, projectile/ballistic weapons, reload/ammo, or aim-assist
tuning — the cone half-angle and damage reuse the existing Rifleman weapon and are a *baseline*, not a
balance lock. Embodied locomotion feel is still owed.

**Consequences:** `core::sim` (`Command::Fire`), `core::combat::resolve_fire`, `core::lockstep` codec,
the new `engine::fire` seam, and one push in `Game::frame`. Covered by core unit tests (cone hit/miss,
range, LoS, cooldown gate, dead-target skip, tie-break) green dev+release, and the determinism + 2-peer
lockstep runners stay green.

---

## D38 — Match-end / victory condition is a host-side derivation, not a sim system

**Status:** decided + landed. Playability push ([`playability-plan.md`](plans/playability-plan.md), worker
W2). A match previously never ended — `MatchOutcome` was hard-wired to `Draw`.

**Decision:**

- The win/lose/draw outcome is decided by a **pure host-side evaluator**
  (`engine::session_shell::evaluate_outcome`) reading already-checksummed end-state — per-faction alive
  unit + building counts, territory, resources — **not** by a `core` system folded into the per-tick
  checksum. It takes an extracted `FactionForces` value, never `&World`, so it is fairness-safe and
  unit-testable without a GPU.
- **Rules:** *elimination* (a combatant with 0 alive units **and** 0 buildings loses; sole survivor
  wins; mutual → draw) dominates a **15-minute** (`elapsed_ticks` ≥ timeout) tiebreak by territory, then
  resources, else draw. `Neutral` never wins or loses. The evaluator returns `None` while the match is
  ongoing; `shell.end_match` is idempotent so the first decided outcome sticks.

**Why:** A match must end, but the outcome is *presentation / session policy*, not deterministic sim
state. Deriving it host-side from facts the sim already settled means it folds nothing new — it
**cannot desync** lockstep (invariants #1/#7) — and keeps `core` free of a win-condition evaluator,
consistent with [D34](#d34--the-shellsim-seam-a-gpu-free-logic-free-coreshell-façade-intent-in-view-out)'s
"the host fills the summary."

**What this does NOT decide:** game modes beyond skirmish elimination/score (no king-of-the-hill,
ticket bleed, or objective modes yet); the 15-minute timeout is a default constant, not a tuned value.

**Consequences:** `engine::session_shell` (the evaluator + `FactionForces`) and the `assemble_summary`
region of `Game::frame` (the `Draw` placeholder swap + the per-faction read). The post-match summary now
shows a real VICTORY / DEFEAT / DRAW. Ten branch unit tests, green dev+release.

---

## D39 — The enemy is a commander-level scripted AI issuing orders via the lockstep stream

**Status:** decided + landed. Playability push ([`playability-plan.md`](plans/playability-plan.md), worker
W3). The enemy previously got one `AttackMove` at spawn then went inert forever.

**Decision:**

- The opponent is a deterministic `core::commander::commander_orders(...)` that emits **only existing
  order/economy commands** (AttackMove / SetOrder-stance / Build / QueueProduction). Units stay
  **literal executors** — all "intelligence" is the commander *choosing orders*, never per-unit autonomy
  (**invariant #3**).
- The host drives it on a **1-second** (`tick % 60 == 0`) gate and feeds its orders into the **same
  `commands` Vec player input rides, before `drive_lockstep`** — so they travel the lockstep stream and
  apply bit-identically on every peer.
- **Determinism (load-bearing):** the commander uses its **own** seeded `core::rng::Rng`
  (seed = `sim_seed ^ faction`) owned by `Game` — it must **never** draw from `sim.rng()` (that stream is
  checksummed; a host-side draw would advance it and desync, invariant #7). All targeting is squared-
  `Fixed` magnitude with stable lowest-index tie-breaks; float-free. Its behavior loop: reinforce at
  camps when affordable, send idle units to capture the nearest neutral/enemy point, press the nearest
  hostile otherwise, and bump `HoldFire` units to `ReturnFire`.

**Why:** A playable match needs an opponent, but it must not break the literal-executor rule or
determinism. Modeling the AI as a *commander that issues orders* preserves invariant #3; routing those
orders through lockstep (own RNG, never the sim RNG) makes the AI cross-play-safe by construction with
zero new checksum-folded state.

**What this does NOT decide:** difficulty tiers, build-order sophistication, or PvE campaign structure —
the commander is a *baseline* opponent. It does not change the netcode topology
(D27); it is a producer of ordinary commands on the existing stream.

**Consequences:** new `core::commander`, `core::lib` (`pub mod commander`), and the `Game` commander RNG
field + the 1 Hz push in `Game::frame` (which replaces the one-shot spawn order). Determinism tests
(same seed+tick ⇒ identical orders; a 300-tick run is checksum-stable) plus a "the enemy now moves,
captures, and reinforces" integration test, green dev+release; sim + 2-peer lockstep runners stay green.

---

## D40 — Embodied-world rendering: a real FPS world drawn while the strategic map stays dark

**Status:** decided + landed. Playability push ([`playability-plan.md`](plans/playability-plan.md), worker
W5). The embodied (first-person) view was a literal near-black void + the avatar quad, which read as
broken and gave no sense of motion or heading.

**Decision:**

- The embodied pass now paints a real first-person space — a camera-reconstructed **sky gradient**, a
  **gridded ground plane**, and a screen-space **weapon viewmodel** with a muzzle-flash cue — in a new
  `render::world` module, replacing the bare `CLEAR_DARK` void.
- **The fairness boundary (invariant #6) is unchanged and structural:** it remains the
  `render::fog::visible_instances` filter, which while embodied keeps only the avatar (`FLAG_EMBODIED`)
  plus the avatar's own line of sight and drops the whole strategic map — off-screen squad, allies, and
  control-point rings. The world pass draws **only the camera-derived environment + a screen-space gun**;
  it has **no access to sim entities**, so it cannot leak intel even in principle.
- **"World goes dark" means losing strategic *intel*, not staring at black.** Consistent with
  [`game-design.md`](game-design.md) §6 / `core::fog::embodied_visibility` ("fog reverts to avatar-only
  vision"), an enemy physically in the avatar's first-person line of sight is **legitimate FPS sight**,
  not a map reveal. The viz-runner fairness assertion was correspondingly moved off a brittle
  "the frame is ~all black" proxy onto the real guarantee: the **strategic map** (own-squad / ally /
  control-point intel) collapses to ~zero even as the enemy fires, because alerts are directional pings,
  not intel.

**Why:** The void hurt readability and embodiment feel for no fairness benefit — the fairness comes from
the fog filter, not from refusing to draw a floor. Rendering the world purely from the *camera* keeps
the guarantee airtight (the pass literally cannot see sim entities) while making first-person movement
legible. The muzzle cue is presentation-only (`Game::last_fire_tick`, never read by the sim).

**What this does NOT decide:** real weapon/character art, world geometry/cover meshes, lighting, or a
skybox asset — the ground/sky/gun are procedural placeholders. It does not change what the fog filter
reveals; it only stops wasting the frame on black.

**Consequences:** new `render::world` (+ shader) and `world`-pass wiring in `render::lib`; the embodied
unit pass now LOADs over the sky clear; a presentation-only `last_fire_tick` on `Game`. The viz-runner
fairness assertions were re-expressed around strategic-map collapse (and proven to hold during combat),
not weakened. Render tests + viz-runner green.

> **Superseded in part by [D52](#d52--embodied-view-draws-fog-filtered-avatar-visible-units-post-match-dismiss-wiring):**
> the "the world pass draws **only** the camera-derived environment + a screen-space gun; it has **no
> access to sim entities**" mechanism above no longer holds — the embodied pass now *does* draw
> fog-filtered avatar-visible sim units. The fairness guarantee (invariant #6) is unchanged but is now
> enforced **structurally by the `render::fog::visible_instances` filter** (avatar's own line of sight
> only — it drops `FLAG_EMBODIED` self + the strategic map + control-point rings), not by withholding
> entity access from the pass.

---

## D41 — AI-generated placeholder models for all render content (skip commissioned art for now)

**Status:** decided (sourcing direction + method); assets not yet landed. **Method chosen:**
Claude-authored **Blender (`bpy`) procedural scripts → `.glb`** — i.e. simple models *scripted* in a
Claude Code session, not pulled from an external text-to-3D service. Scopes the art bullet of the
[`roadmap.md`](roadmap.md) "Path to publishable" checklist.

**Decision:**

- For the push to a publishable, *complete-feeling* build, **every** visible model — units, structures,
  and the embodied weapon viewmodel — is sourced as a **placeholder generated in-session** rather than
  commissioned, bought, or hand-sculpted art. Custom 3D authoring is deferred, not cancelled.
- **Method: Claude-scripted procedural Blender.** Models are built from primitives by a headless
  `blender --background --python` script (`tools/models/gen_models.py`) that exports one `.glb` per object
  plus a license manifest. This is the **procedural / kit-bash** lane that
  [`content-pipeline.md`](content-pipeline.md) §5–§6 already puts in the **"Claude *can*"** column —
  *not* the "Claude cannot sculpt hero meshes" lane. It is greybox quality by construction: blocky,
  intentional-looking placeholders, not final art.
- This **pulls the "AI-assisted" route forward**: §2 had reserved AI/assisted authoring for the *hero*
  tier (the few things the camera lingers on); D41 makes scripted placeholders the **default greybox/low
  tier for the whole game** for now. The mid and hero tiers remain the documented endgame — this is a
  temporal (axis-C) call, not a new permanent tier.
- **The provenance/open thread is resolved by the method.** Code-authored geometry from primitives has
  **no external-tool license to vet** — output is original, CC0-able, owned. The earlier "which generator,
  on what terms?" question is moot: each asset's manifest entry reads `source: procedural (Blender bpy)`,
  `license: CC0-1.0`, with a `sha256`. License hygiene (§3) is *satisfied*, not relaxed.
- **Nothing else about the pipeline relaxes.** Generated assets are still **one source `.glb` per object**
  destined for the cook → LOD → ASTC/atlas chain (§1) and must pass the **two-view filter** (§4) — and the
  honest weak axis is exactly there: primitive-built models read fine as **top-down RTS tokens** but are
  crude at **FPS eye-level**. That's the accepted placeholder trade; the mid/hero art pass is what
  eventually fixes eye-level credibility.

**Why:** The game now *plays* (D37–D40) but looks like greybox; commissioning or hand-authoring art is
the single most expensive, slowest path to "looks finished," and the design corpus has always been
placeholder-first (§2) — buying art before feel is locked is exactly what the production ladder warns
against. Scripting simple models in-session gets a consistent, license-clean, intentional-looking
placeholder set across every object for near-zero cost, which is all a publishable *first* build needs;
real mid/hero art is a later, post-feel spend.

**What this does NOT decide:** the *hero*-tier sourcing question — [`open-questions.md`](open-questions.md)
**Q11 stays open**, since D41 is about the **placeholder/greybox** tier, not the few things the camera
lingers on; final art direction or the eventual mid/hero authoring (still the endgame per §2/§6); the
**glTF runtime loader** — `render` currently draws procedural instanced primitives and has *no* mesh
loader, so wiring these `.glb` into the renderer (+ a cook step) is separate follow-on work; anything in
the sim — assets are render-only, so invariant #1 (no floats in the sim) does not reach here.

**Consequences:** a new `tools/models/` (the `bpy` generator + a wrapper) and `assets/models/` (the
`.glb` outputs + `manifest.json`) land, behind a `pnpm assets:models` task; Blender becomes a
content-tooling dependency (dnf, headless). `content-pipeline.md` §2 names scripted procedural models as
the default placeholder source and the hero bullet points back here; the roadmap "Path to publishable"
art bullet cites D41.

---

## D42 — Desktop command controls: the classic-RTS split (left-click selects, right-click commands)

**Status:** decided + landed (desktop). A control-feel fix on top of the playability push
([`playability-plan.md`](plans/playability-plan.md)): the *desktop* command-layer input scheme. The
question came from play — "if I select a troop and then click, shouldn't it move there?" — and
exposed a genuine smell.

**The problem:** a single left-click did two conflicting things. `map_input_commands` emitted a
`Move` for a **hard-wired avatar** (`Game::player`) on *any* command-view click — ignoring the
selection — while the *same* click also drove unit selection. To actually move the **selected**
squad you had to additionally press a number-key vocabulary slot (slot 0 = Move). So "select a
troop, then click" did **not** move that troop; it moved the avatar and re-selected.

**Decision (desktop):**

- **Left-click selects** (single-click a unit, drag = band-select) and, while **embodied**, **fires**
  (FPS convention; the two consumers are mode-exclusive, so one button is unambiguous). Fire moved
  off the right button onto left + `Space`.
- **Right-click commands the current selection** — the primary, no-modifier order: `Move` onto empty
  ground, `AttackMove` onto an enemy (a generous host-side hit-test picks "on an enemy"). A new
  edge-triggered `pal::InputFrame::command_click` carries this; the desktop backend latches it from
  the right mouse button.
- **The number keys / radial keep the *advanced* order vocabulary** (attack-move-anywhere, the three
  stances, hold, patrol, fall-back, retreat trigger) — `command_ui::commands_for` is unchanged. They
  are now the *depth* layer (invariant #3 / game-design §8), not the only way to move.
- `map_input_commands` no longer moves anything — it handles only the embody/surface swap. The new
  right-click path is a pure `command_ui::command_click_commands` seam, unit-tested.

**Why:** Click-to-command is what every RTS player (and the cited *Company of Heroes* lineage)
expects; separating select (left) from command (right) removes the one-button overload that made the
feel muddy. It is purely the presentation→intent layer — it emits the *same* `Move`/`AttackMove`
commands the sim already had, quantized to `Fixed` at the boundary, so there is **no sim or
determinism change** (the per-tick checksum and 2-peer lockstep are byte-identical before and after).

**What this does NOT decide:** the **touch** scheme. The game is mobile-first and touch has no right
button, so the phone needs its own gesture mapping onto `command_click` (e.g. tap-to-select then
tap-ground-to-move, long-press for the radial) — that stays **[Q4](open-questions.md)** and is wired
later. It also does not add a select-all / control-group / shift-queue system, or a click-to-attack on
a *specific* entity handle (right-click-on-enemy maps to `AttackMove` onto the point, which the
literal-executor unit then engages by stance — invariant #3).

**Consequences:** `pal::InputFrame` gains `command_click`; `pal-desktop` rebinds the mouse (left =
select/fire, right = command) and updates its input tests; `engine::command_ui` gains
`command_click_commands`; `engine` adds an `enemy_unit_at` hit-test and the right-click wiring in
`Game::frame`, and `map_input_commands` loses the legacy avatar-move. Covered by new unit tests
(command-click move/attack/empty cases, the rebind, the no-bare-click-move guarantee); full suite +
determinism + lockstep + viz all green.

---

## D43 — Touch command scheme: single-pointer contextual tap (the mobile sibling of D42)

**Status:** decided + landed (Android command layer). The touchscreen counterpart of
[D42](#d42--desktop-command-controls-the-classic-rts-split-left-click-selects-right-click-commands).
This is the *shipping* touch command UI that [Q4](open-questions.md)/D14 explicitly deferred
as "downstream design work" once the selection + order vocabulary existed — now built.

**The constraint:** a touchscreen has no second button, so the D42 left-select / right-command split
can't transfer directly. The select-vs-command decision must be made by *what was tapped*, which
needs unit hit-testing — so it lives in the **engine**, not the PAL (the PAL has no sim access).

**Decision (touch):**

- **Tap a friendly unit → select it.** **Drag → band-select.** (Unchanged selection grammar.)
- **Tap *off* any unit while a selection is active → issue the default order** to that selection —
  `Move` onto empty ground, `AttackMove` onto an enemy — and **keep** the selection (so you can keep
  ordering). This is the one-button expression of D42's right-click.
- **Two-finger tap → toggle embodiment** (promoted from the provisional Phase-1 binding).
- Mechanism: a new `pal::InputFrame::command_tap` **mode flag** (set every frame by touch backends,
  `false` on desktop) tells the engine to resolve an off-unit tap as a command. `Selection::update_ex`
  now returns a `GestureOutcome` and, in `tap_commands` mode, an empty-ground tap **keeps** the
  selection instead of deselecting; the engine turns that outcome into the **same**
  `command_ui::command_click_commands` emission D42 uses (Move / AttackMove, Fixed-quantized).
- **Fixes a latent bug:** `pal-android` never latched `pointer_up`, so the `Selection` release branch
  never fired — command-layer selection was entirely non-functional on touch. The backend now latches
  the single-finger release edge (and suppresses it for the two-finger gesture).

**Why:** It matches what a mobile RTS player expects ("tap my squad, tap where to go") and keeps one
shared command path with desktop — both schemes funnel into the same `Move`/`AttackMove` emission, so
the sim sees identical commands and there is **no sim or determinism change** (checksum + 2-peer
lockstep byte-identical). Resolving select-vs-command in the engine (the only layer with unit
positions) keeps the PAL thin and platform-agnostic per platforms.md §5.

**What this does NOT decide / still owed (deliberately a separate slice):** the **advanced order
vocabulary on touch** (the on-screen radial for attack-move-anywhere / stances / patrol / fall-back)
needs a long-press signal **and** wedge hit-testing UI that doesn't exist yet — desktop reaches these
via the number keys, touch will reach them via the radial later. **Embodied locomotion on touch**
(on-screen twin-stick / gyro → `move_axis`/`look_axis`, and a fire control) is likewise unbuilt. And
on-device feel is unverified here (the Android input path can't be host-unit-tested — `MotionEvent` is
un-constructible — so the glue is covered only by the host-tested `Selection`/`command_ui` seams it
feeds). [Q4](open-questions.md) stays **RESOLVED** (the feel risk was retired in D14); this is its
implementation, not a reopening.

**Consequences:** `pal::InputFrame` gains `command_tap`; `pal-android` latches `pointer_up`, sets the
mode, and tracks a `multi_touch` flag for the two-finger gesture; `engine::selection` gains
`GestureOutcome` + the `tap_commands` keep-on-empty behavior (and returns the outcome); `engine`
adds the contextual-tap command wiring in `Game::frame`. New `Selection` unit tests (tap-select,
empty-tap-keeps, desktop-empty-tap-still-clears, drag→Banded, no-release→None); the Android
`MotionEvent` glue is exempt per CLAUDE.md (un-constructible-in-test). Full suite + determinism +
lockstep + viz all green.

---

## D44 — Cooked greybox meshes: the .glb-to-runtime pipeline + 3D mesh rendering

**Status:** decided + landed (render + engine, desktop & Android via the shared `engine` loop;
viz-runner smoke test green). The runtime half of [D41](#d41--ai-generated-placeholder-models-for-all-render-content-skip-commissioned-art-for-now)
— it **resolves D41's explicit "no mesh loader / separate follow-on work" note**: the Blender `.glb`
models now actually *show up* in the apps instead of sitting unused on disk.

**The gap it closes:** D41 generated `.glb` models, but nothing loaded them. Units were flat colored
quads, the embodied weapon was hand-built 2D screen-space geometry, and the sky/ground was a procedural
shader — the `.glb` files rendered in **zero** apps, and there was no glTF parser anywhere.

**Decision:**

- **A cooked `.mesh` runtime format (the cook step of `content-pipeline.md` §1, greybox tier).**
  `gen_models.py` now emits, per model, a `.glb` (interchange / source-of-record, for the two-view
  harness §4 and external tools) **and** a cooked `.mesh` — a dead-simple, little-endian, Z-up,
  flat-shaded triangle soup (magic `GDM1`, position + face normal per corner). The engine consumes the
  `.mesh`; the `.glb` stays "the thing we're using" in Blender. Chosen over parsing `.glb` on-device so
  there is **no glTF/JSON parser and no extra crate dependency** in the renderer — the format is ~15
  lines to parse and golden-tested against every committed file. Flat normals are computed from each
  triangle's own vertices (cross product, normalized), immune to the skew the non-uniform `dimensions`
  scale bakes into Blender's cached polygon normal.
- **Embedded, not streamed.** `render::mesh` `include_bytes!`s the committed `.mesh` files, so they ride
  into the binary/APK with **no on-device file IO, no PAL storage round-trip, no Android asset-pack
  plumbing**. Right for the small greybox tier; the heavyweight pak/mmap pipeline (`architecture.md`)
  stays the target for the eventual mid/hero art.
- **One shared 3D mesh pipeline (`render::mesh` + `mesh.wgsl`):** instanced, depth-tested, lit by a
  single directional key light + ambient over the flat facets. Both consumers draw through it:
  - **Embodied weapon viewmodel** — the real `weapon_rifle` mesh, anchored in *view space*
    (`world::weapon_view_model`, fed the projection alone) so it stays glued to the lower-right under
    camera yaw, with a muzzle-flash flare + recoil kick. Replaces the old 2D gun.
  - **Command-view unit tokens** — each fog-visible unit/building is its 3D mesh (infantry → `trooper`,
    building → `camp_hq`), composited under the 2D UI: ground grid clears, tokens draw (depth-tested),
    then the quad pass loads on top with the token body fill suppressed (`FLAG_MESH`) so the mesh shows
    through while health bars / selection rims / control-point rings still read.
- **Four new models** so units/structures/scenery/cover are all covered: `turret`, `tree`, `rock`,
  `barricade` (nine total).

**Why:** the models existing but rendering nowhere was the whole point of D41 left undone. A trivially-
parseable cooked format keeps the renderer dependency-light and bullet-proof (vs. a full glTF reader for
greybox cubes), embedding sidesteps the entire cross-platform asset-loading problem for the small
greybox set, and one shared depth-tested pipeline serves both the FPS viewmodel and the RTS tokens
without forking. It is **render-only** (invariant #1/#4): no `core`/sim/netcode touched, so the lockstep
checksum matrix is untouched; the crate stays `glam`/windowing-free (D19) — the host hands matrices in
as plain `[[f32;4];4]`, and the small transform math (`model_matrix`, `weapon_view_model`) is hand-rolled
scalar `f32`. Embodiment fairness (invariant #6) is intact: tokens are built **only** from the already-
fog-filtered draw set and **only** in the command branch (embodied returns before any token work), so the
3D pass leaks no map intel — the viz-runner dark/fairness assertions stay green.

**What this does NOT decide / honest caveats:** **command-camera framing** — the top-down ortho is kept
straight-down (a tilt would read the 3D forms better but breaks the ground-plane unproject used for
picking/marquee, so it's out of scope); at the ±40-unit command zoom infantry tokens are small (their
real ~1 m footprint), scaled up to fill their selection marker but no larger. *(Superseded by
[D56](#d56--true-scale-token-meshes-drop-the-per-kind-command-view-exaggeration): tokens now draw at
true 1.0 m scale in both views, not scaled up to the marker.)* **Unit facing** — tokens
don't yet yaw to face their velocity (greybox stand-upright). **Tank/crate placement in-game** — the sim
snapshot only distinguishes unit-vs-building, so those meshes exist + load + are golden-tested but aren't
placed in a match until a unit-kind enters the sim. **Q11 (hero-tier sourcing) stays open** — this is
greybox infrastructure, not a hero-art decision. The cooked `.mesh` is a greybox shortcut past the full
LOD/ASTC/atlas/pak cook, which remains the mid/hero target.

**Consequences:** `gen_models.py` emits `.glb` + `.mesh` and the manifest tracks both (`cooked`,
`cooked_bytes`, `cooked_sha256`) plus each model's `base_color` (mirrored in `render::mesh::ModelKind`);
new `render/src/mesh.rs` + `mesh.wgsl` (parser, GPU upload, `MeshLibrary`, the shared `MeshPipeline`, a
depth-texture helper, `model_matrix`); `render/src/world.rs` weapon viewmodel goes 2D→3D; `Renderer`
owns the mesh library/pipeline/depth and composites the command view in three passes; `shader.wgsl` gains
`FLAG_MESH`; `engine` shares one `embodied_proj` between the world camera and the viewmodel and threads
viewport size into `render()`. Pure seams unit-tested (parser + all error variants, golden-parse of every
committed `.mesh`, `model_matrix`, `weapon_view_model`, `token_for`, `embodied_proj`); the GPU path is
covered by viz-runner (all visual assertions pass; the PNGs show the 3D weapon + 3D tokens with UI decals
on top). Full suite + clippy + determinism + lockstep + viz all green.

---

## D45 — Tilt the command camera (three-quarter RTS view) so the 3D tokens read

**Status:** decided + landed (engine command camera + unprojection; viz-runner green). The
follow-on to [D44](#d44--cooked-greybox-meshes-the-glb-to-runtime-pipeline--3d-mesh-rendering) —
it **resolves D44's explicit "command-camera framing / kept straight-down" caveat**: with the
models now rendered as 3D meshes, a straight-down camera saw only their flat tops, so infantry read
as specks. This tilts the command view enough that the greybox forms read as forms.

**Decision:** the command-view camera (`engine::topdown_view_proj`) goes from straight-down
orthographic to a **fixed three-quarter tilt** — pitched `COMMAND_PITCH_DEG = 58°` above the horizon,
looking north from the south (think Company of Heroes). It stays **orthographic** (units keep a
constant on-screen size regardless of position — the RTS-legible choice) and the tilt is **pure pitch
about the world X axis: no yaw, no roll.** Pointer unprojection (`unproject_topdown`) is generalized
from "invert the matrix at one depth" (only valid looking straight down) to a **ground-plane ray
cast** (unproject the pixel at two depths, intersect `z = 0`), which is correct for the tilt and for
any future perspective camera.

**Why:** D44 made units/structures 3D, but a top-down ortho flattens a 3D model to its silhouette —
the whole point of the meshes was lost. A three-quarter tilt is the classic RTS answer (it shows
fronts and sides, and the receding ground grid adds depth) while orthographic keeps unit size
constant across the field, which top-down RTS legibility wants. **Pure pitch (no yaw) is the
load-bearing constraint:** a tilt purely about X keeps the ground↔screen mapping *axis-separable*
(screen-X depends only on world-X, screen-Y only on world-Y), so band-select's world-space
axis-aligned rectangle test (`selection::within_rect`) stays **exact with zero changes** — a yaw
would shear that and silently corrupt picking. The no-yaw invariant is pinned by a separability unit
test and documented on the constant. This is **render/input-boundary only** (host-side `f32`): the
camera and unproject never enter `core`/the sim (invariant #1) — the `Command` world coordinates that
do reach the sim still cross `world_to_fixed`, so determinism + the lockstep checksum matrix are
untouched.

**What this does NOT decide / honest caveats:** **camera control** — pitch/zoom/pan stay *fixed*; a
player-controllable command camera (and especially any **yaw**, which would require moving band-select
into screen space) is out of scope. **Picking parallax** — `unproject_topdown` returns the *ground*
point under the cursor, so tapping the visible top of a raised token lands ≈0.94 wu north of its feet
(at 58°); the zoom-aware tap pick radius (~3.5 wu at the default zoom) swallows it, so taps still
resolve the unit — a mesh-accurate ray-vs-volume pick is deferred until it's worth it. **Token
facing** — still unbuilt (D44), so tokens all face the same way under the tilt. The orthographic Y
extent foreshortens slightly with the tilt (a touch more world Y is framed); accepted.

**Consequences:** `engine::topdown_view_proj` gains `COMMAND_PITCH_DEG` / `COMMAND_EYE_DIST` and the
tilted view matrix; `unproject_topdown` becomes a ground-plane ray cast (callers — tap-pick, marquee
anchor, `pointer_world`, the gesture-scale world-per-px derivation, the radial-menu centre — are
unchanged). The old hardcoded-ortho unproject test is replaced by a project→unproject round-trip test,
plus a new test pinning axis-separability + that height reads up-screen (both fail if a yaw is ever
introduced — the regression guard for the no-yaw invariant). Full suite + clippy + viz all green; the
command/selected PNGs now show upright 3D figures with the selection-rim + health decals on top.

---

## D46 — The headless asset-tooling toolbox (one scriptable CLI per content lane)

**Status:** decided + installed (machine-wide on the workstation; no repo code yet). Generalises
D41 (the Blender method) and D44 (the cook) across the other content lanes.

**Decision:**

- Asset creation across **every** content lane uses a **headless, Claude-scriptable CLI** — the
  D41 model (script the generator, commit the script + manifest, never an opaque binary blob)
  extended from 3D to audio and 2D/UI. The chosen tools, all installed machine-wide and on PATH:
  - **Blender** (`bpy`) — 3D author: meshes, geometry-nodes terrain, rig/anim, glTF export (D41).
  - **gltfpack** — 3D cook: glTF mesh/texture compression (meshopt/Draco) for the mobile /
    200-unit budget.
  - **SoX** — SFX synthesis + processing.
  - **Csound** — deterministic, **seed-scripted** SFX, regenerable + git-diffable: the audio
    analogue of D41's procedural meshes (audio is a primary system, invariant #6).
  - **Inkscape** (`--export-type=png`) — vector → PNG HUD / command-layer UI icons across DPIs.
  - **ImageMagick** (`magick`) — scripted textures, atlases, noise / normal maps (already present).
- Recorded as project convention in [`CLAUDE.md`](../CLAUDE.md) and as the can/can't toolbox table
  in [`content-pipeline.md`](content-pipeline.md) §6, so **every session reaches for these first**
  rather than requesting commissioned art or committing binaries.

**Why:** tools existing on disk doesn't make sessions *use* them — awareness has to live in the
always-loaded project conventions, not just the workstation `~/CLAUDE.md`. One scriptable CLI per
lane keeps asset provenance uniform (script + `source`/`license`/`sha256` manifest, §3) and
reproducible, and extends D41's "no external license to vet, output is CC0-able, owned" property
from meshes to sound and UI. **Csound over SuperCollider** because it's the lighter "script a file
from a seed" fit; **gltfpack** (native prebuilt binary) **over the `gltf-transform` npm tool**
because this is a Cargo workspace with no Node manifest to pin a per-project dep into.

**What this does NOT decide:** the **hero-tier** sourcing fork (commissioned vs CC0 vs AI-gen)
stays open — [`open-questions.md`](open-questions.md) **Q11**; this is greybox/placeholder
infrastructure, the same scope as D41. No sim / `core` / netcode is touched — these are offline
authoring tools, so invariants #1 / #4 / #7 are untouched.

**Consequences:** per-tool install provenance is recorded in the workstation `~/CLAUDE.md`
("Specific tool decisions"); **Csound is a source build** (no Fedora package) so it is *not*
auto-updated by `update-all` — bump manually. New content work (a Csound going-dark alert SFX, an
Inkscape HUD icon set, a gltfpack pass over the D41/D44 greybox `.glb`s) now has a named tool and
the §6 toolbox table to point at.

---

## D47 — The "active camp" model: production panels act on the lowest-index built player camp

**Status:** decided + implemented (`engine::active_player_camp`, wired into the render panels and the
command-view production input).

**Decision:** the per-camp command UI (the train + upgrade panels, and the `train`/`upgrade` input
intents) acts on a single **active camp**, resolved deterministically as the **lowest-index, built,
operational `BuildingKind::Camp` owned by the player** (`build_ticks_left == 0`; a half-built camp is
not offered). There is **no explicit camp-selection gesture** yet — the primary (lowest-index) camp is
the implicit default. Build placement is the exception: it needs no camp (it *creates* one), placing
at the unprojected cursor ground point.

**Why:** Stage 2 needed *some* rule for "which camp does Train/Upgrade target," and the choice has to
be **deterministic and identical on every peer** (invariants #1/#7) because it feeds the lockstep
command stream. Entity-index order is the cheapest stable, peer-identical key — no sim state, no
float, no RNG, no autonomy (invariant #3). A real per-camp *selection* (click a camp → it becomes
active) is a genuine input-model feature with its own UI affordance and is deferred rather than
silently half-built; until then the deterministic default keeps the feature usable and the seam
(`active_player_camp`) is pure + unit-tested. Most early scenes have one camp, so the default is
rarely surprising in practice.

**Consequences:** `active_player_camp` is called twice per command frame (once pre-step to resolve the
input target, once at render time for the panels) — two cheap pure reads, deliberately separate so
input sees pre-step state and the panels see post-step state. When explicit camp selection lands it
**supersedes** this default (a `selected_camp: Option<Entity>` overriding the lowest-index fallback),
and the render/input call sites swap the resolver — the downstream seams
(`train_commands`/`upgrade_commands`, which already take an `Option<Entity>`) don't change shape. A
camp **rally** point is still a flagged follow-up (no `Command` for a building spawn-rally exists —
see `train_ui::rally_point`).

---

## D48 — Desktop command-view production keybinds: B / R / H / U

**Status:** decided + implemented (`pal-desktop`); touch bindings deferred.

**Decision:** the desktop command view binds the Phase-2 "command and grow your camps" production
intents to mnemonic letter keys, distinct from the `1`–`0` order/stance vocabulary (D25):

| key | intent | seam → command |
|-----|--------|----------------|
| `B` | place a **Camp** at the cursor's ground point | `build_ui::build_commands` → `Command::Build` |
| `R` | queue a **Rifleman** at the active camp | `train_ui::train_commands` (slot 0) → `Command::QueueProduction` |
| `H` | queue a **Heavy** at the active camp | `train_ui::train_commands` (slot 1) → `Command::QueueProduction` |
| `U` | **upgrade** the active camp one tier | `upgrade_ui::upgrade_commands` → `Command::Upgrade` |

All four are **edge-latched** (fire once on the press, ignore OS key-repeat, clear on drain) like the
vocabulary slot keys, and are **command-view only** — the engine ignores them while embodied
(invariant #6: no command-layer production while the map is dark). Build places at wherever the cursor
hovers, so it needs no separate "armed-then-click" mode.

**Why:** the digit keys are already the order/stance vocabulary, so production needs its own keys;
mnemonic letters (**B**uild, **R**ifleman, **H**eavy, **U**pgrade) are more memorable than another
numeric bank and stay clear of the `WASD`/`E`/`Q`/`F`/`Space` embodied-combat cluster. A single key
per unit type (rather than a "select-then-number" palette) keeps the desktop flow direct for the tiny
current roster; if the roster grows past a handful, this moves to a palette + slot scheme without
disturbing the `*_slot: Option<u8>` `InputFrame` fields (the backend just maps more keys onto the
existing slots). These are **desktop** bindings; the **touch** equivalent is on-screen palette/panel
buttons hit-tested onto the same `InputFrame` edges (`building_slot`/`train_slot`/`upgrade_pressed`),
deferred with the rest of the on-screen command UI (the radial-menu slice, D43) and TODO-flagged in
`pal-android`.

**Consequences:** three new `InputFrame` edges (`building_slot`, `train_slot`, `upgrade_pressed`)
cross the PAL boundary; the engine consumes them through the pure, tested
`command_view_production_commands` seam. When touch buttons or remappable keybinds land they
**extend** (not replace) this entry. No `core`/sim change — the emitted commands already existed; this
only adds an input path to them (invariants #1/#4/#7 untouched).

---

## D49 — A real LOD chain for the placeholder models: gltfpack decimation tiers + distance-picked runtime selection

**Status:** decided + landed (`tools/models/gen_models.py` + `render/src/mesh.rs`); ASTC/atlas/pak
half of the cook chain deferred to Phase 3.

**Decision:** The cooked-mesh pipeline (D44) shipped a single full-detail tier per model. We now run every
placeholder `.glb` through **gltfpack** in the cook step (`tools/models/gen_models.py`) to emit
**three LOD tiers** per model — LOD0 (full), LOD1 (`-si 0.5`, ≈½ tris) and LOD2 (chained off LOD1,
≈¼ tris) — each re-imported into Blender and re-cooked through the unchanged `export_mesh` so all
tiers share the identical `GDM1` flat-shaded format. Files follow a fixed naming contract:
`<name>.mesh` (LOD0), `<name>.lod1.mesh`, `<name>.lod2.mesh`. The manifest gained a per-asset `lods`
array (level / ratio / tri-count / sha256) for license + provenance hygiene. At runtime the
`MeshLibrary` loads all tiers (`render/src/mesh.rs`, `get_lod`) and a pure, tested
`mesh::select_lod(distance)` picks a tier by eye distance — coarser scenery past 10 m / 22 m.

```text
 .glb (LOD0) ──gltfpack -si 0.5──▶ .lod1.glb ──gltfpack -si 0.5──▶ .lod2.glb
     │                                  │                               │
  export_mesh                       export_mesh                     export_mesh   (Blender re-cook)
     ▼                                  ▼                               ▼
 <name>.mesh                     <name>.lod1.mesh                <name>.lod2.mesh   (all GDM1)
```

**Why:** the 200-unit mobile budget (the honest Phase-3 caveat) wants distant geometry to cost
fewer triangles; the content pipeline (`content-pipeline.md` §2) always named gltfpack for exactly
this, and pulling it forward to the *placeholders* keeps the greybox tier on the same cook→LOD chain
the hero tier will use, so the runtime LOD machinery is proven before any final art exists. LOD0
bytes are held **byte-identical** to D44's committed meshes (the builder funcs + `export_mesh` were
untouched), so the existing golden mesh tests and manifest shas are undisturbed.

**Consequences:** render-only (invariants #1/#4 untouched — no sim sees a mesh). gltfpack's
aggressive simplify (`-sa`) is required because the flat-shaded soup splits normals at every face;
already-minimal props (crate/rock/barricade) "floor out" (LOD1 == LOD0 geometry) and still emit a
valid tier so the loader stays uniform. A pre-existing non-determinism in Blender's UV-sphere
tessellation (trooper/tree/rock) means a clean regen can perturb those three LOD0 files run-to-run;
documented in `tools/models/README.md`, flagged for a follow-up that would need to touch the
builders. The full ASTC/atlas/LZ4-pak half of the cook chain remains Phase-3 follow-on.

---

## D50 — Wire the placeholder model library to the sim: unit-kind tokens, the tank, and first-person world dressing

**Status:** decided + landed (`core` unit_kind + `render`/`engine` token & prop wiring); cross-platform
checksum stream verified byte-identical over 300 ticks.

**Decision:** The mesh library held nine models but the game drew only three (infantry token, camp, weapon).
This wires the rest. (1) A render-facing **`unit_kind`** component now rides the ECS (`core`,
mirroring `EntityKind`) and the render snapshot (`UnitSnapshot.unit_kind`), set deterministically at
the production spawn — kept **out of the per-tick checksum** (its gameplay effect is already in the
spawned `health`/`weapon`; the save/resume codec carries it outside the `fold`, `SNAPSHOT_VERSION`
1→2). (2) `render`'s `model_for_unit` maps the snapshot kind to a token mesh — **Heavy → tank**,
Rifleman → infantry, building → camp — and the command-view token pass buckets by `ModelKind` so any
model draws. (3) The embodied view gained **static world dressing** (`render_world_props` +
`PROP_LAYOUT`): trees, boulders, crates, sandbag berms and turret emplacements scattered as cosmetic
scenery/cover, drawn at the LOD picked by eye distance (D49), so "going dark" lands you in a *place*
rather than a bare ground/sky void.

**Why:** the placeholder roster (D41) only earns its keep once the sim actually selects between
models; a per-entity unit-kind is the minimal honest seam (buildings already had `BuildingKind`,
units discarded theirs after spawn). Keeping `unit_kind` out of the checksum preserves the committed
cross-platform streams (verified byte-identical over 300 ticks) — it is a presentation label, not
sim state, so it belongs on the render side of the snapshot (invariant #4). The world props are a
**fixed compile-time layout with no sim entity behind them** — they reveal no unit/enemy/map intel,
so they stay fair under "world goes dark" (invariant #6): scenery is terrain, not the strategic
picture the dark frame is supposed to take away.

**Consequences:** the `model: u32` token index is a trailing field on the Pod `UnitInstance`,
unreferenced by the quad pipeline's instance attributes (offsets 0–31), so the GPU layout is
unchanged. No float crosses into `core` (the LOD distance + placement math is render-only). When the
roster or building set grows, `model_for_unit` and `PROP_LAYOUT` extend; if props ever need to be
gameplay cover they must become sim entities and pass through the snapshot like any other unit
(never a render-side back-channel to the sim — invariant #4).

---

## D51 — Embodied FPS controls: ammo+reload+crouch mechanics + the COD-style on-screen touch HUD

**Status:** decided + landed (`core` mechanics + `pal`/`engine` touch seam + `pal-android` multi-touch
forwarding + `render` on-screen GUI); workspace green, `core` determinism tests pass in both profiles,
sim-runner checksum stream verified bit-stable across runs.

**Decision:** Build the shipping first-person control scheme that [D14](#d14--phase-0-control-prototype-passes-touch-feel-risk-retired)'s Phase-0 prototype validated
and deferred. Two halves:

(1) **Three embodied combat mechanics, all deterministic sim state** (fixed-point, folded into the
per-tick checksum + the authoritative snapshot; `SNAPSHOT_VERSION` 2→3, `WIRE_VERSION` 4→5):
  - **Ammo + reload** — an *opt-in* magazine on `Weapon` (`mag_size`/`ammo`/`reload_ticks`/`reload_left`).
    `mag_size == 0` means **no magazine** (infinite ammo), which every AI/auto-combat unit and every
    pre-existing test uses, so the `combat_system` engage pass is untouched. The gate lives ONLY in the
    embodied fire path (`combat::resolve_fire`): an empty mag or in-progress reload is a silent dry click
    (no cooldown spent). `Command::Reload` starts the timer; combat upkeep counts it down and refills.
    The playable Rifleman/Heavy get real 30/50-round mags from `economy::unit_stats`.
  - **Crouch posture** — a per-unit `Posture {Standing,Crouched}` array. Crouch is a *marksman stance*:
    half move speed (`systems::CROUCH_MOVE_SPEED`), a tighter aim cone (`FIRE_CONE_COS_HALF_CROUCHED`) and
    +25% range (`CROUCH_RANGE_BONUS`) — a deliberate "aim true, reach further, can't reposition" trade.
    Player-only state (`Command::Crouch`, a toggle resolved off authoritative sim posture so the host
    holds no toggle bit); AI units stay `Standing` (literal-executor, invariant #3).

(2) **The Android on-screen FPS HUD** — a floating **left move stick**, a **right drag-to-look region**
(no visible stick; COD-Mobile feel), and floating **Fire / Crouch / Reload / Surface** buttons. The pure,
host-tested `engine::touch_controls` seam turns raw `InputFrame.touches` into the embodied intents (the
testable logic an Android `MotionEvent` can't host); `pal-android` only forwards the down-pointer set;
`render::touch_controls` draws the controls as a screen-space LOAD overlay with shader-drawn glyphs (no
binary art). Desktop keeps keyboard+mouse (C=crouch, V=reload) — the GUI is **Android-only**.

**The Surface button supersedes the two-finger toggle while embodied.** Twin-stick play means two
fingers are *constantly* down (move + look), so the old two-finger embody/surface gesture would eject the
player mid-fight. The Android two-finger gesture is now **embody-only** (`map_input_commands` no-ops it
when already embodied); ejecting is the on-screen Surface button.

**Why:** the embodied view was uncontrollable on a touchscreen (`pal-android` forwarded only a single
command-layer pointer). Move/look/fire already had deterministic seams; ammo/reload/crouch are the buttons
those FPS controls imply, and the user chose full mechanics over dead buttons. Scoping ammo/reload/crouch
to the **embodied path only** keeps RTS auto-combat balance byte-identical (the engage pass never reads the
new fields) and keeps AI units literal executors that never manage ammo. Keeping the new state in the
checksum is mandatory — it affects fire/movement, so an unfolded field would desync lockstep silently
(invariant #1/#7). The testable logic lives in `engine` (not `pal-android`) per the standing seam rule.

**Consequences:** new sim state means the lockstep/snapshot codecs grew (versioned, so a stale peer is
rejected at the handshake, not desynced mid-session). Crouch's tighter cone is a *downside for sloppy aim*
paid for by the range bonus — tune against feel later (the cone/range/speed constants are baseline, not
locked). The on-screen icons are shader glyphs for now; real Inkscape-exported art is later polish (D46
pipeline), not a blocker. Gyro aim is a deferred optional aid. The numbers (mag sizes, reload ticks,
crouch multipliers) are a playtest baseline.

---

## D52 — Embodied view draws fog-filtered avatar-visible units; post-match DISMISS wiring

**Status:** decided + landed. Playability push, finishing the embodied first-person view ([D40](#d40--embodied-world-rendering-a-real-fps-world-drawn-while-the-strategic-map-stays-dark)) and the post-match shell ([D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)/[D34](#d34--the-shellsim-seam-a-gpu-free-logic-free-coreshell-façade-intent-in-view-out)).

**Decision:**

- **The embodied pass now draws sim units, not just the procedural world.** While embodied, the
  first-person mesh pass renders the **fog-filtered, avatar-visible** sim entities (`unit_draw_plan` in
  `render::lib`; `render_world_props` → `render_world_meshes`) alongside the D40 sky/ground/weapon. An
  enemy physically in the avatar's line of sight is now actually *drawn* — previously the embodied view
  could not show any sim entity at all.
- **Fairness (invariant #6) is preserved structurally by the fog filter, not by entity-withholding.**
  The avatar-visible set comes from `render::fog::visible_instances`, which drops the embodied self
  (`FLAG_EMBODIED`), the strategic map, and control-point rings — leaving only what the avatar's own
  line of sight legitimately sees. This **supersedes the D40 mechanism** ("the world pass has no access
  to sim entities, so it cannot leak intel even in principle"): the pass now *does* see entities; the fog
  filter is the guarantee.
- **Post-match DISMISS leaves the match and returns to title.** The post-match summary overlay's DISMISS
  button gets a pure NDC hit-test seam (`render::overlay::button_slot_at`) and an `ExitToTitle` host
  transition (`app::shell`), so DISMISS tears down the in-session shell and returns to the title screen
  instead of being a dead control.

**Why:** a void that can never draw an in-line-of-sight enemy is both unfair-feeling *and* unreadable —
the going-dark cost is meant to be *losing strategic intel*, not *blindness in front of you* (invariant
#6, [game-design.md](game-design.md) §6). Routing visibility through the existing `render::fog` filter
keeps the fairness boundary in one place that's already the source of truth for command-view fog, rather
than relying on the fragile "the renderer simply can't see entities" property. The DISMISS wiring closes
the obvious loop: a summary you can't dismiss is a dead end.

**Consequences:** D40's "no access to sim entities" claim is now stale and carries a superseding note.
The embodied view's correctness now depends on `visible_instances` being exactly the avatar's line of
sight — the viz-runner strategic-map-collapse fairness assertion still guards this. The DISMISS path adds
a host-level `ExitToTitle` transition; the hit-test is a pure function (host-tested), per the standing
seam rule.

## D53 — Wire the pause-overlay trigger: Esc opens pause; in-match surrender becomes reachable

**Status:** decided + landed. Closes the in-session shell ([phase-4-plan WS-B](plans/phase-4-plan.md)), the
sibling of the post-match DISMISS wiring ([D52](#d52--embodied-view-draws-fog-filtered-avatar-visible-units-post-match-dismiss-wiring)).

**Decision:**

- **The pause overlay finally has a trigger.** `engine::session_shell`'s pause/surrender state machine
  and `render::overlay`'s chrome were already built and tested ([D34](#d34--the-shellsim-seam-a-gpu-free-logic-free-coreshell-façade-intent-in-view-out)), but nothing *opened* the
  pause menu in a live match — it was unreachable. A new pure seam `pause_toggle_action(surface) ->
  Option<SessionAction>` (Playing → Pause, Paused → Resume, `None` on Ended/ReconnectPrompt) plus thin
  `Game::toggle_pause` / `Game::shell_overlay_active` (over the pure `overlay_active` seam) close that.
- **Desktop binds the pause toggle to Esc.** Esc was the sticky free-cursor toggle; it is now the
  conventional pause key. The transient **Left-Alt** free-cursor (e.g. to alt-tab) stays; opening any
  shell overlay frees the cursor on its own so the menu's buttons are clickable.
- **In-match surrender is reachable through the existing path.** Once Paused is on screen, the
  already-wired `overlay_click` slots reach **Resume** (slot 0) and **Surrender** (slot 1) → the
  host-side summary → DISMISS → return-to-title (D52). No new surrender plumbing — the trigger was the
  only missing link.
- **The match freezes under any overlay.** While a shell overlay is up the host feeds a neutral input
  frame, so a click that misses an overlay button (or a held key) can't drive selection / fire the
  weapon / pan the camera behind the menu. The overlay's own buttons resolve *before* the blanking.

**Why:** every downstream piece — the state machine, the overlay render, the Resume/Surrender slot map,
the summary assembler, the DISMISS→title transition — was built and green, but a pause menu you cannot
open is a dead feature; surrender rode on it being reachable. The fix is host-side input wiring, not new
design: pause is a host/session `SessionAction` that never enters the lockstep input stream, so the
per-tick checksum is byte-identical (invariants #1/#4) and the single-player tick halt stays the existing
`halts_local_tick` rule (lockstep keeps stepping — a local pause is an overlay, never a peer-agreed sim
pause).

**Consequences:** the roadmap's in-session-shell checklist item is now fully done (pause + surrender +
post-match summary). Esc no longer toggles the sticky free-cursor mode (subsumed by overlay-frees-cursor
+ Left-Alt). The pause *decision* logic is pure and unit-tested (`pause_toggle_action`, `overlay_active`);
the winit/Esc host glue is the only un-constructible seam, exempt per the standing testing rule. Android
back-gesture → `toggle_pause` is the natural follow-up (the engine seam is platform-neutral); only the
desktop binding landed here. *(Superseded: the Android binding landed in
[D54](#d54--android-back-gesture-pause-binding--the-platform-twin-of-d53).)*

## D54 — Android back-gesture pause binding — the platform twin of D53

**Status:** decided + landed. The Android counterpart of the desktop pause wiring
([D53](#d53--wire-the-pause-overlay-trigger-esc-opens-pause-in-match-surrender-becomes-reachable)),
which D53 flagged as the natural follow-up.

**Decision:**

- **The system back gesture toggles the in-session pause overlay.** `pal-android` maps `Keycode::Back`
  (Down edge) to a host-side `back_pressed` edge that `android_main` drains (`AndroidInput::take_back_pressed`)
  and routes to `Game::toggle_pause` — the exact platform twin of desktop's Esc. Like Esc, it is handled
  **outside** the deterministic `InputFrame`, so the sim/checksum stream is untouched (invariants #1/#4).
  Back opens the pause menu while playing and resumes while paused (the `pause_toggle_action` map, D53).
- **Back is always consumed; it never falls through to the OS "finish activity" default.** Returning
  `InputStatus::Handled` for the back key suppresses Android's default back-exits-the-activity behavior,
  so back is a true pause toggle rather than an app-exit. Leaving the match is the pause menu's job.
- **The match freezes under any overlay.** While `Game::shell_overlay_active()` is true, `android_main`
  feeds a neutral `InputFrame`, so touches behind the menu can't select units / fire / pan the camera —
  the same freeze the desktop host applies. Single-player pause also halts the tick (`halts_local_tick`);
  this stops *world input*, not the clock.

**Why:** D53 left Android able to open no pause menu at all; the back gesture is the conventional mobile
pause affordance and the engine seam was already platform-neutral, so the binding is pure host glue. Back
was previously swallowed as a no-op (the key path returned `Handled` but mapped nothing), so routing it to
pause is a strict improvement with no behavior regressed. Keeping it out of the `InputFrame` mirrors the
desktop Esc rationale and preserves determinism by construction.

**Consequences:** Android now has a usable pause (open + resume via back) with the world correctly frozen
underneath. **Deferred — the Android leave-to-title path:** tapping the pause menu's **Surrender** or the
post-match **DISMISS** button is *not* wired on Android, because finishing the `NativeActivity` to return
to the Compose title ([D35](#d35--first-native-app-shell-surface-the-android-compose-boot--title-landing-screen)) needs a
JNI `Activity.finish()` call — there is no `AndroidApp::finish()` in android-activity 0.6. Wiring it now
would strand the player on an undismissable `Ended` summary, so overlay-button taps stay desktop-only (the
Android twin of D52's desktop-only `ExitToTitle`) until that JNI path lands. The new Android code is host/PAL
glue over already-tested engine seams (`pause_toggle_action`, `shell_overlay_active`); `AndroidApp`/`KeyEvent`
are un-constructible off-device, so it carries the same standing test-exemption as the sibling
`apply_motion`/`apply_key`/`capture_touches`.

## D55 — Tank embodiment goes War Thunder-flavoured: independent hull/turret + all-unit armour facing

**Status:** decided + plan recorded; **P1–P4 landed** — `trig::atan2`/`rotate_toward` (`a5812fb`),
hull/turret heading + inertia + slew (`c1e4059`), the ballistic projectile pool (`4fbe31b`), and the
all-unit armour-facing rewrite — `Armor{front,side,rear}` + `Weapon.penetration` + a shared
`facing_penetration_multiplier` resolved at impact across all three damage sites (`dc8ce4e`);
P5–P9 phased. Verified green (301 core tests dev+release; 2-peer lockstep agrees over 300 ticks;
`WIRE_VERSION` 6 — P4 adds no command, `SNAPSHOT_VERSION` 5→6). Reference feel **War Thunder (sim)**; both follow-up
forks settled — ballistic flight is a **core phase** (not deferred) and the tank is the **deep**
embodiment by design. Full plan: [`tank-embodiment-plan.md`](plans/tank-embodiment-plan.md).

**Decision:** the embodied tank stops being "infantry-FPS in a tank-shaped token" (D50–D52, where
`Heavy` merely renders as a tank mesh and drives with the rifleman scheme of D51) and becomes a
real **vehicle**, anchored on **War Thunder (sim)** feel within the fixed-point/lockstep envelope:

- **Hull heading and turret bearing are independent, first-class sim state.** Two new
  per-entity `Angle`s (`hull_heading`, `turret_yaw`) — *none exist today*; facing is currently
  derived from velocity in render only. The turret slews toward the aim at a rate-limited
  `turret_speed`/tick. `turret_speed == 0` means "no turret" (locked to hull), which is **every
  infantry unit** — so the system is **opt-in by a zero default**, exactly like `mag_size == 0`
  disables the magazine (D51). Non-tank entities cost nothing and move the checksum by nothing.
- **Penetration-vs-armour-facing becomes the combat model for ALL units**, not an embodied-only
  bonus. A new `Armor{front,side,rear}` component (**default all-zero = unarmoured**) and a
  `Weapon.penetration` field add a `facing_penetration_multiplier` to the damage step. The hit
  facet is chosen from the dot product of shot-direction vs hull-heading — the **same
  squared-cosine, sqrt-free trick** the aim cone already uses. An unarmoured defender always
  takes the multiplier as `1.0`, so **existing infantry balance and every combat test are
  byte-for-byte unchanged**; only the new armoured **`UnitKind::Tank`** gets bounce/flank texture
  (front shots can hard-bounce; flank/rear pen). It applies to AI-driven *and* embodied tanks.
- **Aim is a skill, via dispersion + slow traverse**, not a tighter cone: a reticle that blooms
  on the move/traverse and settles at rest. **Skill-honest dispersion (refined):** a *fully
  settled* gun fires dead-on `turret_yaw` with **zero scatter** — only an unsettled gun scatters,
  the offset scaling with `dispersion` — so a perfect aim is never robbed by an RNG bullet. The
  bounded scatter still uses `combat`'s reserved `&mut Rng` (integer draw, deterministic). Shell
  types (AP/APHE/HE) trade penetration against damage/splash via a `SelectShell` command.
- **Ballistic shell flight is a CORE phase, not deferred (fork resolved).** Travel time +
  leading + drop is War Thunder's soul, so it ships as real fixed-point projectiles. Crucially,
  resolving facing **at impact** (not at fire time) means a shell that catches a tank mid-turn
  hits the side it rotated into — so building ballistics *before* armour facing avoids a
  hitscan-then-projectile rework. Verticality is **localized to the projectile** (units stay 2D
  at a per-kind hull height; only the shell carries `height`+`vz` and integrates gravity), so the
  signature *drop* lands without forcing a world z-axis. A bounded projectile **ring** caps shell
  count against the Phase-3 thermal budget. `muzzle_vel == 0` ⇒ hitscan (infantry) — same
  zero-default opt-in. ([Q13](open-questions.md) resolves here.)
- **The tank is the project's DEEP embodiment — an intended asymmetry.** Unlike D51's deliberately
  shallow infantry (move/aim/crouch/reload), the tank is rich and sticky *on purpose*: it's the
  unit you commit to and master. The pillar tension this creates (a rewarding embodiment vs. the
  "cost is time away" rule) is held by the **existing** levers — going-dark blindness + the
  precious-unit economy — not by flattening the tank. If playtest shows tanks over-reward camping,
  the dial is the going-dark cost, not the tank's depth.
- **Deferred to their own later decisions:** **module/crew damage** (tracks/breech/ammo-rack) and
  a **true world z-axis** (unit elevation / multi-storey cover — the projectile-local height above
  covers drop without it). Both ride cleanly on this spine once it ships and proves fun.

**Why:** the user picked War Thunder fidelity and all-unit armour facing explicitly. The defining
tank mechanic — hull≠turret and angle-your-armour — is what makes embodying a tank *mechanically
better* than letting the AI drive it (the §5 acceptance bar), and routing it through new
per-entity components keeps embodiment a pure input-swap (invariant #5) rather than a vehicle
object. Gating both systems behind a zero default is what lets an "all-unit" combat-model rewrite
land **without** disturbing the D30-tuned infantry balance or the lockstep checksum — the
determinism risk (invariant #7) is contained to scenes that actually field armour. Fixed-point
`atan2`/`rotate_toward` and integer penetration LUTs keep the whole thing float-free (invariant
#1); the AI tank still only points where its order/stance already aims (invariant #3).

**Consequences:** new pure `trig` angle math (**P1, done — isolated, fully tested, committed**),
then heading state + hull inertia + slew (P2), the ballistic projectile pool (P3), the
impact-resolved all-unit armour rewrite (P4), then dispersion/shells/render/HUD (P5–P9). **P3 and
P4 both add checksummed sim state** (projectiles and the damage rewrite), so each ships with
cross-arch checksum coverage and runs through `/safe-edit`. Until P4 lands, combat damage is
unchanged. The embodied tank HUD diverges from the infantry HUD (hull-relative turret indicator,
dispersion reticle, **lead pip**, shell selector) — a render/`engine` follow-up, not a sim change.
A tank gun reuses D51's magazine as `mag_size = 1` + a long reload (no new reload code).

## D56 — True-scale token meshes: drop the per-kind command-view exaggeration

**Status:** decided + landed. Settles the token-scale hedge [D44](#d44--cooked-greybox-meshes-the-glb-to-runtime-pipeline--3d-mesh-rendering)
left open ("scaled up to fill their selection marker") now that the embodied first-person view puts
the meshes next to metre-scale scenery.

**Decision:**

- **Every 3D token mesh draws at true 1.0 metre scale, in both views.** The greybox models are
  authored in real-world metres (`tools/models/gen_models.py`: a trooper ~1.74 m tall, a tank
  ~3.2 m long, the camp ~3.5 m across), so a single `render::TOKEN_SCALE = 1.0` is honest scale —
  relative sizes are truthful and a unit stands at its real height beside the metre-scale
  `PROP_LAYOUT` scenery in the embodied view.
- **This replaces the per-kind cosmetic exaggeration** (infantry ×2.2, tank ×0.42, building ×2.2)
  that D44 introduced to make top-down tokens read as map markers. That distorted relative size —
  a trooper drawn *larger* than a tank, and a 3.8 m soldier towering over the 1.5 m embodied eye —
  which is exactly what reads as "wrong scale / buildings don't look like buildings" in first
  person. Map-marker readability is now a *camera/zoom* concern, not a per-mesh fudge.

**Why:** the user asked for models to be to scale everywhere. The exaggeration was a command-view
readability hack that leaked into the shared `token_meshes` seam and therefore into the embodied
view, where it has no business — the FPS view wants physical truth. Render-only (invariant #4): no
sim state touched, no determinism impact.

**Consequences:** top-down tokens are now physically sized (smaller at the ±40-unit command zoom);
if that proves too small to click, the dial is the command camera (zoom / a future tilt), not the
mesh scale. The tank hull + turret still share one scale, so the turret still seats on the ring
(P7). The 2D command-view footprint quad (`BUILDING_HALF`) is unchanged and is a separate marker
from the 3D mesh.

## D57 — Buildings are solid: a fixed-point footprint push-out in the sim step

**Status:** decided + landed. Adds the first real movement obstacle, which
[`flow_field`](../core/src/flow_field.rs) had explicitly deferred ("Phase 1 has no obstacles").

**Decision:**

- **Buildings block movement via a circular footprint, resolved as a post-movement push-out.**
  A new `core::systems::resolve_building_collisions` runs in `Sim::step` **after all movement**
  (the embodied avatar moved in the command phase, AI units in `order_system`) and **before** the
  cosmetic slew / combat: any non-building entity whose centre is within
  `BUILDING_RADIUS (1.75 m) + UNIT_RADIUS (0.25 m)` of a building centre is snapped radially back
  onto that boundary circle. A unit exactly on the centre (no defined normal) is ejected along `+X`
  — a fixed, peer-identical choice.
- **It is push-out, not flow-field obstacle cost.** The flow field still degenerates to "point at
  the goal"; solidity is a cheap positional correction layered on top, not a re-route. Local
  steering/avoidance remains the deferred Layer-3 design target (`architecture.md`).
- **It applies to the embodied player and AI units alike** — this is physics, not autonomy, so it
  does **not** touch invariant #3: an ordered unit still walks where it was told, it just can't
  occupy a wall.

**Why:** the user reported walking straight through camps. All-integer fixed-point (`len_sq`,
`normalized` over the deterministic fixed `sqrt`, `scale`), iterated in stable index order, with a
deterministic degenerate-case eject, keeps it bit-identical across the lockstep matrix
(invariants #1/#7). The truncating fixed `sqrt` makes the normalize overshoot slightly, so a pushed
unit lands *on or just outside* the boundary — never inside — which makes the pass idempotent.

**Consequences:** the deterministic step order is now **move → collide → orient → fight → capture →
economy** (extends [D23](#d23--phase-2-game-systems-the-deterministic-model-and-its-module-decomposition)
and D55's `orient` step). Ships with unit tests for the resolver and a `Sim::step`-level integration
test (a unit driven into a building) so the collide step's wiring rides the `determinism.yml`
cross-arch run. **Deferred:** non-circular footprints, building-vs-building placement rules, and
flow-field re-routing around structures.

---

## D58 — PvE-first: the Operations campaign is the first shippable product (resolves Q5)

**Status:** decided (design). Resolves [Q5](open-questions.md) — *single-player, multiplayer, or
both, and in what order* — which had carried a soft "likely PvE-first" lean since pre-production.

**Decision:**

- **The first shippable product is single-player PvE** — a campaign of missions
  ([`pve-campaign.md`](pve-campaign.md)). **PvP is a fast-follow** riding the same
  deterministic-lockstep core, not a parallel track.
- PvE is the **onboarding surface for the going-dark mechanic** (invariant #6): a controlled
  place to teach a new player the blindness cost *before* they ever face a human.
- Single-player runs the **existing** `core::lockstep` loop as a 1-peer, delay-0 session
  ([D27](decisions.md)) — no new netcode is in the critical path to ship.

**Why:** PvE derisks the two scariest unknowns *independently* — is the core loop **fun**
(provable single-player) and does it hold up **over the wire** (Phase 3) — instead of betting
both at once. It is also the only honest way to teach invariant #6: a stranger's first match
cannot be against another human. The lockstep-ready architecture means choosing PvE-first
costs nothing toward the PvP fast-follow — the sim, order vocabulary, and netcode are
single-sourced (invariant #2).

**Consequences:** the roadmap gains a dedicated **Operations-campaign** build section (the
first shippable slice), sequenced in [`pve-campaign-plan.md`](plans/pve-campaign-plan.md). The
PvP-specific forks ([Q1](open-questions.md) thread-thinness, [Q3](open-questions.md) leash,
the PvP attention mind-game) stay open — PvE-first does not resolve them, it defers their
*lock* to when live PvP exists. Opens [Q14](open-questions.md) (co-op PvE).

---

## D59 — The Operations-hub campaign + a host-side objective system

**Status:** decided (design). The structural design of the D58 campaign.

**Decision:**

- **Structure = an Operations hub** (Company of Heroes meta-map + Delta-Force *Operations*):
  a **node-graph of replayable missions**, not a linear reel. Clearing a node unlocks its
  successors; any cleared node replays at higher difficulty. **Modifiers** (Destiny-2-style
  rotation) change **scenario parameters** (force size, reinforcement cadence, fog rules,
  time limits) — **never balance numbers** — so the measured combat/economy baseline
  ([D30](decisions.md)) and determinism are untouched.
- **Missions are data, not engine:** each is a **parameterized scenario** (a starting world via
  the data-driven `Sim::new` + spawn path) plus an **objective set**. Four archetypes ship the
  verbs — **Seize** (the "10 troops, take the base" first mission), **Hold**, **Assassinate/
  Extract**, **Push**.
- **Objectives are host-side, not sim state.** An `ObjectiveSet` is evaluated **after
  `Sim::step`** by reading the per-tick `SimEvent` stream + already-derived faction reads —
  the **same footing as `evaluate_outcome` ([D38](decisions.md))** and fog/alerts/tell
  ([D23](decisions.md)/[D33](decisions.md)). It generalizes `evaluate_outcome`'s
  elimination/territory/timeout rules rather than replacing them.
- **Difficulty extends the honest commander** ([`commander_orders`](../core/src/commander.rs),
  [D39](decisions.md)) with a deterministic tier (reserve/unit-mix/cadence/aggression knobs on
  the seeded planner). It **must never** become omniscient ("you're embodied, attack now") —
  that is the cheap punisher [`game-design.md`](game-design.md) §9 forbids and would break
  invariant #6.

**Why:** keeping objectives **out of the checksum fold** means missions can be authored,
tuned, and reshuffled with **zero lockstep/desync risk** (invariant #7) and zero new cross-arch
coverage for the objective layer itself — it observes the sim, it never changes it. Expressing
every borrowed idea (Halo set-pieces, CoH territory objectives, Delta-Force replayable ops,
Destiny modifiers) as a *scenario parameter* or a *host-side objective* is what keeps the whole
content pillar from reopening a locked invariant.

**Consequences:** new host-side `Objective`/`ObjectiveSet` types + `ObjectiveCompleted/Failed`
events feeding the existing `MatchSummary` and a new in-match objective HUD; a `difficulty`
parameter threaded into the commander. First code slice (objective evaluator + mission 1, with
`core`/`engine` tests green dev+release and the determinism matrix green) is
[`pve-campaign-plan.md`](plans/pve-campaign-plan.md) WS-A. Deferred: mission authoring format
([Q15](open-questions.md) — since **resolved** by [D76](#d76--missionscenario-authoring-format-external-ron-data-files-behind-a-host-side-loader-resolves-q15)),
narrative depth ([Q16](open-questions.md)).

---

## D60 — Horizontal weapon customization: a sidegrade gunsmith, never an upgrade tree

**Status:** decided (design). Reaffirms [D13](decisions.md) (cosmetic-only, no pay-to-win)
under the new progression surface.

**Decision:**

- **The gunsmith is horizontal.** A CoD-Mobile-style attachment-slot system on the embodied
  weapon ([D51](decisions.md)) where **every attachment is a trade, not an upgrade** (long
  barrel → +range / −ADS speed; grip → +recoil control / −handling). Design rule: **no
  strictly-dominant build** — the same anti-degeneracy bar [D30](decisions.md) holds units to.
  A loadout is a *playstyle*, not a *power tier*.
- **Loadout stat deltas are sim state, handled deterministically.** They are **fixed-point
  (Q16.16, [D17](decisions.md))**, applied to the weapon component **at match start** as
  match-setup **input** (never mutated live), and therefore **folded into the per-tick
  checksum** ([D28](decisions.md)) — a loadout divergence is caught by the cross-arch matrix
  (invariant #7) like any other. **No floats** (invariant #1).
- **Cosmetics stay strictly presentation-layer** (skins/paint/charms): render-only, can't
  touch determinism, hitboxes, silhouette readability, or the gone-dark tell — the
  [D13](decisions.md) guardrails. **Unlocks grant content** (more attachment options, units,
  maps), never raw power.

**Why:** a stat-raising attachment tree would be pay-to-win or grind-to-win, detonating pillar
4 and D13 — the fairness argument the entire game rests on. Horizontal sidegrades give the
gunsmith real depth *without* a power axis, so it can carry into PvP untouched. Putting the one
sim-touching part (function deltas) through the fixed-point/checksum path keeps the
customization from becoming a determinism hole; keeping looks render-only keeps cosmetics free
of the sim entirely.

**Consequences:** a fixed-point attachment-delta table in `core` (checksum-folded) + a
pre-match loadout UI on the command layer; the cosmetic catalogue feeds the
[D13](decisions.md)/[Q9](open-questions.md) store. Build slice: [`pve-campaign-plan.md`](plans/pve-campaign-plan.md)
WS-C. Full design: [`customization.md`](customization.md).

---

## D61 — Mobile HUD customization: a per-layer layout editor, presentation-only

**Status:** decided (design). Realizes the roadmap "Touch-layout / rebind editor" item as a
concrete feature, scoped against invariant #6.

**Decision:**

- **A CoD-Mobile / Mobile-Legends layout editor** for the touch controls: drag, resize, and
  opacity for **every** on-screen control, with **per-layer presets** (the command layer and
  the embodied layer are different control sets — [D51](decisions.md)), multiple saved presets,
  and reset-to-default.
- **Pure presentation / input-mapping — never sim.** It configures *where a control is and what
  raw touch maps to which intent*; it plugs into the host-tested touch seam (`engine`
  `touch_controls`) and the screen-space HUD pass (`render::touch_controls`), and lives in the
  native **Settings** shell ([D32](decisions.md)). It is stored in local/profile config, not
  sim state.
- **Hard constraint — invariant #6:** the editor configures **placement, never information.**
  It may not add, reveal, or relocate any element that surfaces strategic intel while embodied
  (no minimap onto the FPS view, no enemy readout). It can reposition the directional alert
  *indicator*, not turn it into a map. Accessibility cues for the alert channel are a **separate**
  (non-optional) settings surface, not this cosmetic editor.

**Why:** a movable HUD is table-stakes for a serious mobile shooter and costs nothing in
fairness *as long as* it stays placement-only — which is why the invariant-#6 guardrail is
written into the decision itself rather than left implicit. Per-layer presets matter because a
thumb-reach tuned for driving a tank is wrong for marquee-selecting a squad. Presentation/input
only means invariant #2 holds — no game logic forks, the seam is the existing one.

**Consequences:** the editor is a Settings-shell surface ([D32](decisions.md)) over the existing
touch seams — no sim or netcode change. Build slice: [`pve-campaign-plan.md`](plans/pve-campaign-plan.md)
WS-D. Full design: [`customization.md`](customization.md).

---

## D62 — Selection-contextual command panel; no minimap

**Status:** decided. Realizes the Phase-4 "command HUD" polish item; replaces the always-on
build/train/upgrade text panels with one selection-driven panel.

**Decision:**

- **The command panel is contextual on the current selection** (CoH-style), a single boxed panel
  in the **top-right**: select a **camp** → its resources, train options, upgrade, and production
  queue; select **troops** → their composition, average health, and stance; select **nothing** →
  the build palette + banked resources. It supersedes the old always-on panels (build palette
  always shown, train/upgrade shown whenever a camp existed), which are deleted. The numbers come
  from the same `economy` / `train_options` / `upgrade_view` seams the old panels used, so they
  still match the sim.
- **The actions stay the existing key/seam bindings** (`B` build, `R`/`H` train, `U` upgrade); the
  panel is the *readout* of what is contextually available — it changes nothing about the
  `Command` vocabulary or the sim. Pure presentation: the host derives the panel from a read-only
  pass over the (checksummed) sim + selection (`engine::command_panel_view`), folds nothing, and
  draws it command-view-only (never over the dark embodied frame, invariant #6).
- **No minimap** — anywhere. The "going dark" dread pillar is *no map reveal*; Q1 (reaffirmed
  [D31](decisions.md)) leans alerts-only, and [D61](decisions.md) already forbids a minimap while
  embodied. A *command-view* minimap would not break invariant #6, but it cuts against the
  intended feel, so it is ruled out by design rather than left as a tempting default. Spatial
  awareness in the command view comes from the world itself (pan/zoom over the real battlefield),
  not a corner map.
- **Troops have no in-match upgrades yet** — that system does not exist in the sim (the gunsmith
  customization, [D60](decisions.md), is *pre-match loadout*). The troops panel honestly shows
  composition + stance (the order/stance vocabulary *is* the unit "options" — invariant #3); real
  per-unit upgrade rows are a `core` follow-up if/when that system lands.

**Why:** a selection-contextual panel is table-stakes RTS UX and is pure placement/derivation, so
it sits inside the locked decisions ([D43](decisions.md) selection grammar, [D61](decisions.md)
movable HUD) without touching an invariant. Keeping the *minimap* out is the load-bearing call: it
is the one element here that would erode the going-dark tension, so the decision records *why* it
is absent rather than letting a future "just add a minimap" slip in. Reusing the existing economy /
train / upgrade data seams keeps the panel byte-consistent with the sim and let the old
layout-only code (`render_command_panels`, `*_labels`) be deleted as dead.

**Consequences:** render gains `render::command_panel` (a boxed top-right panel drawn through the
shared `overlay` quad pipeline + the text pass) and the engine gains the pure `command_panel_view`
derivation; the per-corner `render_command_panels` / `CommandPanels` / `ActiveCamp` API and the
orphaned `train_panel_labels` / `upgrade_labels` layout fns are removed. The contextual panel
becomes one of the controls the [D61](decisions.md) HUD-layout editor can later reposition.

---

## D63 — Debug scenes: one shared `core::scenario` seeder, driven both headlessly and live

**Status:** decided. A "debug version" methodology for exercising one mechanic in isolation —
load two tanks into a tiny world, fire, and validate the hitboxes work. First scene: the tank
hitbox duel.

**Decision:**

- **A debug scene is seeded from a single source in `core`** — `core::scenario` (first entry
  `seed_duel`: two armoured, ballistic-gun `Heavy` chassis facing off on the X axis). The seeder is
  pure fixed-point (invariant #1/#2), so the world is bit-identical everywhere it is built.
- **It is consumed two ways from that one seed.** (a) Headless — the `sim-runner duel` scenario
  embodies the player, fires on cadence, flips the enemy hull to expose its flank, and drives the
  **real** ballistic pipeline (`fire_ballistic` → `projectile_system` → `apply_impact`), printing a
  per-event report to stderr and the determinism-covered `<tick> <checksum>` stream to stdout. (b)
  Playable — `engine::Game::new_scene(.., Scene::Duel)`, launched by `app --scene duel`, boots
  **embodied** in the player tank; a **command-view** `render::debug` overlay (F3) draws each unit's
  shell hit-radius ring coloured by armour facet (red front / yellow side / green rear), a
  hull-heading spoke, and shell tracers.
- **The duel re-dresses the existing `Heavy` chassis locally** (tank-like `Armor` + a
  `muzzle_vel`/`penetration` gun) rather than introducing a `UnitKind::Tank`; it touches neither
  `economy::unit_stats` nor the shipping balance.

**Why:**

- The [D55](decisions.md) all-unit armour-facet + ballistic-shell machinery shipped, but **no
  produced unit carried it** — so it had no focused validation surface, and the cross-arch
  determinism matrix never exercised it (`phase2`/`stress` are rifle squads: `muzzle_vel == 0`,
  unarmoured). The duel closes that gap with a golden-checksum `core` test that runs the ballistic
  path under `cargo test -p gonedark-core` (invariant #7).
- **Single-sourcing the seed makes the scene you *drive* bit-identical to the scene CI *checks*** —
  a screenshot corresponds to an assertion. It also separates two independent verification axes:
  "can I *see* which facet got hit?" (the overlay) from "does the *checksum* agree?" (the harness).
- The pattern is **expandable**: a new debug scene is one `core::scenario` entry, picked up by the
  runners and `Scene`/`--scene` with no structural change.

**Consequences:** new `core::scenario` module + a `sim-runner duel` mode; `engine` gains
`Game::new_scene(.., Scene)` (the old `Game::new` becomes `new_scene(.., Scene::Default)` — the demo
skirmish is byte-unchanged) and a presentation-only `debug_hitboxes` toggle; `render` gains the
`debug` line pass (a GPU-free `hitbox_lines`/`tracer_lines` geometry seam + a `DebugRenderer`
reusing the unit pass's camera bind group); `app` gains the `--scene <name>` flag + the **F3**
overlay key. The overlay is **command-view only** and folds nothing into the sim, so it cannot move
the checksum or reveal intel while embodied (invariants #4/#6). A real `UnitKind::Tank` (with its own
`economy::unit_stats` armour/gun) remains a later step; the duel proves the *systems* first.

**Update (second scene):** `seed_infantry` landed as the second instance of this pattern — a
hitscan sandbox (a player rifleman vs HoldFire dummies proving range / aim-cone / Light-cover /
line-of-sight / crouch, plus a `sim-runner infantry` auto-combat battery for stance / suppression /
retreat / reload), with `Scene::Infantry` + `app --scene infantry`. It needed **no structural
change** (exactly as predicted above): one `core::scenario` entry, picked up by the runner and the
scene dispatch. `render::debug` was generalized — `render_debug` now takes a flat `DebugVertex` list
composed by the host-tested `engine::debug_overlay_lines` (tanks → hitbox rings; infantry →
range-ring + firing-cone wedge; all → Player→Enemy LoS connectors (green clear / red blocked) +
muzzle-flash marker when firing) — so the overlay reads each scene's mechanic, not just the
tank's.

## D64 — The playable skirmish + a scenario-local income-pace lever

**Status:** decided. The first *real* (non-debug) match, and the economy knob that gives it its
"slow by default, faster when you hold ground" feel — without reopening the measured [D30](decisions.md)
balance.

**Decision:**

- **`core::scenario::seed_skirmish` is the first playable match**, alongside the [D63](decisions.md)
  debug seeders (`seed_duel`/`seed_infantry`) and single-sourced the same way: two operational base
  camps at `(∓30, 0)`, **exactly one starting Rifleman troop each**, and **three neutral capture
  posts** (centre + two flanks). It is pure fixed-point (invariant #1/#2), so the played match is
  bit-identical to anything a harness drives. The Enemy carries no scripted opening order — the
  existing `commander` ([D39](decisions.md)) plays it; match-end is the existing host-side
  `evaluate_outcome` (elimination, then a 15-min territory/resource timeout, [D34](decisions.md)).
  It is wired as `Scene::Skirmish` (`app --scene skirmish`/`match`) and is the **desktop default
  boot** (no flag), so launching the game drops you into it; `--scene default` keeps the old demo.
- **Economy pace is two scenario-local levers, neither touching the D30 constants:** (a) a small
  starting purse (`SKIRMISH_START_PURSE = 100`, one Rifleman) so no turn-one flood; and (b) a new
  per-`Sim` **income accrual period** (`Sim::set_income_period`). Income in `economy_system` accrues
  the *unchanged* per-accrual amount only on `tick % income_period == 0`, so the period stretches the
  *cadence*, not the amount. The skirmish uses `SKIRMISH_INCOME_PERIOD = 18`: base income ≈ 1
  Rifleman / 30 s, and since a held post still adds `PER_POINT_INCOME` per accrual, one post ⇒
  ~10 s/Rifleman and all three ⇒ ~4 s. "Take a post to earn gold faster", made literal.
- **`income_period` follows the `map_id` pattern:** it is static per-match config, so it is
  serialized in the snapshot **wrapper** (SNAPSHOT_VERSION 6 → 7) but **NOT folded into the per-tick
  checksum**. Its *effect* (the resource purse) is folded, so two peers on different periods diverge
  in resources and the desync is caught on the next tick (invariant #7). Default `1` = accrue every
  tick = the unchanged full rate, so every pre-existing scene's checksum stream is byte-identical
  (the determinism goldens are untouched).

**Why:**

- The machinery for a real match already existed and was generic over the seeded world (economy,
  capture, literal-executor units, the scripted commander, the win-condition evaluator); what was
  missing was a *scene* shaped like the game we describe (the old `Scene::Default` demo opens with
  3-unit squads already in contact, no clean opening). One `core::scenario` entry closes that with
  no structural change — exactly the expandability [D63](decisions.md) predicted.
- The measured D30 economy (~1 Rifleman / 1.7 s at base) is far faster than the intended skirmish
  feel. Re-tuning D30 globally would discard a harness-measured baseline; instead the income period
  is a *scenario-local* dial, so the skirmish can be slow-and-deliberate while D30 stays the locked
  reference every balance metric was measured against. Pacing by *cadence* (not by shrinking the
  amount) keeps territory's relative value intact — a post always ~triples income — and avoids the
  integer-truncation trap of dividing a `BASE_INCOME` of 1 (which would floor to 0).
- Keeping the period out of the checksum (the `map_id` precedent) means the lever costs the
  determinism CI surface nothing: no golden re-bless, no widened fold, while a mismatched period is
  still caught immediately through its resource effect.

**Consequences:** `core::scenario` gains `seed_skirmish` + the `Skirmish` handle and the skirmish
constants; `Sim` gains the `income_period` field + `set_income_period`/`income_period` accessors;
`economy_system` takes `(tick, income_period)` and gates income on the period; `serialize`/
`deserialize` carry the field in the wrapper (version 7). `engine` gains `Scene::Skirmish` +
`seed_skirmish_scene`; `app` boots the skirmish by default. A scenario-local match driver test pins
that the loop is live (the commander captures a post, funds production, and reinforces in 30 s), and
the income-period gate + a non-default-period snapshot round-trip are unit-tested. Barracks, a real
`UnitKind::Tank`, and a Medic/healing system remain follow-ups; this lands the *match*, not new
content.

## D65 — First content beyond the rifle squad: Tank, Medic (+ a heal system), Barracks

**Status:** decided. The skirmish ([D64](decisions.md)) shipped with only Rifleman/Heavy from a
Camp. This adds the first new producible content and the production structure to gate it — and the
first genuinely new *system* since the economy: healing.

**Decision:**

- **`UnitKind::Tank`** — a produced armoured vehicle: high HP (300), a hard, slow gun, and an
  independently-slewing turret (cosmetic, reusing the D55 hull/turret split + the existing tank
  mesh). It is **unarmoured** (`penetration == 0`, no `Armor`) and **hitscan** (`muzzle_vel == 0`).
  The full armoured + ballistic tank — which a penetration-0 Rifleman cannot crack frontally — stays
  the **duel scene's** domain ([D63](decisions.md)) until an anti-tank counter exists: fielding it as
  a produced unit in the rifle-centric skirmish, with no AT answer, would make one tank unkillable
  and the match a stalemate. Hitscan also means auto-combat resolves it exactly like every other
  produced unit (no new combat path).
- **`UnitKind::Medic` + a new `core::heal` system** — a support unit with **no offensive weapon**
  (range/damage 0, so `combat` never engages it) that, each tick, heals friendly **units** within
  `HEAL_RADIUS` (6) by `HEAL_PER_TICK` (1/8 HP/tick = 7.5 HP/s), capped at max. `heal_system` runs in
  `Sim::step` *after* combat/projectiles settle damage and despawn the dead (so a Medic never heals a
  corpse) and before territory/economy. It is fixed-point, index-ordered, RNG-free, and writes only
  `health` (which is checksum-folded), so it is deterministic (invariant #1/#7); a Medic-free world
  is a no-op, so every existing scene's checksum is byte-unchanged.
- **`BuildingKind::Barracks` + a production-routing rule (`economy::can_produce`)** — a cheaper
  (150), faster (10 s), lower-HP (600) forward building. The **Camp** (base) fields infantry +
  vehicles (Rifleman / Heavy / Tank); the **Barracks** is infantry-only and the **sole source of the
  Medic**. `queue_production` enforces the routing (a mismatched request is rejected without
  spending), and `economy_system` now serves *any* operational building's queue, not just a Camp's.
  Per-kind building HP / build-time come from `building_hp` / `build_ticks`. The slot vocabulary
  lives in the engine seams (`build_ui` slot 1 = Barracks; `train_ui` slots 2/3 = Tank/Medic);
  the desktop keybinds in `pal-desktop` still reach only Camp (`B`) / Rifleman (`R`) / Heavy (`H`),
  so Barracks/Tank/Medic are not yet selectable on the desktop host — a keybind gap, not a sim gap.

**Why:**

- The Medic is the smallest addition that introduces a *new mechanic* (heal-over-time) rather than
  another stat block — the thing the skirmish lacked. Keeping it as its own `core::heal` system (not
  bolted into `combat`) keeps the tick pipeline legible and the mechanic independently testable.
- The Tank is high-value but mostly *reuses* existing machinery (armour/ballistic/turret from D55,
  the tank mesh); shipping it **unarmoured** is the honest balance call for a rifle-only roster with
  no AT counter — the armoured/ballistic version is already demonstrated in the duel, so nothing is
  lost, and the upgrade path (armour + an AT answer) is a clean future step.
- Routing (`can_produce`) gives the buildings *distinct purpose* (the user's "build a barracks to
  make medics") without a heavy tech-tree: one table, enforced at the single `queue_production`
  choke point. New `UnitKind`/`BuildingKind` tags were added identically across the **three** codecs
  that encode them — the checksum/persist fold (`sim.rs`) **and** the lockstep wire codec
  (`lockstep.rs`) — so a `Build`/`QueueProduction` command decodes to the same kind on every peer
  (invariant #7).

**Consequences:** `components` gains the variants + `Health::heal`; `economy` gains the Tank/Medic
stats + cost/time tables, `build_ticks`/`building_hp`/`can_produce`, and per-kind build/produce;
`core::heal` is a new module wired into `Sim::step`; the unit/building tags grew in `sim.rs` (persist)
and `lockstep.rs` (wire). `render` maps Tank→tank mesh, Medic→trooper; `engine`/UI gain the train
slots (Tank/Medic), build slot (Barracks), display names, and composition readout. Determinism-
audited clean (heal float-free + index-ordered; tags agree across codecs; Medic-free scenes
byte-unchanged). **Deliberately deferred:** an armoured/ballistic produced tank + an anti-tank
infantry counter; a dedicated vehicle Factory (Tank currently comes from the Camp); teaching the
enemy `commander` to build Barracks and field Tanks/Medics (today it still masses Rifleman/Heavy from
its Camp, so the new content is player-only until the commander is extended); and a `SimEvent`/audio
cue for healing. Stats are a **playtest baseline**, not `--metrics`-measured (D30 covers only
Rifleman/Heavy).

---

## D66 — Modern lethality: ×5 weapon damage (a hit kills, not chips)

**Decision:** Scale per-shot `damage` **×5** across every produced weapon in `economy::unit_stats`
(Rifleman 6→30, Heavy 18→90, Tank 24→120; **HP, cooldown, and range unchanged**). A symmetric open
rifle 1v1 now resolves in **~1.5 s / 4 hits** (measured 91 ticks at 60 Hz), down from the D30 ~8 s
attrition. This supersedes D30's *time-to-kill* targets; D30's cost/economy numbers stand.

**Why:** the D30 baseline made a soldier a ~17-round bullet sponge — the player's words, *"why do
infantry take so long to die."* That attrition feel is wrong for the **modern-army fantasy** the player set as the
north star (a real rifle round is decisive) — the same framing later given a destination in
[D68](decisions.md). Scaling **every** weapon
by the same factor was deliberate: it preserves the whole D30 DPS-*ratio* lattice exactly (the
range-trade relationships are unchanged on paper) while compressing the clock 5×. Scaling damage (not
cutting HP) keeps `Health`/`heal`/retreat-threshold semantics and the HP numbers the UI shows intact.

**Consequences:** two *emergent* balance properties the `--metrics` suite guarded shift at lethal
speed, because combat resolves in 1–2 near-simultaneous volleys where body-count + cadence quantize
the outcome:
- the equal-cost Rifleman-vs-Heavy **rock-paper-scissors collapses** — rifle mass now wins at *every*
  range (heavies wiped 0-for), not just at range;
- per-*hit* **suppression no longer pins before the kill** — the target dies first, so the
  fire-and-maneuver lever is vestigial.

Both are genuine regressions of *inter-unit* balance, not of the lethality goal, and both need a
**re-tune at lethal speed** (a measurement loop) — tracked as [Q18](open-questions.md). The metrics
tests were **re-pinned to lock the measured reality** (honest names: `rifle_ttk_in_lethal_band`,
`equal_cost_outcomes_locked_at_lethal_baseline`, `suppression_no_longer_pins_before_kill_at_lethal_speed`)
rather than assert the now-false properties — so the numbers can't drift *silently* before the
re-tune. One golden checksum (embodied infantry scene) regenerated. Stats remain a playtest baseline.
*(Superseded in part by [D69](decisions.md): the WS-A stat re-tune subsequently raised Heavy HP
280→300 and damage 90→100 — restoring the Rifleman/Heavy RPS at lethal speed. "HP …
unchanged" was true for D66 itself; the Heavy's HP moved in the follow-on re-tune.)*

---

## D67 — All-unit ammo + resupply: finite carried rounds, rearm at base (logistics)

**Decision:** Ammo is **all-unit logistics**, not an embodied-only toggle. Every magazine weapon
(`mag_size > 0`) now rations rounds in **auto-combat** as well as embodied fire:

- `combat::combat_system`'s engage pass gains the same ammo gate `resolve_fire` already had (no fire
  while reloading or empty) and **spends a round per shot**; upkeep **auto-starts a reload** for an
  AI unit whose magazine runs dry while `reserve` remains (the embodied player still reloads manually
  via `Command::Reload` — invariant #3: auto-reload loads the held gun, it never picks targets).
- A reload now **draws from carried `reserve`** (new `Weapon` field) up to `mag_size` instead of
  refilling from nothing; an empty reserve = **combat-ineffective** until rearmed.
- New `core::resupply` system: a unit within `RESUPPLY_RANGE` (8) of a friendly **finished** Camp or
  Barracks tops `reserve` back up by `RESUPPLY_PER_TICK` (2/tick) toward `reserve_max` (new `Weapon`
  field). Wired into `Sim::step` after `heal`, before `territory`.

Loadouts: Rifleman 30 mag + 180 reserve (~210-round real loadout), Heavy 50 + 200, **Tank now finite**
(6 + 24 shells, was infinite), Medic unarmed.

**Why:** the player's second ask — *"there shouldn't be unlimited ammo… realistic to how modern armies
operate."* The old model was backwards: AI-commanded units fired **forever**; only the embodied player
ever ran dry. Making ammo bind everyone, with a reserve that depletes and a base you must pull back to,
turns logistics into a real pressure (you can't park an army on the front indefinitely) without a
heavy supply-chain system — resupply is one boolean "near a friendly depot?" check.

**Consequences:** `Weapon` gains `reserve` + `reserve_max` (u16), folded into the checksum **and**
mirrored in `deserialize` (same field order — verified) — so the Weapon fold grew two u32/slot and
**three golden checksums** were re-pinned (ballistic_pipeline, duel, infantry scene; the streams
shifted by design, not desync). `core::resupply` is a new module; a building-free or ammo-free scene
is a no-op, so those goldens are byte-unchanged. Determinism-audited clean (float-free, index-ordered,
all u16 arithmetic proven over/underflow-safe, reserve folded so a divergence is caught — invariant
#1/#7). Covered by four combat ammo tests + seven resupply tests; loadouts locked in the economy
tests. **Deliberately deferred:** dropped-ammo pickup / ammo crates in the field; teaching the enemy
`commander` and the Medic-less forward economy about resupply logistics; an out-of-ammo HUD/audio cue;
ammo as a *cost* (resupply is currently free at a depot). Loadout numbers are a playtest baseline.

---

## D68 — Factions are modelled on real modern armies (USA vs France first); design now, build later

**Decision:** The game's two sides will be **asymmetric factions modelled on real modern armies**, the
first matchup **US Army vs French Army** — replacing the generic `Player`/`Enemy` `UnitKind` roster
with **per-faction rosters** (each army's own infantry / vehicles / support, distinct silhouettes and
stats) under a strict **fairness bound** (asymmetry of *flavour and feel*, never of *power* — pillar 4:
the cost must always feel fair; cross-play parity, [Q17](open-questions.md)). **This decision records
the direction and is design-only for now:** the design lives in [`factions.md`](factions.md); the
build is **not** started. Lethality ([D66](decisions.md)) and all-unit ammo ([D67](decisions.md))
land first on the existing shared roster.

**Why:** the player set the north star — *"the goal is to have a USA army vs the French army."* It is a
large structural change (today there is only `UnitKind`, no faction identity beyond the `Faction`
enum's allegiance tag), so committing the **direction** now — and capturing the hard questions before
writing code — is worth more than a half-built roster. It also gives D66/D67's "modern-army" framing a
concrete destination.

**Consequences:** a new design doc [`factions.md`](factions.md) (the roster/asymmetry/fairness model);
the unresolved specifics — exact rosters, *how* asymmetric, how it interacts with the gunsmith
([D60](decisions.md)) and the PvE campaign ([D58](decisions.md)) — are tracked as
[Q19](open-questions.md). No engine code changes in this decision. The `Faction` enum stays an
allegiance tag (`Player`/`Enemy`/`Neutral`); a *faction identity* (US/FR) is a separate, future
component layered over `UnitKind` rosters, not a rename of `Faction`.

**Plan:** the build sequencing is [`factions-plan.md`](plans/factions-plan.md) (gated on the
shared-archetype rebalance, [`combat-rebalance-plan.md`](plans/combat-rebalance-plan.md), landing
first).

---

## D69 — Combat rebalance WS-A: restore the Rifleman↔Heavy RPS at lethal speed (Heavy 280/90 → 300/100)

**Decision:** Re-tune the **Heavy** unit (and only the Heavy) from the [D66](decisions.md) lethal
baseline of **280 HP / 90 damage** to **300 HP / 100 damage**, keeping its range (11), cooldown (48),
and all ammo/loadout stats unchanged. This restores the intended range-dependent rock-paper-scissors:
the cost-equal Heavy mass **wins at point-blank**, the longer-ranged Rifleman mass **kites and wins at
range**. Measured against the [D30](decisions.md) `sim-runner --metrics` harness: equal-cost **sep5 →
Heavy survives +2, Rifle 0-for**; **sep9 → Rifle survives +2, Heavy 0-for**. This is workstream **WS-A**
of [`combat-rebalance-plan.md`](plans/combat-rebalance-plan.md); it partially resolves
[Q18](open-questions.md) (WS-B, area suppression, is the remaining half).

**Why:** [D66](decisions.md)'s ×5 lethality scaled every weapon uniformly, which preserved the DPS
*ratios* on paper but at 1–2-volley kill speed **flattened** the RPS — the Rifleman mass's body-count +
faster cadence won at *every* range (Heavies wiped 0-for), erasing the [D26](decisions.md)/[D30](decisions.md)
"Heavies win close, rifles kite at range" matchup. D66 honestly **re-pinned** the metrics test to lock
that regression rather than assert the now-false property. This un-breaks it. Suppression+maneuver is
the core of the modern-infantry fantasy the [D68](decisions.md) US-vs-France direction leans into, so
the matchup matters. The fix is the **smallest** Heavy buff that crosses over cleanly (HP +20, damage
+10): bigger buffs made the Heavy win at range too; smaller ones failed to win close.

**Consequences:** two integer `Fixed::from_int` constants in `economy::unit_stats` changed —
fixed-point, so the cross-arch `determinism.yml` matrix is bit-identical by construction, and **no
checksum goldens moved** (the ballistic/duel/infantry goldens use scene-local stats or are Rifleman-only;
confirmed). The re-pinned metrics test `equal_cost_outcomes_locked_at_lethal_baseline` was **renamed**
to `heavy_wins_close_rifle_wins_at_range` and now asserts the *intended* direction (who wins, nobody
0-for) plus exact survivor/tick pins. `rifle_ttk_in_lethal_band` stays green (Rifleman untouched, 91
ticks). The economy baseline + a `heal` test fixture updated to 300 HP. **Measured, not felt** — every
number dialed against `--metrics` (pillar 4: a *relative* re-tune, no power creep). **Remaining:** WS-B
makes suppression bite before the kill at lethal speed (separate commit, `/safe-edit`); landing it
closes [Q18](open-questions.md).

---

## D70 — Combat rebalance WS-B: area suppression + lower pin (suppression bites before the kill)

**Decision:** Add **area (fire-and-maneuver) suppression** and lower the pin threshold so concentrated
fire pins a cluster *before* it is wiped, at the [D66](decisions.md) lethal kill speed:

- A shot now suppresses the **area** around its impact, not just the body it hits. Every hostile
  **unit** within `SUPPRESSION_RADIUS` (4 world units) of the target accrues `SUPPRESSION_SPLASH_PER_HIT`
  (1/16 — strictly less than the 1/8 a direct hit applies), on top of the full per-hit on the target
  itself. Applied on **both** fire paths — the auto-resolver's engage pass (reusing the per-tick
  `SpatialHash` via the new `SpatialHash::for_each_within`) and the embodied `resolve_fire` (an
  index-ordered scan). Friendlies and buildings are excluded (invariant #3: only enemy soldiers pin).
- `SUPPRESSION_PIN` lowered **1/2 → 3/8**. Area splash alone cannot reach the pin line at lethal speed
  (four splash increments of <1/8 sum below 1/2, and decay erases a volley between cooldowns), so the
  threshold had to drop with it. At 3/8, a 4-shooter cluster volley pins while a lone shooter — one
  decaying hit per cooldown — still never does.

This is workstream **WS-B** of [`combat-rebalance-plan.md`](plans/combat-rebalance-plan.md); with WS-A
([D69](decisions.md)) it **closes [Q18](open-questions.md)**.

**Why:** suppression + maneuver *is* modern infantry doctrine — the fantasy the [D68](decisions.md)
US-vs-France direction leans into — and [D66](decisions.md)'s ×5 lethality had made it vestigial: a
target died before per-*hit* suppression could pin it, so "concentrate fire to pin" did nothing (the
metric honestly locked pin-at-0). Suppressing the *area* (rounds cracking past a position pin the
soldiers near them, even those not hit) is both the doctrinally-correct model and what lets a pin land
before the kill. The user chose this fork (area splash + a lower global pin) over slowing decay or a
splash-only partial fix.

**Why these numbers:** measured against `sim-runner --metrics`, not felt. Splash was first tried at 3/32
but that let a numerous rifle blob pin-and-wipe a cost-equal Heavy force (area suppression amplifies
numerical superiority), flattening the [D69](decisions.md) RPS at scale (equal-cost 1000 close flipped
to a rifle 10-0 blowout). Dropping splash to **1/16** keeps the 4-shooter pin-before-kill (now via the
3-direct-hit unit) while keeping the equal-cost trades *real fights*: the canonical RPS holds — heavy
wins close at 500, rifle kites at range — and a larger rifle mass trading up close stays a ~3 s fight,
not a blowout. **Known interaction (honest caveat):** area suppression still tilts the equal-cost trade
toward the more numerous side, so "Heavy wins close" is now budget-dependent — it holds at the smaller
(500) budget but a 10-rifle mass out-suppresses 4 Heavies close at the 1000 budget. Acceptable (massed
infantry volume-of-fire pinning a few gunners reads true) and flagged for the per-faction tuning pass.

**Consequences:** `core::combat` gains `SUPPRESSION_RADIUS`, `SUPPRESSION_SPLASH_PER_HIT`, and a
`splash_suppress` helper; `core::spatial` gains `for_each_within` (an area companion to `nearest_within`,
with its own superset/order-independence contract + tests). All fixed-point, float-free (determinism
guard green), index-ordered, no RNG — the per-slot saturating add is order-independent, so the
cross-arch + 2-peer lockstep runners stay the safety net (invariants #1/#7). Suppression is
checksum-folded, so behavior moved by design; **no goldens needed re-pinning** — the pinned scenes
assert their *final* checksum, where the area-suppressed dummies (HoldFire, order-less) are already dead.
The metrics tests were re-pinned to the intended properties:
`suppression_no_longer_pins_before_kill_at_lethal_speed` → `focus_fire_pins_before_kill_but_lone_shooter_never_pins`
(the focus-fire scenario became a tight **cluster**, since a lone target has no neighbours for area
suppression to act on), and the [D69](decisions.md) `heavy_wins_close_rifle_wins_at_range` exact pins
were updated for the suppression-moved numbers (directions unchanged). Five new `core::combat`
suppression tests + two `core::spatial` tests. **Deliberately deferred:** suppression as a function of
*incoming volume* over a window (vs per-shot); cover/terrain modulating splash; per-faction suppression
feel; tuning splash/pin again once the per-faction `unit_stats` land.

## D71 — Factions WS-B: identity tilts on logistics rhythm, not gun stats (soft asymmetry, swap-invariant)

**Decision:** Per-faction rosters (US Army vs French Army) are **soft-asymmetric** — every army fields the
same shared archetype skeleton (rifleman / heavy / vehicle / support) — and the only axis that differs
between armies is the **logistics rhythm** (magazine size / reload time / reserve depth). The
**combat-power axes — damage, cadence, range, HP, penetration — are held strictly shared** across US and
FR. The logistics tilt is scaled to keep **sustained DPS and reserve depth invariant** between the two:

- **US** = deep-magazine / long-reload **sustained-fire** doctrine (M249/M240, M1 Abrams).
- **FR** = shallow-magazine / snappy-reload **quick-swap** doctrine (FAMAS, Leclerc).
- **Tank** identity is **turret-slew only** (cosmetic, [invariant #3](../CLAUDE.md)) — its shallow 6-shell
  magazine makes *any* logistics tilt unfair under reload pressure, so it carries no stat tilt at all.
- **Medic / support** is shared (no fair combat surface to tilt). **Neutral** army = **byte-identical** to
  the pre-factions shared baseline.

This realizes workstream **WS-B** of [`factions-plan.md`](plans/factions-plan.md) and **answers the
WS-B stat-budget design gate** that was the open fork in [Q19](open-questions.md) — locking the **soft
asymmetry** lean and pinning *where* the asymmetry lives. Implemented in `core::economy`
(`unit_stats_for(Army, kind)`), folded into the per-tick checksum at production (wave-2 W6, `ff4a53a`).

**Why:** the equal-cost mass infantry trade is a **Lanchester square-law snowball** — a small per-unit
edge compounds with surviving body-count, so *any* tilt to the core combat axes flips the equal-cost
matchup outside any fairness band. Measured, not felt: a 2-point Rifleman damage gap run 10-vs-0 produced
non-monotonic outcome flips. A "soft gun tilt" therefore cannot stay fair. Logistics rhythm is the one
axis that delivers a distinct *feel* (how a firefight breathes — burst-and-swap vs. lean-on-the-trigger)
**without** changing the equal-cost kill math, because it is tuned to hold sustained DPS and reserve depth
equal. The tank exception falls out of the same measurement: a shell-count tilt handed FR a tank-in-cover
standoff 2-0 once shells deplete and reloads gate fire, so the tank keeps a purely cosmetic (turret-slew)
identity. This keeps [invariant #1](../CLAUDE.md) (fixed-point), [invariant #3](../CLAUDE.md)
(literal-executor — no autonomous unit smarts), and the [D30](decisions.md) cost-parity discipline, and it
serves the [D68](decisions.md) US-vs-France direction (real-doctrine feel) without re-opening the
[D69](decisions.md)/[D70](decisions.md) balance the rebalance just settled.

**Why these numbers:** dialed against `sim-runner --metrics`, the same objective signal as
[D30](decisions.md)/[D69](decisions.md)/[D70](decisions.md). A new `cross_faction_equal_cost` check
asserts **swap-invariance**: the mirror-of-roles equal-cost trade is **bit-identical across US/FR, FR/US,
and Neutral/Neutral** for every archetype and separation (zero army-power delta; the Player-side win is a
fixed index-order artifact, identical in all three orderings), holding even under a reload-pressure tank
standoff exercised long enough to deplete and reload shells.

**Consequences:** `core::economy` gains `unit_stats_for(Army, kind)` (the production roster seam every peer
spawns the bit-identical unit from); production now draws the producing faction's army roster, folded into
the checksum so a mismatched-army peer desyncs at production (caught by the cross-arch + 2-peer lockstep
runners, [invariant #7](../CLAUDE.md)). All tilt tables are `u16`/`Fixed::from_int` — float-free
(determinism guard green). **Scope (honest caveat):** the change is the **production-roster seam only** —
`scenario.rs` pre-placed *starting* troops still draw the shared `unit_stats`, so no shipping scene
(`seed_skirmish`/`seed_seize`) runs US/FR production long enough to spawn a tilted unit, the
Neutral-baseline scenes are byte-unchanged, and **no goldens moved**. Army-tilting the pre-placed starting
troops is a clean WS-C/WS-D follow-up (it will move those scenes' checksums by design). Tests: +7
`core::economy`/`sim` (Neutral==baseline; US/FR differ only on logistics; infantry tilt is
DPS/depth-neutral; tank tilt is cosmetic turret-only; production spawns the producing army's roster; 2-peer
mismatched-armies lockstep agreement; per-army roster diverges the checksum at production) and +3
`sim-runner` metrics tests (swap-invariance, swap-invariance under reload pressure, distinct-but-Neutral-
matches-baseline), with `cross_faction_equal_cost` wired into the `--metrics summary` digest.

## D72 — AI-controlled ballistic fire: a produced tank's gun travels regardless of driver (Q20 → option ii)

**Decision.** When a produced `UnitKind::Tank` carries `muzzle_vel > 0` (the armoured ballistic tank
of [`tank-embodiment-plan.md`](plans/tank-embodiment-plan.md) P9), its shot is a real traveling
`Projectile` **whether the unit is AI-driven or embodied**. The AI auto-resolver
`combat::combat_system` is taught to spawn a `Projectile` (via the existing
`projectile::fire_ballistic` path) for `muzzle_vel > 0` instead of resolving instant hitscan; hitscan
remains the path only for `muzzle_vel == 0` weapons (rifles, the unarmoured D65 tank). This closes
[Q20](open-questions.md) on **option (ii)**.

**Why.** The same barrel firing a laser when AI-driven and a cannon when embodied is a visible physical
inconsistency the moment two AI tanks duel at range — exactly where ballistics matter. Option (ii) makes
the gun identical regardless of who pulls the trigger, and it is *emergent*, not *clever*: the AI does not
lead or solve a firing solution (that would touch [invariant #3](../CLAUDE.md)) — it fires along its
current aim and the shell travels, so a moving target can now out-run or be missed by an AI shot, and the
shell can be seen and reacted to in flight. The literal-executor stays literal; only the projectile becomes
physical. This keeps the gun's *feel* swap-invariant with [D71](decisions.md)'s identity model.

**Determinism.** This adds new sim writes (projectile spawns) inside the AI resolver, so it is
[invariant #7](../CLAUDE.md)-sensitive: the spawn is index-ordered and fixed-point (the same
`fire_ballistic` the embodied path already uses, [invariant #1](../CLAUDE.md)), folded into the per-tick
checksum, and **must keep `determinism.yml`'s arch matrix and the 2-peer lockstep runner green** — a desync
here is a real bug, never to be silenced. Armour facing is unchanged: `facing_penetration_multiplier`
already resolves identically across all three fire paths at impact (P4, `dc8ce4e`), so this fork was only
ever about projectile *travel*, not damage.

**Consequences.** `combat::combat_system` branches on `muzzle_vel` (hitscan vs. ballistic spawn); the
bounded projectile ring (§6a) now caps AI-originated shells too and `log()`s on saturation; the P9
ballistic-gun worker builds against this contract. Ships with: a test that an AI tank with `muzzle_vel > 0`
spawns a traveling projectile (near target hit *later* than hitscan), that `muzzle_vel == 0` units still
hitscan unchanged (golden), and a 2-peer lockstep agreement over the AI-ballistic-fire path on one seed.

## D73 — Infantry anti-tank counter is a new dedicated AT infantry unit (restores the armour RPS triangle)

**Decision.** Wave-1's P9 armour block made the produced `UnitKind::Tank` immune to small-arms (a
Rifleman's `penetration == 0` shot bounces every facet — [W1], D72/economy). The counter is a **new
dedicated anti-tank infantry archetype** — a fragile, slow-firing bazooka/AT-team `UnitKind` carrying
`penetration > 0` — **not** giving an existing unit penetration (would muddy the measured Rifleman↔Heavy
RPS, [D69](decisions.md)/[D70](decisions.md)) and **not** an embodied-only AT weapon (would leave AI
infantry unable to answer an AI tank push). This restores the classic Company-of-Heroes triangle:
**AT-infantry beats armour, massed infantry beats AT-infantry, armour beats infantry.**

**Role / balance contract.** The AT unit pens the tank's **frontal** facet (so it is a real threat head-on,
not just a flank-poke) — i.e. `2 · penetration ≥ TANK_ARMOR_FRONT (40)`, so `penetration ≥ 20` — but is
**fragile** (low HP), **slow** (long cooldown / few ready rounds, D67 logistics), and **weak vs massed
infantry** (poor anti-personnel DPS), so equal-cost it loses to riflemen. Numbers are dialed against
`sim-runner --metrics` (the [D30](decisions.md) cost-parity signal), and the AT↔tank and AT↔rifle
matchups are locked with measured assertions. It is **unarmoured** (infantry — `unit_armor` default), and
per-faction stats route through `economy::unit_stats_for` like every other archetype ([D71](decisions.md),
held within the fairness band).

**Determinism / blast radius.** A new `UnitKind` variant is invariant-#7-sensitive: it crosses the lockstep
wire (a `QueueProduction` command) and the snapshot. The variant gets a **new codec tag** in
`lockstep::put/get_unit_kind` mirrored in `sim::unit_kind_tag` (append-only: existing tags 0–3 unchanged,
AT = 4), and **both versions bump** — `WIRE_VERSION 8 → 9` and `SNAPSHOT_VERSION 9 → 10` — so a
mismatched-build peer fails the handshake rather than silently desyncing. Every exhaustive `match UnitKind`
(economy, sim, combat, heal, commander, gunsmith, scenario, render `model_for_unit`/train panels, engine
train UI) gains its arm. Float-free (invariant #1); kept green across `determinism.yml`'s arch matrix and
the 2-peer lockstep runner. High blast radius → built via `/safe-edit`-style review.

**Consequences.** Players (and the scripted commander) can now train an answer to armour; the
`components.rs` `UnitKind::Tank` doc comment (which still says the armoured tank "remains the duel scene's
domain until an anti-tank counter exists") is corrected — the counter now exists. Ships with: the new arm
across all matches (compile-exhaustive), codec round-trip for tag 4, a 2-peer lockstep agreement that
produces and fights an AT unit, and the measured AT↔tank (frontal pen) + AT↔rifle (loses equal-cost)
balance assertions.

## D74 — Visual-design foundation: a central theme + an anti-aliased font atlas (replaces the 5×7 bitmap)

**Decision.** Stand up the renderer's first deliberate art-direction layer, advancing the
[`roadmap.md`](roadmap.md) "visual-design pass … so it looks intentional, not greybox" backlog item.
Two pieces: **(1)** a single `render::theme` module is now the source of truth for the palette, type
scale, and spacing — the ~15 hand-tuned colour consts scattered across `lib.rs`, the `overlay`
`QuadRole` map, and the per-panel HUD modules now reference it, and its ink/panel/text/amber ramp is
aligned to the desktop title-shell palette (`app/src/shell.rs`) so the egui chrome and the in-match
`wgpu` HUD finally share one identity. **(2)** the in-match text pass swaps its legacy **5×7 uppercase
bitmap** for a **fixed-cell monospace anti-aliased font atlas** (printable ASCII 0x20–0x7E, lowercase
+ punctuation), baked by `tools/fonts/gen_hud_font.py` from Liberation Mono Bold via ImageMagick.

**Why.** The 5×7 bitmap — uppercase-only, no punctuation, one solid quad per lit cell — was the single
biggest "this is a prototype" tell in the HUD, and the per-module colour sprawl meant nothing read as
art-directed. The user explicitly asked to push the look toward near-final, which reopens the original
"baked bitmap, no font deps" call documented in `render::text`. This keeps the *spirit* of that call —
the atlas ships as **raw R8 coverage bytes** (`assets/fonts/hud_atlas.gray`) `include_bytes!`d straight
in, so the render crate stays **`wgpu` + `bytemuck` only**: no png-decode, no font-rasterisation, no
atlas-management crate. Script-not-binary ([D41](decisions.md)/[D46](decisions.md)): the generator +
the `assets/fonts/manifest.json` provenance entry (source / OFL licence / sha256) are the committed
record; the atlas is a regenerable artifact (`pnpm assets:font`).

**Determinism.** None — this is entirely the render-side float boundary ([invariant #1/#4](../CLAUDE.md)).
No `core`/sim type changes, no new sim reads; the per-tick checksum stream is untouched. Text remains
NDC chrome carrying no world position ([invariant #6](../CLAUDE.md)).

**Consequences.** `render::text` keeps its public API (`queue`/`render`/`set_aspect`/`measure`/`Anchor`),
so every caller (radial, readout, panels, overlay, post-match) is unchanged; it now owns an R8 atlas
texture uploaded lazily on the first `render` (the construction path has no `queue`). The `FONT_*`
metrics in `render::text` are the contract with the generator and are pinned by a test against the
baked blob's length. Monospace advance is a touch tighter than the old bitmap (less panel overflow,
never more). Verified on the RTX 3070 via the offscreen `viz-runner` (all visual assertions green; the
radial/readout/command-bar labels render crisp mixed-case). Follow-on visual work (world lighting +
tonemap/fog, post-processing, HUD icons via Inkscape, ground/detail textures via ImageMagick, richer
greybox meshes via Blender) builds on this `theme` foundation.

## D75 — Desktop Settings / Profile / About surfaces land (Phase 4 surface 3, partial), audio + look prefs wired

**Decision.** Build the desktop out-of-match utility screens the [D36](#d36--the-desktop-app-shell-an-egui-boot--title-title-screen-desktop-sibling-of-d35)
title shell stubbed as no-ops, and reorganise the landing screen into a real HUD. Three screens, drawn
in the same egui shell over a live 3D backdrop: a **Settings** screen (audio master/SFX/music, look
sensitivity + invert-Y, fullscreen, a render-quality choice), a **Profile** screen (callsign, faction
preference, lifetime record), and an **About / field manual** (the one-line pitch, the *real* default
keymap grouped by layer, the build stamp). Two prefs are **wired through to the host**: **master + SFX
volume** scale the desktop audio sink (`DesktopAudio::set_gains` → the new `pal::mix::scaled_gain`), and
**look sensitivity + invert-Y** shape the desktop look input (`DesktopInput::set_look_prefs` →
`scale_look`), both pushed each match frame. The title screen itself is re-laid as an anchored HUD
(brand top-left, Settings/Profile chips top-right, the Campaign/PvE/PvP play cluster bottom-left, build
stamp opposite) over an animated `render::title_backdrop` 3D diorama (parallax to the cursor).

This advances phase-4 **surface 3 (Settings)** from BLOCKED to **PARTIAL (desktop)**, and reaches into
**surface 6 (Progression & profile)** for a local, pre-account slice.

**Why.** [D36](#d36--the-desktop-app-shell-an-egui-boot--title-title-screen-desktop-sibling-of-d35)
shipped the desktop title shell but left every utility button a placeholder; the user asked to make the
landing screen real and then to actually wire the prefs. Settings is *"buildable in design terms — it
configures shipped systems"* ([`phase-4-plan.md`](plans/phase-4-plan.md) surface 3), and audio gain +
mouse sensitivity are exactly those shipped systems (the `pal` audio mix, the `pal-desktop` input
mapper) — they need no new backend. Wiring them now proves the host-pref → PAL path end-to-end before
the heavier surface-3 pieces that genuinely gate on other work.

**Determinism.** None — entirely host-side presentation ([invariant #1/#2](../CLAUDE.md)). The volume
prefs feed only the `pal` audio sink; the sensitivity/invert prefs scale the host `look_axis` *before*
the engine boundary, and the embodied look they feed is quantized to `Fixed` at the command boundary, so
per-player sensitivity **cannot** diverge the lockstep stream (reviewed via `/check`). No `core` type
changes, no new sim reads, the per-tick checksum stream is untouched. All new decision/format/validation
logic is pure and unit-tested (`resolve_title_action`/`apply_settings_action`/`apply_profile_action`,
`scaled_gain`, `scale_look`, `sanitize_callsign`, `win_rate_pct`, the quality/faction cyclers,
`controls_reference`); the egui drawing + wgpu compositing stay the [D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)/D36
device-gated glue. The `title_backdrop` renderer is render-side float ([invariant #4](../CLAUDE.md)),
self-contained, with a unit-tested parallax/view-proj seam and a `viz-runner` screenshot scene.

**Consequences / scope boundary.** **Wired now:** master + SFX volume, look sensitivity + invert-Y
(desktop). **Dormant prefs** (stored + on-screen but not yet consumed): **music volume** (no music cue
exists to scale) and **render-quality** (not yet routed into [`render::tiers`](architecture.md)) —
`SettingsState`'s doc states exactly what is wired vs not. **Still unbuilt on surface 3:** accessibility
cues (the going-dark fairness channel, invariant #6), the touch-layout / key-rebind editor, gamepad
rebinds — these keep the surface **PARTIAL, not LANDED**. **Profile is local-only:** no account /
persistence backend (surface 6 stays BLOCKED on that, [`infrastructure.md`](infrastructure.md)); the
lifetime record is a host counter not yet written by the post-match summary, and nothing survives a
restart. Android's Compose shell ([D35](#d35--first-native-app-shell-surface-the-android-compose-boot--title-landing-screen))
is unchanged (no Settings/Profile there yet) — desktop-only landing. Ships with the three screens, the
two pref-wiring paths, the title-HUD + backdrop, and their tests (app 35, pal-desktop 47, pal mix;
dev + release + `audio` feature all green).

## D76 — Mission/scenario authoring format: external RON data files behind a host-side loader (resolves Q15)

**Decision.** Missions and battlefields become **external RON data files**, not Rust scenario
builders and not a scripting VM. A mission ships as a `*.mission.ron` file; the spatial half of a
mission (terrain id, control points, cover-prop placement, spawn zones) factors out into a
`*.map.ron` **battlefield** file a mission references, so one battlefield backs many missions (the
Operations-hub replay model, [`pve-campaign.md`](pve-campaign.md) §2). This resolves
[Q15](open-questions.md) in favour of its standing lean (RON, serde-native), with Lua/scripting
explicitly deferred to a possible second pass.

The **load-bearing architecture call** is *where the data layer lives*: **host-side, in `engine`,
never in `core`.** `core` today carries **no serde dependency** and must stay that way (invariant
#2 — `core` depends on no non-sim crates). So the format lands as two pieces:

1. **`core` grows a deterministic, serde-free `ScenarioBuilder` API** — a thin typed builder over
   the spawn/build primitives the hand-written `core::scenario` seeders already call privately
   (spawn a `UnitKind` at a cell with a `Faction`/`Stance`, `economy::build` a camp, `set_income`,
   `set_army`, place a control point). Programmatic, fixed-point, no parser. The existing
   `seed_seize_mission` becomes one caller of this builder rather than bespoke code.
2. **`engine` grows a host-side loader (`engine::mission_format`)** that owns the serde/RON
   dependency, parses a `*.mission.ron`/`*.map.ron` into a validated `MissionSpec`/`MapSpec`, and
   drives the `core` builder. This is the **exact split the objective system already uses**
   ([D59](#d59--the-operations-hub-campaign--a-host-side-objective-system)): the deterministic
   primitive lives in `core`; the host-side content layer that *selects and parameterizes* it lives
   in `engine`.

**Why.** Missions are the campaign's **content volume** — the thing we author the most of — and the
battlefield format is the gate on *extensive* maps for both PvE and PvP. A recompile-per-mission
loop (the Rust-builder option) throttles that hard and demands a Rust toolchain to write a level;
it is also the single biggest amplifier of Rust's weak engine hot-reload ([D10](#d10--engine-language-rust)
tradeoff, [`roadmap.md`](roadmap.md) dev-workflow scripting lane). RON over Lua because it is
**serde-native** — the schema is a `#[derive(Deserialize)]`, with no scripting VM, no new runtime,
and no iOS-JIT problem ([`platforms.md`](platforms.md)); it is designer-editable, hot-reloadable,
and diffs cleanly in git. A mission is *data*, not *behaviour*, so it doesn't need a language; the
moment a scripted set-piece genuinely needs control flow (a Halo beat, [Q16](open-questions.md)),
the Lua second pass is the documented escalation, layered on the same loader.

**Determinism.** The loader is the **float airlock**, and this is the property that keeps the format
safe under invariants #1/#7. Every numeric field parses as an **integer** — positions as integer
cells, distances/rates/HP as fixed-point milli-units folded straight into `Fixed` via
`Fixed::from_*` — and there is **no `f32`/`f64` path from a file into the sim**. The schema is
`deny_unknown_fields` + range-validated, so malformed or out-of-range input **fails loudly at load,
host-side**, never silently desyncs. The determinism guard greps the loader like any sim-adjacent
code. Critically, the **data file never enters the per-tick checksum** — only the seeded `Sim` state
does, identical to how a hand-written seeder works today, so the cross-arch `determinism.yml` matrix
covers a data-loaded mission exactly as it covers `seed_skirmish` with **zero new fold surface**.
The proof obligation is a round-trip test: the data-loaded *Seize* mission must build a `Sim`
**byte-identical** to the code-built `seed_seize_mission` (same opening checksum), so the format is
demonstrably a faithful re-expression, not a new code path.

**Consequences.** Mission/map authoring moves from engineer-recompile to designer-edit-a-file — the
unblock the content pillar exists for. `engine::mission_registry` gains a **data-backed path**:
`MissionDef`s load from a content directory of `*.mission.ron` instead of hardcoded
`MissionDef::new(...)` calls; the existing hardcoded `default_registry()` stays as the
test/fallback baseline and the round-trip oracle. Battlefields become first-class reusable artifacts
(`*.map.ron`), which is what "extensive battlefields" requires for PvE *and* PvP (a PvP skirmish is
the same `MapSpec` with two human commanders instead of a scripted one — the format is mode-agnostic;
PvP's remaining gap is the live net layer, Phase 3, not the content layer). Hot-reload of content
files between matches becomes the primary mitigation for Rust's weak engine reload (D10,
[`roadmap.md`](roadmap.md)). The remaining `ObjectiveKind` archetypes already modelled in
`engine::objectives` (Hold/Survive, Push, Assassinate/Escort — [D59](#d59--the-operations-hub-campaign--a-host-side-objective-system))
become **expressible in the format** rather than requiring new code per mission. The native
Operations-hub mission-select/briefing shell stays [D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)-blocked
(unchanged by this) — this decision unblocks *authoring* content, not *presenting* the campaign menu.
Build sequencing, workstreams, and the validation harness: **[`content-tooling-plan.md`](plans/content-tooling-plan.md)**.

## D77 — Content-addressed terrain: maps carry the grid, persist serializes a content-hash id (resolves Q22)

**Decision.** Lock [Q22](open-questions.md) option (iii): **content-addressed terrain.** Terrain
stops being a hard-coded registry and becomes **authorable/generatable data** — a battlefield's
cover/line-of-sight grid lives in the map content (`*.map.ron` / a terrain content artifact,
[`content-tooling-plan.md`](plans/content-tooling-plan.md) CT-C) and is identified by a
**deterministic content hash of its canonical fixed-point bytes**. `MapId` evolves from a `u16`
registry index into that content-hash digest. `persist`/reconnect ([D28](#d28--authoritative-snapshot-format-a-hand-rolled-le-serialization-sharing-the-checksum-walk))
keeps serializing **only the id, not the grid** — a resuming or joining peer rebuilds the identical
grid by looking the id up in the shared content set it loaded at match start. A missing or mismatched
id is a **hard, explicit failure at load/setup, never a silent desync.**

**Why.** Of the three [Q22](open-questions.md) forks: **(i)** a built-in-id registry makes every new
terrain a recompile shipped in the binary — it defeats the whole [D76](#d76--missionscenario-authoring-format-external-ron-data-files-behind-a-host-side-loader-resolves-q15)
data-file goal for the *one* piece that most defines a battlefield, and the procedural generator
(CT-G) could never make new ground. **(ii)** embedding the grid by value bloats every reconnect
snapshot with a full grid, regressing [D28](#d28--authoritative-snapshot-format-a-hand-rolled-le-serialization-sharing-the-checksum-walk)'s
deliberately lean snapshot (which carries terrain *by id* precisely to stay small). **(iii)
content-addressed** is the only option that keeps the snapshot lean (id only, exactly as today) **and**
makes terrain authorable — and the shared content set it relies on is **already** introduced by
[D76](#d76--missionscenario-authoring-format-external-ron-data-files-behind-a-host-side-loader-resolves-q15)'s
data-file model, so it rides existing machinery rather than inventing new. The content-hash id is also
**self-validating**: if two peers ever computed different grids, their ids differ and the mismatch is
caught at setup, not as a silent lockstep divergence.

**Determinism.** The terrain grid is fixed-point `Cover` per cell — **no floats** ([invariant #1](../CLAUDE.md)),
unchanged. The content hash is computed over the grid's **canonical integer bytes** using the same
arch-independent hashing discipline as the checksum field-walk
([D28](#d28--authoritative-snapshot-format-a-hand-rolled-le-serialization-sharing-the-checksum-walk)),
so the id is **bit-identical on every platform** ([invariant #7](../CLAUDE.md)). Terrain is loaded once
at match start through the **host-side [D76](#d76--missionscenario-authoring-format-external-ron-data-files-behind-a-host-side-loader-resolves-q15)
airlock** (so authored grids can't smuggle a float in), then is static sim-read state; it does **not**
mutate per tick, so the per-tick checksum is unaffected. Cross-peer terrain equality is established at
match setup via the content-hash id, and reconnect carries that id **exactly where the `u16` map-id sat**
([D28](#d28--authoritative-snapshot-format-a-hand-rolled-le-serialization-sharing-the-checksum-walk)) —
so the resume path's determinism property is preserved verbatim; only the id's *meaning* (registry index
→ content hash) and width change.

**Consequences.** `MapId` widens from `u16` to the content-hash digest (reuse the 64-bit checksum
hash — ample for a curated content set; a wider/crypto hash is a later concern only if untrusted
user-mod terrain ships, PC-4). `Terrain::from_map_id` is replaced by a content-set lookup
(`Terrain::from_content`/by-hash); today's built-in open field (id `0`) becomes one content entry like
any other (or a reserved sentinel for the empty field). **CT-C** authors terrain grids as data; **CT-G**
generates them and emits the grid **plus its content hash**, with the **CT-F** lint verifying the hash
matches the grid (reproducible, untampered). **Content distribution:** shipped/official maps bundle with
the build; user-generated/mod terrain ([`roadmap.md`](roadmap.md) PC-4) needs a distribution path — a
known later concern, and a peer lacking a referenced terrain id fails the match-setup handshake
**explicitly**. This **unblocks CT-G's terrain half** (Q22 no longer gates it; the placement half was
already unblocked). Net: novel battlefields — terrain included — become **scripted, lintable content**.

## D78 — Android title backdrop is Compose-native, not an embedded wgpu surface

**Decision.** The Android Compose landing screen ([D35](#d35--first-native-app-shell-surface-the-android-compose-boot--title-landing-screen))
gets its visual depth from a **Compose-native animated backdrop** (a gradient + drifting vector
motif, driven by Compose animation), **not** by embedding a `wgpu` `SurfaceView` to run the
desktop's live 3D `render::title_backdrop::TitleBackdrop`. The Android title stays pure Compose
chrome; pixel-for-pixel parity with the desktop 3D backdrop is explicitly *not* a goal. (Recorded
ahead of the work in [`compose-shell-parity.md`](plans/compose-shell-parity.md) §7.)

**Why.** The desktop backdrop is a wgpu scene composited under egui — but on Android the title is a
separate Compose `MainActivity`, with the engine (and its only wgpu surface) living in a *different*
`NativeActivity`. Bringing the real 3D backdrop to the Compose title would mean standing up a second
render surface inside the shell process — its own device, lifecycle, and threading — which is a large
cost for a static screen and partly re-litigates the [D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)
native-chrome split (the whole point of which is that out-of-match chrome is *cheap* native UI, not
engine rendering). A Compose-native animated backdrop buys ~80% of the perceived polish at a tiny
fraction of the cost and keeps the shell a pure-Compose surface. The decision is **reversible**: if a
shipped title ever genuinely needs the live 3D scene, option 2 (an embedded `SurfaceView`) remains
open — this just declines to pay for it now.

**Consequences.** Desktop and Android title screens will look deliberately different (3D scene vs.
animated 2D motif); that is accepted, not a parity bug. No engine/render change is needed on the
Android path — the backdrop is authored entirely in Compose.

## D79 — Android shell's pure decision/validation seams are re-implemented in Kotlin (with tests), not single-sourced over JNI

**Decision.** The pure decision/validation logic behind the Android out-of-match shell
(title-action routing, callsign sanitisation, win-rate math, settings clamping, and the numeric
bounds `SENS_MIN`/`SENS_MAX`, `CALLSIGN_MAX`) is **re-implemented in plain Kotlin and covered by JVM
unit tests** — the [`BuildStamp.kt`](../android/app/src/main/java/com/jaredhoward/goingdark/BuildStamp.kt)
pattern — **not** single-sourced in `core::shell` and called over JNI. The shared *numeric bounds*
are mirrored from `core` with a JVM test asserting the mirrored values, so a drift between platforms
is caught in CI rather than shipping as a silent inconsistency. (Recorded ahead of the work in
[`compose-shell-parity.md`](plans/compose-shell-parity.md) §8.)

**Why.** [D32](#d32--meta-ui--app-shell-native-per-platform-shells-out-of-match-in-engine-in-session)
already establishes that out-of-match *chrome* forks per platform — only the game (sim, netcode,
order/stance vocabulary) is single-sourced in `core` (invariant #2). These seams are presentation
helpers (sanitise a display name, route a button press, clamp a slider), not game logic: they make
no unit decisions, fold into no checksum, and have no determinism obligation. Dragging JNI onto the
hot Compose UI path to single-source a string-trim is disproportionate. The genuine risk is not the
logic forking but the **shared constants** drifting (a sensitivity range or callsign cap that differs
across platforms is a real fairness/consistency bug) — so that, and only that, is pinned with a
mirrored-constants test. The desktop egui shell already keeps these as pure Rust seams
(`app/src/shell.rs`); Kotlin gets the symmetric treatment.

**Consequences.** Each Android shell surface lands its decision/validation seam as testable Kotlin
alongside the (exempt) Compose UI, keeping the CLAUDE.md "logic ships with tests" floor intact for
the shell. The bounds live in one place per platform with a cross-check test; if `core` ever changes
a bound, the Kotlin mirror test fails until updated. Single-sourcing over JNI stays available as a
later option if the duplicated surface ever grows beyond trivial helpers.

## D80 — Real-world battlefield maps: a scripted GIS ingest→bake→lint pipeline, faithful-then-balance-passed

**Decision.** Real-place and historical battlefields are built by a **scripted asset pipeline**
(`tools/maps/`, [`maps.md`](maps.md)), the real-world-sourced sibling of the CT-G procedural
generator ([`content-tooling-plan.md`](plans/content-tooling-plan.md)): **ingest** a lon/lat bounding
box (elevation from a public DEM — Copernicus GLO-30 / SRTM / USGS 3DEP; features from OpenStreetMap),
**bake** the vector features into a deterministic integer `Cover` grid, and **lint** it headlessly for
playability. The imported terrain is **faithful first, then balance-passed**: import the real ground
1:1, then hand-tune where it is structurally unfair (fairness wins — invariant #6); the linter emits
symmetry/density metrics to target that pass. Modern, historical, and real-inspired maps all use the
one pipeline, selected by a per-map `mode`/`fidelity` config.

One source produces **two decoupled artifacts** (invariant #4): a **float render mesh** (real
elevation, detail; render-only) and an **integer, byte-stable cover grid** (the sim's terrain).
Float GIS data lives only offline in the baker and in the render mesh; **only integers cross into the
sim** (invariant #1), so lockstep can't silently desync on a real-world map (invariant #7).

**Why.** Commissioned battlefield art is off-model for this project (D41/D46: every asset is a
committed *generator script* + manifest, not an opaque blob). Real GIS data is abundant, free, and
machine-readable, so a bake pipeline makes "author a real battlefield" a scripted content task — the
same ethos as CT-G, pointed at the real world. The two-artifact split is forced by the invariants: a
faithful DEM is float metres, which must never reach the fixed-point sim, but is exactly what a
render mesh wants. Faithful-then-balance-pass is the honest resolution of realism vs. fairness: a real
valley can be a one-sided killbox, and a competitive/PvE map must be fair, so realism is the starting
material and fairness is the veto.

**Determinism.** The bake runs offline (floats permitted there); its output — the `.covergrid` — is
integer `Cover` per cell, and `Terrain::from_cover_grid`/`apply_cover_grid` build a `Terrain` from it
with integer-only math (invariant #1). `bake.py --verify` asserts a byte-stable re-bake. This is the
D77 "map carries its grid as data → Terrain" primitive; see the interim note below.

**Interim vs. [D77](#d77--content-addressed-terrain-maps-carry-the-grid-persist-serializes-a-content-hash-id-resolves-q22)/[D76](#d76--missionscenario-authoring-format-external-ron-data-files-behind-a-host-side-loader-resolves-q15).**
D77 locks *content-addressed* terrain (`MapId` = content-hash, `Terrain::from_content` lookup, loaded
through the D76 airlock), but the **code is not yet migrated** — `core::terrain` still uses the `u16`
`from_map_id` registry. So the first baked map is wired the only way the current code allows: a
`Terrain::POINTE_DU_HOC_MAP_ID` arm that `include_str!`s its `.covergrid`, plus `Sim::load_map`
(sets `map_id` **and** terrain together so a snapshot round-trips — invariant #7). This is an
**explicit interim bridge**: when the D77/D76 content-set loader lands, the `.covergrid` becomes CT-C
map content identified by its content hash, `from_content` replaces the hardcoded arm, and the bridge
is deleted. `tools/maps/lint.py` is likewise the interim, real-world analogue of the **CT-F**
content-lint + CT-G PvP-symmetry validator.

**Diagnostics.** Map bugs are found two ways (invariant: debug scenes get a live overlay *and* a
headless harness): headlessly via `tools/maps/lint.py` (reachability, sealed pockets, spawn validity,
structure enumeration with cell coords, PNG preview), and in-engine via `render::debug::covergrid_lines`
(F3 draws the sim's actual cover cells) inside a `Scene::MapInspect` sandbox that loads a baked map.

**Consequences.** New real-world maps are a config + `pnpm assets:maps` + `pnpm maps:lint`, not
hand-work. Elevation is render-only until [Q23](open-questions.md#q23--sim-elevation) lands a sim
height layer; water/cliffs are `Cover::Heavy` until [Q24](open-questions.md#q24--terrain-traversal-cost)
adds a traversal-cost layer; destructibility is deferred to
[Q25](open-questions.md#q25--destructible-terrain). Licensing rides the manifest per source (OSM =
ODbL-1.0, attribution + share-alike).

## D81 — Play modes don't funnel through the gunsmith; the gunsmith is loadout customization behind Settings

**Decision.** Tapping a play mode goes **toward the match**, not through the loadout gunsmith. On the
out-of-match shell: **Campaign** opens the Operations hub (mission-select → briefing → **Deploy**),
and **PvE / PvP** open a lightweight **mode/map select** (`ModeSelectScreen`, `shellGameModes`) whose
tiles **Deploy straight into the chosen scene**. The **gunsmith is no longer a play gate** — it is
loadout *customization*, reached on demand from **Settings** (a `GUNSMITH` entry), with no Deploy of
its own; its edits persist ([`ShellPrefs`]) and are folded into whatever match you launch next. The
title→screen routing runs through the unit-tested `resolveTitleAction` seam (`TitleAction.kt`), which
the live Compose router (`MainActivity.Shell`) now **consumes** — previously it hand-wired the same
mapping inline, so the JVM tests covered a function the app didn't run.

**Why.** The old flow put the gunsmith in front of *every* play mode and made **Deploy** the only way
forward — so on a device where the engine hand-off wasn't obvious the gunsmith read as a **dead end**
("I can't get past this screen"), and it made a customization surface a mandatory step for players who
just wanted to play. Loadout tweaking is an occasional, opt-in activity; gating fights it belongs
with the other opt-in preferences (Settings), and the play modes should lead to a *match*, not a
weapon editor. Routing the live navigation through the tested seam closes the drift hazard the two
Kotlin tables carried (the inline `when` was untested).

**Scene tokens.** The mode tiles carry the tokens `engine::lib::Scene::parse` accepts
(`skirmish`, `seize`); `GameModeTest` pins every mode to a known token so a typo can't ship an
un-launchable tile. PvE and PvP share the picker until PvP match-setup exists
([Q5](open-questions.md#q5--single-player-multiplayer-or-both--and-in-what-order--resolved-d58-pve-first)-blocked); their divergence stays future work.

**Owed — desktop parity ([D79](#d79--compose-shell-parity-is-hand-mirrored-not-jni-single-sourced)).**
This landed on the **Android Compose shell** (the reported platform). The desktop egui shell
(`app/src/shell.rs::resolve_title_action`, `app/src/main.rs`) still routes Pve/Pvp through
`OpenLoadout` — reconciling it (add an `OpenModeSelect`/`Screen::ModeSelect`, move the loadout behind
Settings, repoint the Briefing→match hop) is the owed D79 hand-parity half, tracked here so the two
shells' divergence is explicit, not silent.

**Consequences.** `TitleRoute` loses its `Loadout` member (no title action reaches the gunsmith); the
gunsmith's DEPLOY button becomes DONE (save-and-return-to-Settings). New play modes are a
`shellGameModes` entry + a valid scene token. The desktop divergence above must be closed before the
two shells can be called at parity again.

## D82 — Shell parity is bidirectional: desktop adopts D81, both platforms converge features and persistence

**Decision.** A parity pass reconciles the desktop egui shell and the Android Compose shell in **both**
directions — neither is authoritative; each catches up to whatever the other did better.

- **Desktop adopts D81 in full** (closing D81's "owed" half): PvE/PvP route through a new
  `Screen::ModeSelect` (mirroring `shellGameModes` via a shared `engine::shell_modes` seam whose tokens
  resolve through `Scene::parse`), the gunsmith moves behind Settings as customization-only
  (RESET/DONE, no Deploy), and the mode-select / briefing hops become the deploy gate. Match creation
  is refactored into one `App::enter_match()` shared by both paths; the deterministic core is
  untouched (loadout still reaches the sim only via the scenario seeder).
- **About/Field Manual is reachable from BOTH the title and Settings, on both platforms** — resolving
  the prior split where desktop reached it only from Settings and Android only from the title. Desktop
  gains a title `MANUAL` chip (with an `AboutReturn` origin so BACK returns correctly) while keeping
  the Settings entry.
- **Feature catch-up, Android → desktop parity:** the embodied touch HUD gains a **jump** button, a
  **select-fire** toggle with an on-glyph SEMI/AUTO readout, and **look-sensitivity / invert-Y** now
  actually shape the touch-look delta (`TouchControls` seam), where before they were stored-but-inert.
- **Feature catch-up, desktop → Android parity:** Android gains a real **campaign progress model**
  (locked/available/cleared + status pills, prerequisite-gated, mirroring `core::campaign` as a pure
  Kotlin `CampaignProgress` seam), a briefing clear-status line, a `diff` launch-wire key that threads
  the chosen difficulty into `mission_registry` commander-tier application, and campaign-progress
  persistence + record-on-win (via an `Activity.setResult` code).
- **Persistence is symmetric now:** desktop persists Settings/Profile/loadout to `shell.dat`
  (previously in-memory only; Android already had `ShellPrefs`), and Android persists campaign progress
  (previously desktop-only via `campaign.dat`). Each platform gained the other's disjoint persisted
  state.

**Why.** The two shells had drifted in *both* directions — treating either as canonical would have
regressed real work on the other. The About entry-point split was an accident of independent
development, not a considered platform difference (unlike the D78 backdrop), so converging to
"both entry points on both" is strictly more discoverable at trivial cost. The remaining intentional
differences are left as-is: the 2D-Compose vs 3D title backdrop ([D78](#d78--android-title-backdrop-is-compose-native-not-an-embedded-wgpu-surface)), and desktop's fullscreen toggle (mobile has no equivalent).

**Method (process note).** Built by three parallel worktree-isolated workers (one per surface cluster:
Android embodied controls, desktop shell, Android campaign) off a shared base, each shipping tests in
its own commits, then a single integration + full-suite verification pass — the D79-style
"self-contained seam per worker, orchestrator integrates" pattern, extended across platforms.

**Owed / deferred.** The Android campaign result code assumes the single shipped node `NodeId(0)`; a
2nd/gated campaign node needs the node index carried on the launch wire (marked in-code). Android jump
/ fire-mode button *placement* is greybox-reasonable and unit-tested for non-overlap but not
HUD-editor-exposed (WS-D) or device-playtested. On-device look-sensitivity shakeout still pending (no
device on hand).

---

## D83 — Campaign replay difficulty reshapes the *situation*, not a 4th commander band (resolves Q21)

**Decision.** The 4-tier campaign progression/replay coordinate (`core::campaign::Difficulty`:
Recruit / Regular / Veteran / Elite) maps onto the running fight on **two axes**, not one —
resolving [Q21](open-questions.md#q21--replay-tier-to-commander-tier) with **option (iii)**:

- **Commander aggression stays 3 tiers.** The 4→3 collapse applies to the *aggression axis only*:
  Recruit→Recruit, {Regular, Veteran}→Veteran, Elite→Elite. This reuses the shipped, golden-checksum-
  stable `core::mission_tuning::DifficultyParams` bands ([D30](#d30--a-measured-combateconomy-balance-baseline--a-deterministic-balance-metrics-harness)
  context) **unchanged** — no re-measurement of the commander constants.
- **Scenario modifiers carry the full 4-tier resolution.** The complete progression coordinate drives
  `core::mission_tuning::ScenarioModifiers` intensity (starting-force `force_scale_pct`, reinforcement
  cadence `reinforcement_period`, fog `TellMode`, host time limit), so **no two replay tiers produce an
  identical fight** — Regular and Veteran share a commander band but differ in the *situation* they put
  you in.
- A pure mapping seam (`core::campaign::Difficulty → (mission_tuning::Difficulty, ScenarioModifiers)`)
  is the single bridge between the two enums; `engine::mission_registry` consumes it at
  `LaunchedMission` launch, applying the commander tier via `Game::set_commander_difficulty` and the
  reinforcement cadence via `ScenarioModifiers::apply_to_sim`. The `Regular` tier reproduces the
  mission's shipped baseline (neutral modifiers, `Veteran` commander) so the default fight stays
  bit-identical; the other three tiers deviate deliberately.

**Why.** This is the project's *already-locked* difficulty philosophy applied verbatim:
[D30](#d30--a-measured-combateconomy-balance-baseline--a-deterministic-balance-metrics-harness) says difficulty **reshapes the
situation, never the balance numbers**, and `ScenarioModifiers` is exactly that instrument — so
resolving Q21 this way keeps difficulty expressed through one philosophy instead of forking a second
one. It fixes the only real weakness of a bare 4→3 collapse (option (i): two replay tiers feel
identical, making the replay reward cosmetic for one step) *without* paying option (ii)'s cost — a 4th
commander aggression band would force a re-balance of the carefully-tuned D30 constants and add
permanent sim-tuning surface to maintain and keep checksum-folded forever. The honest-AI constraint
(invariants #3/#6, [D39](#d39--the-enemy-is-a-commander-level-scripted-ai-issuing-orders-via-the-lockstep-stream)) holds at every
tier: a harder replay never makes the commander omniscient, it faces you with more bodies / a faster
drip / a harsher fog regime. Every lever is integer, so the mapping stays fixed-point and
checksum-folded (invariants #1/#7). Until this landed, replaying a node at a higher tier recorded a
best-tier badge but the fight was unchanged — the progression coordinate was inert; D83 makes replay a
real player-facing feature.

**Status.** Design locked **and implemented**. The pure `core::campaign` mapping
(`Difficulty::commander_tier` / `scenario_modifiers` / `combat_tuning`) is threaded through
`engine::mission_registry::MissionDef::launch` and the shared `engine::Game::apply_campaign_tuning`
seam, which **both** hosts call right after seeding the mission scene, before tick 0 (`app/src/main.rs`
`enter_match`; `pal-android` `android_backend`, mapping the `diff` wire rank via `Difficulty::from_tier`).
The four modifier profiles (easiest→hardest): force `90/100/115/130`%, reinforcement cadence
`900 / None(=600 baseline) / 360 / 240` ticks, fog `Marked / Subtle / Subtle / Hidden`. `Regular` is
exactly the neutral baseline (byte-identical seed). The `default`/`stress` checksum streams are
untouched (those scenes never launch a mission); mission-scene tests pin: `Regular` == bare-seed
baseline byte-for-byte, four distinct monotonic profiles, and same-tier peer parity + cross-tier
divergence. Magnitudes are situation dials, tunable in playtest.

**Live vs declared (honest scope).** Of the four `ScenarioModifiers` levers the profiles carry, only
**two currently reach the fight**: the commander aggression band and the reinforcement cadence
(`reinforcement_period` → `apply_to_sim` → the enemy purse's accrual, folded into the checksum
transitively via `resources`). This alone makes all four tiers distinct in-fight and passes the
cross-tier divergence test. The remaining three levers — `force_scale_pct`, `fog` (`TellMode`),
`time_limit_ticks` — are **declared per-tier but not yet consumed** (no seeder/host reads them today —
the same posture the *authored* briefing modifiers always had). This is a functional no-op, identical
on both platforms, so it is not a desync risk (confirmed by a determinism audit). **Owed follow-up:**
thread `force_scale_pct` into the `core::scenario` seize seeder (it must be applied *at* spawn, before
tick 0) and route `fog` into the detection `TellMode` (the checksum-excluded going-dark tell, D33) so
the harder tiers deliver the *fuller* situation the profiles advertise — especially fog, which is the
invariant-#6 intel lever.

**Cross-link:** [`plans/pve-campaign-plan.md`](plans/pve-campaign-plan.md) WS-B/WS-E,
[D30](#d30--a-measured-combateconomy-balance-baseline--a-deterministic-balance-metrics-harness),
[D39](#d39--the-enemy-is-a-commander-level-scripted-ai-issuing-orders-via-the-lockstep-stream).

## D84 — Animation floor (CP-3/WS-B) is a pure clip-selection seam + a procedural pose; rig authoring lands, skeletal playback deferred

**Status: landed floor.** The first slice of the animation floor (visual-design-plan **WS-B**,
positioning **CP-3** — the *conceded* "not jarring" tier). Two render-side seams + one authoring
artifact, all presentation-only:

- **Clip-selection seam (`render::anim::select_clip`).** A pure, total classifier mapping a small
  presentation `AnimState { speed, firing, alive }` → `AnimClip ∈ {Idle, Walk, Fire, Death}`, with a
  fixed priority order: `Death` ▶ `Fire` ▶ `Walk` ▶ `Idle`. Read off the interpolated render snapshot
  at `interpolate_instances` (speed = magnitude of the pre-existing `UnitSnapshot::vel`; firing =
  `UnitSnapshot::firing`) — **no new sim surface**. This is the load-bearing piece: the eventual
  skeletal player slots in *behind* this same enum.
- **Procedural pose (`render::anim::anim_pose` + `pose_matrix`).** Until a real skeletal/rigid-part
  runtime exists, playback is a cheap per-instance pose — a vertical bob (idle breathing / walk
  stride), a forward lean, a recoil pitch, a death topple — folded into the token model matrix and
  **gated to infantry** (`is_infantry`). `pose_matrix(_, _, _, AnimPose::REST)` is byte-identical to
  `mesh::model_matrix`, so vehicles/structures and units at rest render exactly as before. Two new
  **trailing CPU-side** `UnitInstance` fields (`anim_clip`, `anim_phase`) carry it, off the GPU quad
  instance layout (same pattern as `model`/`hull_yaw`/`kind`).
- **Rig authoring (`tools/models/gen_trooper_rig.py`, `pnpm assets:rig`).** A Blender generator that
  rigs the greybox trooper as **rigid box parts on a 7-bone hierarchy** (each part bound 1.0 to one
  bone — no soft vertex weights) and bakes four clips (idle/walk/fire/death) to
  `assets/models/rigs/trooper_rig.glb` with real glTF animation channels + a provenance manifest
  (`source`/`license`/`sha256`), script-not-binary per [D41](#d41--ai-generated-placeholder-models-for-all-render-content-skip-commissioned-art-for-now)/[D46](#d46--the-headless-asset-tooling-toolbox-one-scriptable-cli-per-content-lane). Deterministic (bit-identical regen).

**Why.** WS-B is the conceded animation tier — the bar is *"not jarring,"* not UE5 parity. Splitting
it into (1) a pure clip **seam**, (2) a procedural **stand-in**, and (3) glTF **authoring** lets the
floor ship value now (units visibly bob/lean/recoil in both the command and embodied viz scenes)
while keeping the expensive part — a runtime skeletal/rigid-part loader that consumes the authored
`.glb` — as a clean follow-up *behind the already-stable `AnimClip` interface*. It respects every
invariant: it lives entirely on the render/float side (invariant #1/#4), reads a presentation copy of
unit state and never writes back to the sim, adds no checksum surface (the `default`/`stress` streams
stay bit-identical), and adds **no** render crate dependency (still `wgpu`+`bytemuck`).

**Owed follow-up.** (1) A runtime skeletal/rigid-part player that consumes `trooper_rig.glb` and
drives the bones from the `AnimClip` (replacing the procedural pose). (2) `AnimClip::Death` is
selectable + tested but not *driven* at runtime today — dead units are dropped from the snapshot
(`core::snapshot::Snapshot::capture`), so a visible death topple needs cross-tick unit identity + a
death linger. Both are honestly disclosed in `render::anim`'s module doc.

**Cross-link:** [`plans/visual-design-plan.md`](plans/visual-design-plan.md) WS-B,
[D41](#d41--ai-generated-placeholder-models-for-all-render-content-skip-commissioned-art-for-now),
[D46](#d46--the-headless-asset-tooling-toolbox-one-scriptable-cli-per-content-lane),
[D74](#d74--visual-design-foundation-a-central-theme--an-anti-aliased-font-atlas-replaces-the-57-bitmap).

## D85 — Gunsmith breadth (CP-1): Stock + Muzzle become sim sidegrade slots; Grip is cosmetic-only

**Status: decided + IMPLEMENTED (Stock + Muzzle sim slots + Grip cosmetic landed; fold byte-neutral,
243-build fairness proof + 2-peer checksum agreement green, D69/D70 RPS re-validated).** Resolves the design fork in extending the
[D60](#d60--gunsmith-is-horizontal-fixed-point-sidegrades-checksum-folded) gunsmith from its 3
sim slots (Optic / Barrel / Magazine) to the six categories a CoD-Mobile player expects (optics,
barrel, **stock**, mag, **grip**, **muzzle**) — **horizontal only** (sidegrades, never vertical
power). Full implementation spec below (from a design pass over `core::gunsmith`/`combat`/`systems`).

**The problem.** D60's no-strict-domination proof rests on **disjoint axis pairs**, and the 3 existing
slots already consume all six tracked stat axes (Optic = range↔cooldown, Barrel = damage↔reserve,
Magazine = mag_size↔reload). New slots that are *real* sim sidegrades therefore need **new disjoint
axis pairs** — new sim-meaningful `Weapon` axes.

**Decision.**
- **Stock — sim slot.** New pair `move_speed_delta ↔ cone_cos_delta` (mobility vs. steadiness).
  `move_speed_delta` (higher=better) offsets carrier speed in the embodied `Locomote` path **and** the
  AI mover (`orders.rs`), *preserving the zero-delta fast path* so every Standard/legacy unit stays
  bit-identical. `cone_cos_delta` (higher=tighter) offsets the embodied aim cone in `resolve_fire`
  (embodied-only by nature — AI `can_engage` has no cone). Options: `Agile` (+move,−cone) / `Marksman`
  (−move,+cone).
- **Muzzle — sim slot.** New pair `supp_out_delta ↔ falloff_delta` (blast/suppression vs. downrange
  retention). `supp_out_delta` (higher=better) offsets suppression-per-hit at both hit sites (AI +
  embodied). `falloff_delta` (lower=better) applies a **sqrt-free**, `dist_sq`-bucketed damage falloff
  beyond `range/2` (multiplier `ONE` when zero ⇒ byte-neutral). Options: `Brake` (+supp,+falloff) /
  `Suppressor` (−supp,−falloff).
- **Grip — cosmetic/feel-only, NOT a sim slot.** Grip's real identity is recoil/hipfire feel, which
  is **presentation-only** (invariant #4 / CP-2 — the sim models no recoil). Forcing a sim axis would
  invent a fake mechanic. Grip lives on the render/cosmetic surface; the player still sees six
  gunsmith rows, five functional + Grip tuning feel.

**Why this stays fair + deterministic.** The two new slots add two **disjoint** axis pairs, so D60's
proof generalizes unchanged (any two loadouts differ in ≥1 slot contributing one strictly-good and
one strictly-bad component no other slot can cancel); the exhaustive `no_loadout_strictly_dominates`
test grows to `3^5 = 243` builds, per-army too. All four axes are `Fixed`, zero-default, appended to
`Sim::fold` after `shell` (mirrored in deserialize) — byte-neutral by the
`turret_speed`/`muzzle_vel`/`penetration` precedent, so every existing golden checksum is unmoved.
`GunsmithPool`/`pool_for` gain the four axes for Neutral/Us/Fr.

**Implementation** (files + full test plan — the ready-to-execute spec): `components.rs` (4 Weapon
fields + fold + deserialize), `gunsmith.rs` (StatDelta axes + polarity, two `slot_enum!`s, pool +
apply), `combat.rs`/`systems.rs`/`orders.rs` (the four mechanics, fast-path-guarded),
`engine/loadout_ui.rs` (two UI slots), `customization.md` §1. Tests: generalize every D60 fairness
test to 243 builds (+ per-pool), byte-neutral-default + golden-checksum-unmoved, 2-peer checksum
agreement with stock/muzzle selections, and a per-mechanic combat test each. **Slice Stock first**
(cheapest — no new damage curve), then Muzzle (lands falloff; re-validate the
[D69](#d69)/[D70](#d70) Rifleman↔Heavy RPS with `sim-runner --metrics`). This is a
sim-touching, `/safe-edit`-class change (invariant #1/#7) — implement behind the coder↔reviewer +
determinism-auditor loop, not fire-and-forget.

**Cross-link:** [D60](#d60--gunsmith-is-horizontal-fixed-point-sidegrades-checksum-folded),
[`roadmap.md`](roadmap.md) CP-1, [`plans/pve-campaign-plan.md`](plans/pve-campaign-plan.md) WS-C,
[`customization.md`](customization.md) §1.

## D86 — Camp spawn-rally is authoritative folded sim state, inherited as a literal-executor first Move

**Status: landed.** Closes the dangling rally seam the roadmap flagged under *Playable game loop →
Troop-training UI* (the `engine::train_ui::rally_point` quantizer existed but had **no sim `Command`**
to emit into). A camp can now hold a spawn-rally point, and units it produces path there on spawn.

- **Command + state.** New `Command::SetCampRally { camp, rally: Vec2 }` (applied via
  `economy::set_camp_rally`, a no-op on a dead/non-building handle) writes a new `Building.rally:
  Option<Vec2>` component field (default `None`). `engine::train_ui::rally_commands` quantizes the
  tapped world point through the existing pure `rally_point()` seam and emits the command.
- **Literal-executor inheritance (invariant #3).** In `economy_system`'s spawn pass a produced unit
  receives `Order::MoveTo(rally)` as its **first** order when the camp has a rally set, else `Idle` as
  before. The unit just walks to the point — **no autonomous pathing/AI**; depth stays in the
  order/stance vocabulary, not the brain. The rally applies only to units produced *after* it is set
  (no re-issue to in-flight units) — the minimal deterministic behavior.

**Why.** A spawn rally is genuine, authoritative game state — two clients must agree on it or units
diverge — so it is **folded into the per-tick checksum**, serialized in `core::persist`, and encoded
in the `core::lockstep` wire codec (new **tag 18**), never a presentation-side convenience. That fold
shifted the raw serialized stream by design, so it bumps both format versions —
**`SNAPSHOT_VERSION` 10 → 11** and **`WIRE_VERSION` 9 → 10** — and re-pins the camp-bearing golden
checksums (ballistic duel, sim-runner duel/infantry), fights byte-identical. Ships with `core` tests
(command writes the field; produced unit inherits the rally; rally folds into the checksum;
persist round-trips) + an `engine` `train_ui` test, green dev+release; 2-peer lockstep agreed with no
desync.

**Cross-link:** [D18](#d18) (SoA ECS), [D27](#d27) (lockstep), [D28](#d28) (snapshot/persist),
invariant #1/#3/#7, [`roadmap.md`](roadmap.md) *Playable game loop → Troop-training UI*.

## D87 — Runtime skeletal playback landed, completing the D84 deferral

**Status: landed.** [D84](#d84--animation-floor-cp3ws-b-is-a-pure-clip-selection-seam--a-procedural-pose-rig-authoring-lands-skeletal-playback-deferred)
shipped the animation floor as a clip-selection seam + a procedural pose and **explicitly deferred**
the runtime skeletal player that consumes the authored rig. That deferred half is now built: the
generic `ModelKind::Trooper` draws through the authored 7-bone rigid-part rig
(`assets/models/rigs/trooper_rig.skel`, cooked `GDSK`) instead of the procedural stand-in, in **both**
the command-view token pass and the embodied world pass.

**Why.** As D84 designed, the skeletal player slots in *behind* the already-stable `AnimClip`
interface, so nothing upstream changed. "Skinning" is one matrix per rigid part —
`model = place · A_bone(t) · inverse_bind[bone]` — drawn as ordinary instanced `MeshInstance`s
through the **existing** `MeshPipeline`: no new shader, no skinning shader, no new vertex attribute,
no glTF-at-runtime (render stays `wgpu` + `bytemuck` only, [D19](#d19)/[D46](#d46)). Render never
touches sim state (invariant #4) — all inputs are the already-interpolated float render snapshot.
Since `interpolate_instances` forces `Army::Neutral`, essentially all in-game infantry are generic
Troopers, so this animates the whole roster; per-faction rigs (`TrooperUs`/`TrooperFr` keep the
procedural floor) and runtime-driven death (dead units drop from the snapshot) remain follow-ups.
491 render tests green dev+release.

**Cross-link:** [D84](#d84--animation-floor-cp3ws-b-is-a-pure-clip-selection-seam--a-procedural-pose-rig-authoring-lands-skeletal-playback-deferred),
[D19](#d19), [D46](#d46), invariant #4, [`roadmap.md`](roadmap.md) CP-3 / *Art & assets*,
[`plans/visual-design-plan.md`](plans/visual-design-plan.md) WS-B.

## D88 — `core::scenario::ScenarioBuilder` lands the content-tooling spine (CT-A)

**Status: landed.** [D76](#d76) settled the mission/map authoring format as **external RON data behind
a host-side loader driving a serde-free `core` builder**;
[`plans/content-tooling-plan.md`](plans/content-tooling-plan.md) CT-A is the first slice of that
build-out and is now in `core::scenario`. `ScenarioBuilder<'a>` borrows `&mut Sim` and exposes a
serde-free, float-free, fixed-point API over the private spawn/build primitives the hand-written
seeders already call — `set_income` / `set_army` / `set_purse` / `control_point` / `spawn(kind, pos,
faction, stance, facing)` / `build_camp` — plus a `sim_mut()` escape hatch for mission-specific
shaping (loadout apply, terrain, baked orders). `seed_seize_mission` **and** `seed_skirmish` were
refactored to build *through* it (kept as the living oracle, not deleted).

**Why.** The builder gives the D76 host-side RON loader a `core`-clean seam to target without dragging
serde into `core` (invariant #2) or adding per-tick checksum surface (invariant #7). The refactor is
pinned **byte-identical** by golden opening checksums (Seize `0x474cdbf2ad913ecb`, skirmish
`0x3b1d9e207ce97e65`) plus a test that rebuilds Seize through the *public* builder API alone and
matches both the seeder and the golden — so the determinism matrix covers a builder-seeded `Sim` for
free. 557 `core` tests green dev+release; the sim-runner checksum stream is unchanged.

**Note for CT-B:** the builder borrows rather than owns a `Sim` (`let mut sim = Sim::new(seed); {
ScenarioBuilder::new(&mut sim)… }; sim`); mission-specific shaping is via `sim_mut()`, so the RON
loader may want thin builder wrappers for the common shaping ops if the schema warrants it.

**Cross-link:** [D76](#d76), [D77](#d77),
[`plans/content-tooling-plan.md`](plans/content-tooling-plan.md) CT-A, invariants #1/#2/#7.

## D89 — Replays are a seed + an input log, proven by checksum equality (PC-3)

**Status: landed (single-client foundation).** A match is fully determined by its RNG seed and the
per-tick ordered `Command` stream (invariant #1), so replay/spectating need store **no world state** —
only seed + inputs. Implemented as the headless `replay-runner` crate (sibling to
`sim-runner`/`net-sim-runner`, depending only on `gonedark-core`): **record** captures the command log
while driving a scenario; **playback** re-seeds from `(scenario, seed)` and re-feeds only the recorded
log; the load-bearing proof is that the playback per-tick checksum stream (`Sim::checksum`, invariant
#7) is **bit-identical** to the record run, verified *through a serialized on-disk artifact* (`GDRP`
magic + version + tagged commands). `pnpm desktop:replay` runs the round-trip.

**Why.** Determinism already guarantees reproducibility; this cashes it in for near-zero cost and hands
netcode/QA a replay artifact for desync forensics, plus a cheap PC/e-sports differentiator
([`positioning-pc.md`](positioning/positioning-pc.md) PC-3). The replay byte codec lives in the runner,
**not** `core` (invariant #2: `core` stays serde-free/dep-free); it mirrors `core::lockstep`'s wire
discipline — raw `Fixed` bits, exhaustively-tagged enums (adding a `Command` variant is a compile
error, not a silent drop), loud decode errors. **Scope:** single-client (one ordered command stream);
multi-peer per-peer ordering and a *rendered* spectator view are follow-ups on this same seed+log
foundation. 9 crate tests green dev+release. Opens [Q26](open-questions.md).

**Cross-link:** invariants #1/#2/#7, [D27](#d27) (lockstep wire discipline),
[`positioning-pc.md`](positioning/positioning-pc.md) PC-3, [`roadmap.md`](roadmap.md) PC-3.

## D90 — Desktop key-rebind editor is a pure `engine::keybind` seam wired at the `app` boundary

**Status: landed (host toggles).** The owed rebind half of the [D75](#d75) Settings surface: the
rebindable host actions (Pause/Esc, Toggle-fullscreen/F11, Toggle-debug-overlay/F3) now bind through a
`KeybindMap` in a new `engine::keybind` module — defaults, conflict-rejecting `rebind` (a shared key
returns `RebindOutcome::Conflict(owner)` and leaves the map untouched), reset, and an
ordinal-based `encode`/`decode` (tolerant: garbage/out-of-range/duplicate → defaults) reusing the
`QualityChoice` persisted-ordinal pattern. The egui Settings screen gained a click-to-arm "KEY
BINDINGS" section; `app` maps `winit::KeyCode` / egui `Key` ↔ the neutral `KeyId` at the boundary and
resolves live keys through the map instead of hardcoded matches; the binding persists via the existing
shell-prefs codec.

**Why.** Keeping the model in `engine` holds invariant #2 (engine pulls in **no** windowing crate — the
winit/egui↔`KeyId` mapping lives only in `app`), makes the rebind/conflict/persistence logic
unit-testable winit-free (11 engine + relevant app tests, green dev+release), and keeps the egui
capture flow thin over a tested type. Keybinds are presentation/input-only — never sim or checksum
state. **Scope:** only the `app`-owned host toggles are rebindable; the `pal-desktop` gameplay keymap
(move/fire/embody/build/train/upgrade/…) stays hardcoded because it decodes in a different crate —
extending the map to it is a PAL-boundary change, deferred as [Q27](open-questions.md).

**Cross-link:** [D75](#d75), invariant #2, [`roadmap.md`](roadmap.md) Settings / *UI-UX polish*.

## D91 — The content-tooling format layer lands: RON mission/map airlock, archetype vocab, content lint, procedural map generator (CT-B/C/E/F/G)

**Status: landed.** Building on the [D88](#d88) `ScenarioBuilder` spine, five parallel
[`plans/content-tooling-plan.md`](plans/content-tooling-plan.md) workstreams landed together, turning
mission/battlefield authoring from an engineer-recompile task toward a designer-edit-a-file task:

- **CT-B — `engine::mission_format`.** A host-side `#[derive(Deserialize)]` `MissionSpec`
  (`deny_unknown_fields`) + float-airlock loader mapping the RON schema onto the CT-A builder. Every
  numeric field is an **integer** converted integer→`Fixed`/`Angle`; there is no `f32`/`f64` in the
  type graph from file to sim, so a float literal cannot even deserialize (invariant #1). serde/RON
  live in `engine`, never `core` (invariant #2). `missions/seize.mission.ron` loads a `Sim`
  **byte-identical** to `seed_seize_mission` — opening checksum `0x474cdbf2ad913ecb` — the oracle
  proving the format is a faithful re-expression, not a second code path. Fail-loud validation
  (`MissionLoadError`) resolves every objective/force ref; rejection battery green.
- **CT-C — `engine::map_format`.** The spatial half factored into a `MapSpec` (terrain map-id,
  control points, cover props, spawn zones), same airlock discipline — all-integer, `deny_unknown_fields`,
  range/overlap-checked, fails loud. Applies onto a `ScenarioBuilder`; the same map applied twice is
  byte-identical. Ships `maps/crossroads.map.ron`.
- **CT-E — objective archetype vocabulary.** `ObjectiveSet::mission_push`/`mission_assassinate`/
  `mission_extract` as **pure composition** of the existing `Capture`/`Eliminate`/`Reach` evaluators —
  no new `ObjectiveKind`, no new sim state, zero checksum surface (invariant #7). (Push is a flat set of
  required Captures — lane *order* would need host-side evaluator state for no win/lose benefit, so it's
  deliberately not enforced.)
- **CT-F — content-lint harness** (`engine/tests/content_lint.rs`). A headless, no-GPU guard: every
  shipped mission seeds **deterministically** (double-seed + 180-tick checksum stream identical),
  every objective target **resolves in the seeded world**, and the campaign graph is well-formed. A
  deliberately-broken-target fixture proves the lint has teeth. A `LintTarget` seam lets a future
  loaded `*.mission.ron` reuse every assertion unchanged. Built against the code-built registry (the
  RON files ride the same harness once wired).
- **CT-G sibling — procedural map generator** (`tools/maps/generate.py`). Offline tooling only (own
  seeded `random.Random`, never touches `core`); emits bake.py-format `.covergrid` maps with
  mirror/rotational symmetry, verified by the existing `lint.py --pvp` (symmetric maps pass; a
  wrong-transform map correctly ERRORs) — the fairness gate for the LEAD symmetric-PvP shape. Same seed
  → byte-identical output. Output under `assets/maps/generated/`.

**Why.** [D76](#d76) settled the format question; this cashes it in while holding every guardrail: the
loader is the single float airlock, `core` stays serde-free, the data file never enters the checksum
(only the seeded `Sim` does — same footing as a hand seeder, so the cross-arch matrix needs no new
coverage), and authored/generated content is deterministic and lint-guarded. Built in five isolated
git worktrees and integrated on `main`; the only overlap was additive `engine/Cargo.toml`
(serde/ron) + `lib.rs` mod lines. Post-integration: **engine 437 lib + 6 content-lint tests green
dev+release**, **core 557 green dev+release** (untouched — determinism floor intact), workspace `cargo
check` clean, maps `lint.py --self-test` 14/14.

**Remaining (per the plan):** CT-D (data-backed registry + between-match hot-reload) is the payoff slice
still owed — it points `mission_registry` at a content directory of the CT-B/CT-C files and wires the
CT-F lint over them; CT-G's terrain half builds against [D77](#d77) content-addressed terrain.

**Cross-link:** [D76](#d76), [D77](#d77), [D88](#d88), invariants #1/#2/#7,
[`plans/content-tooling-plan.md`](plans/content-tooling-plan.md) CT-B/C/E/F/G,
[`maps.md`](maps.md) (the real-world CT-G sibling), [`roadmap.md`](roadmap.md) PC-4.

---

## D92 — Impassable terrain tier + obstacle collision & pathfinding; props become sim-owned map data (closes Q24)

**Status: landed** (`core` impassable tier + collision + obstacle-aware flow field + the
`core::obstacles` layout; `render` draws props from it). Workspace suites green in both cargo
profiles; the skirmish opening golden checksum is unchanged.

**Decision.** Three long-standing gaps closed together, resolving [Q24](open-questions.md):

- **`Cover::Impassable`** — a distinct tier that blocks **movement** (new), and (like `Heavy`)
  blocks sight and mitigates fire. `Heavy` stays *passable* concealment (a tall hedge / low wall
  you fire over), so the two are no longer conflated. Baked-map `'#'` (walls / water / buildings)
  now maps to `Impassable` instead of `Heavy` — which *aligns the engine with `tools/maps/lint.py`*,
  which already treated `'#'` as impassable for its connectivity flood-fill. Sight behaviour for
  `'#'` cells is preserved (`Impassable` blocks sight too).
- **`core::obstacles`** — the single source of truth for the skirmish's visible props (trees /
  rocks / crates / sandbag berms / turret emplacements). It lives in `core` (static map data, like
  terrain: never mutated per tick, never folded into the checksum), paints `Impassable` cells under
  each prop, and is **four-fold symmetric** so — now that a prop is real collision/cover — it
  favours no side or flank (invariant #6). The renderer *reads* this list to draw the props
  (`core → render`); it no longer owns a private `PROP_LAYOUT`.
- **Obstacle-aware pathing + collision.** `FlowField::build` never relaxes a path *into* an
  impassable cell, so AI units route **around** obstacles (the Phase-2 generalisation the flow-field
  docs always anticipated). A new `systems::resolve_terrain_collisions` pushes any mover — including
  the embodied avatar, which doesn't pathfind — out of a solid cell (minimal-penetration axis first,
  so a unit grazing a wall slides along it), the terrain analogue of `resolve_building_collisions`.

**Why.** The player could walk through almost everything they saw: the embodied props were
render-only with **no sim body** ([D50](#d50) shipped them as a cosmetic `PROP_LAYOUT` and flagged
the fix — *"if props ever need to be gameplay cover they must become sim … data, never a render-side
back-channel to the sim — invariant #4"*), and terrain cover cells never blocked movement at all.
[Q24](open-questions.md)'s lean was an explicit per-cell **cost** layer; a binary **impassable
tier** is the smaller honest step that fixes the actual bug (you can't enter a wall) without a new
authored cost grid — graded traversal cost (mud/slope) remains future work if the game needs it.
Making `core` own the obstacle layout (not a per-tick ECS entity, which would bloat the snapshot for
static geometry) keeps props on the same footing as terrain — static map data the renderer reads —
which satisfies D50's invariant-#4 requirement without new checksum surface. It is **byte-neutral on
an open field** (no impassable cells ⇒ the flow field and the resolve pass are no-ops), so existing
replays/cross-arch checksum streams are untouched; the skirmish opening golden still matches because
terrain is not folded into the checksum. Fixed-point and index-ordered throughout (invariants #1/#7).

**Consequences.** `Cover` gains a variant, so exhaustive matches (the `render::debug` cover overlay)
gained an `Impassable` arm (a distinct hot-orange in the map debug view). `FlowField::build` /
`FlowFieldCache::new` now take the terrain; the cache holds it so `get` is unchanged. The skirmish
prop layout is now visibly **symmetric** (mirrored), a small cosmetic change from the old scattered
dressing. Graded traversal **cost** (the other half of Q24's lean) is deliberately still open —
reopen it as a new question if marsh/slow-mud is ever scoped.

**Cross-link:** [D50](#d50), [D28](#d28)/[D77](#d77) (terrain as static, content-addressed map
data), invariants #1/#4/#6/#7, [`maps.md`](maps.md), [`architecture.md`](architecture.md),
[Q24](open-questions.md) (closed), [Q25](open-questions.md#q25--destructible-terrain) (prop
destruction still leans on entity props — unaffected).

## D93 — Multi-peer replays merge per-peer command sets in ascending peer order, matching lockstep (PC-3)

**Status: landed.** Extends the [D89](#d89) replay foundation from a single command stream to a
**multi-peer** form for lockstep PvP matches, where each tick's inputs come from several peers. A
`MultiReplay` keeps every peer's commands *separately* per tick (`tick → peer → commands`, both
`BTreeMap`s) and merges them at playback into a single ordered set: **every peer's commands
concatenated in ascending peer-id order**. That is byte-for-byte the merge
`core::lockstep::Lockstep::try_advance` performs (it iterates its per-peer slots by index and
`extend`s), so a recorded PvP match replays to the same sim the live lockstep loop produced. Because
the log is keyed by peer id, the merge is **order-independent by construction**: the headline test
rebuilds a replay from its `(tick, peer)` arrivals forward *and* fully reversed and asserts both
identical encoded bytes and an identical per-tick checksum stream. A distinct `GDMP` magic (vs. D89's
`GDRP`) makes a single-peer artifact fed to the multi-peer decoder — or the reverse — fail loudly at
`BadMagic`, and a new `PeerOutOfRange` decode error mirrors `core::lockstep`'s. `pnpm
desktop:replay:multi` runs the round-trip (2 peers, 48 commands over 300 ticks, bit-identical).

**Why.** Determinism makes this near-free (a match is still just seed + ordered inputs, invariant #1);
the only real design point is the intra-tick ordering, and matching lockstep's fixed peer order is the
one choice that keeps a recorded PvP match faithful. Codec stays in the runner, not `core` (invariant
#2: `core` serde-free); the ordering rule is the same stable application order `Sim::step` relies on
(invariant #7). 10 new crate tests green dev+release; no checksum assertion weakened.

**Scope / follow-up.** The *rendered* spectator view (rendering a playback) is still deferred — it is
GPU-gated render-side work and can consume `MultiReplay::merged_for`/`arrivals` directly when built.

**Cross-link:** invariants #1/#2/#7, [D89](#d89) (replay foundation), [D27](#d27) (lockstep wire
discipline + per-peer ordering), [`positioning-pc.md`](positioning/positioning-pc.md) PC-3,
[`roadmap.md`](roadmap.md) PC-3.
