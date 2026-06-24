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
  tell. If [Q2](open-questions.md) lands on the marked-hero option, that marker is
  engine-owned — skins render *under* it, never over it.
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
([`phase-1-plan.md`](phase-1-plan.md) §2).

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
([`phase-1-plan.md`](phase-1-plan.md) §2).

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
([`phase-1-plan.md`](phase-1-plan.md) §2).

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
which Phase 1 deliberately does not have ([`phase-1-plan.md`](phase-1-plan.md) §8). So the
dual-rate re-evaluation belongs to **Phase 3** (200-unit stress + thermal profiling on target
hardware — [`roadmap.md`](roadmap.md)), not here, and is not a reason to add a second clock now.

**Consequences:**
- [Q10](open-questions.md) **closes**; `TICK_HZ = 60` is the Phase-1 lock.
- **Phase 3** owns the 200-unit thermal re-evaluation that could reopen a dual-rate split; the
  named constant keeps the door open for a cheap re-tune or split if profiling demands it.
- No invariant changes: the sim stays **fixed-point** (invariant #1) and **decoupled** from
  render (invariant #4) at whatever rate; a faster tick admits no floats.
