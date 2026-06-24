# Phase 0.5 — Embodiment-over-network latency spike *(plan)*

> **Status: NEXT** (Phase 0 passed — [`decisions.md`](decisions.md) D14). This is the
> gate before any engine code. Throwaway, like Phase 0.
>
> **Goal:** prove embodied FPS combat feels acceptable under the chosen
> deterministic-lockstep + input-delay netcode — *or* decide to change the netcode model
> ([`open-questions.md`](open-questions.md) Q7) or tick rate (Q8) — **before** committing
> the engine spine.
>
> **Exit criterion (from [`roadmap.md`](roadmap.md)):** a credible path to good embodied
> combat feel over the net, *or* a decision to change Q7/Q8 before Phase 1.

---

## 1. The precise question

Lockstep + input delay is **RTS-optimal and FPS-hostile** (see
[`architecture.md`](architecture.md) §"Embodied combat over lockstep"). Input delay
**executes inputs several ticks in the future** so every peer has them in time — invisible
for top-down command, but for a first-person gunfight it is *fixed latency between your
finger and your avatar aiming/firing.*

The spike answers one feel question, in hand, under real delay:

> When I'm embodied and shooting another player, does my **own avatar** feel responsive —
> or mushy and laggy — and does **avatar-local prediction** rescue it if raw lockstep
> doesn't?

Everything else in this plan serves answering that honestly.

## 2. Non-goals — what this spike is NOT

Phase 0 stayed faithful but disposable; Phase 0.5 does the same. Explicitly out of scope:

- **No fixed-point determinism.** This is a *latency/feel* test, not a determinism test.
  Floats are fine here (throwaway). Both peers run identical code on the same exchanged
  inputs, so they stay close enough for a short 1v1 — we are **not** checksumming, and the
  spike validates *nothing* about Phase 1's bit-exact fixed-point sim. Say so in the
  write-up; don't let "it networked fine" be misread as "determinism is proven."
- **No 200-unit sim.** Two embodied avatars only. The lockstep *pacing-under-load*
  question is Phase 1/Phase 3, not this.
- **No real netcode stack.** Direct IP, no matchmaking/relay/reconnect/snapshotting.
- **No audio.** Still carried as a D14 caveat; not load-bearing for the latency judgment.
- **No attention-split test.** This is a pure gunfight harness; the command↔embody divide
  was Phase 0.

This spike tests **1v1 embodied feel over a wire**. Nothing more.

## 3. Approach — extend the Phase 0 Godot prototype

Reuse `prototypes/phase0-controls/` rather than build fresh. It already has the embodied
FPS controls, a 3D arena, hitscan fire, and the touch scheme — the expensive parts. We add
a second networked avatar and a tick/netcode layer on top. Work in a **copy**
(`prototypes/phase0.5-netfeel/`) so the Phase 0 reference artifact stays intact until both
are deleted.

Transport: Godot high-level multiplayer (`ENetMultiplayerPeer`), host/join by LAN IP —
runs on Android. We do **not** use Godot's scene-replication; we exchange **input
commands** and tick both peers ourselves (that *is* the lockstep model — see §5).

## 4. The harness — components to build

1. **Fixed-tick loop** decoupled from render — an accumulator stepping the sim at a
   selectable **30 Hz or 60 Hz** (Q8 arm), render interpolates as in Phase 0.
2. **Per-tick input command**: `{ tick, move:Vec2, look:Vec2Δ, fire:bool }`, serialized
   compactly.
3. **Input exchange + scheduling buffer**: an input sampled at tick `T` is stamped to
   execute at `T + D` (D = input-delay ticks, §6). Both peers apply both avatars' inputs
   at the scheduled tick. If a peer's input for the due tick hasn't arrived, the tick
   **stalls** (the lockstep "slowest peer paces everyone" gotcha — brief, for 2 peers).
4. **Latency injector** (the key instrument): wrap outbound packets in a queue applying
   tunable **added delay + jitter + loss**, so we can sweep real-world mobile RTT on a LAN
   that is otherwise ~0 ms. Runtime-adjustable.
5. **Two modes, hot-swappable at runtime** (§5): pure lockstep vs avatar-local prediction.
6. **Two avatars that fight**: reuse hitscan fire; add hit feedback + a hit/down readout so
   "did my shot land when I aimed" is legible. Simple respawn-in-place to keep testing.
7. **Diagnostics HUD**: current mode · tick rate · injected RTT/jitter/loss · measured
   one-way delay · input-delay D in ticks/ms · a felt "input→avatar-acts" latency readout.
8. **In-hand A/B controls**: buttons to cycle mode, tick rate, and latency preset *without
   a rebuild* — so you can feel two configs back-to-back in seconds.

## 5. The two netcode models under test (Q7)

**Mode A — Pure lockstep + input delay** (the cheap baseline). Your input at tick `T`
executes at `T+D` on *both* peers, including your own avatar. Your avatar does not move/aim
/fire until `T+D`.

```
 Pure lockstep, D = 2 ticks (~66 ms @ 30 Hz):
   tick:        T0    T1    T2    T3    T4
   press FIRE at T2 ──────────────────┐
   avatar fires (both peers) .........│..... T4   ← ~66 ms finger→fire latency
```

**Mode B — Avatar-local prediction** (the current lean). Your *own* avatar responds to
your input **immediately** in the presentation/input path; the authoritative result still
resolves at `T+D`; each authoritative tick **reconciles** predicted vs confirmed (snap or
short blend). The *remote* avatar stays pure lockstep + interpolation. The prediction must
**never feed back into shared sim state** (in Phase 1 that would desync; here it's the
discipline we're rehearsing).

```
 Avatar-local prediction:
   press FIRE at T2 ─┐
   local avatar fires │ T2 (predicted, immediate)
   authoritative @ T4 ┘ → reconcile: predicted vs confirmed (snap/blend on mismatch)
```

What to watch in Mode B: rubber-banding / snapping on **mispredict**, which worsens with
**jitter and loss** — a model that feels great at clean 80 ms RTT but vomits artifacts at
80 ms ±20 jitter + 2% loss has not actually passed.

## 6. Input-delay sizing

`D = ceil( (RTT/2 + jitter_margin) / tick_period )`, capped so the wait covers the
one-way trip + jitter. At 30 Hz, `tick_period = 33 ms`:

| RTT (ms) | one-way+margin | D (30 Hz) | D (60 Hz) |
|---|---|---|---|
| 0 (LAN)  | ~5 ms   | 1 | 1 |
| 40       | ~30 ms  | 1 | 2 |
| 80       | ~55 ms  | 2 | 4 |
| 120      | ~75 ms  | 3 | 5 |
| 160      | ~95 ms  | 3 | 6 |

Higher tick rate → finer aim granularity but **more ticks of delay** for the same RTT
(more packets in flight) — that tension is exactly the Q8 question, surfaced here.

## 7. Test matrix

Sweep, judging each cell in hand:

- **Mode:** { pure lockstep, avatar-local prediction }
- **Tick:** { 30 Hz, 60 Hz }
- **Injected RTT:** { 0, 40, 80, 120, 160 ms }
- **Jitter / loss:** { clean, ±15 ms / 2% loss } on the realistic RTTs (80–160)

Then a **real-device pass** to validate the injector's numbers aren't lying: two phones on
the **same Wi-Fi** (expect low RTT), and — if reachable — one on **cellular / hotspot** for
a real mobile RTT. The simulated sweep finds the thresholds; the device pass confirms they
match reality.

## 8. How to judge — the readouts that matter

Per cell, capture a quick subjective verdict (playable / mushy / unplayable) plus the
specific **failure signature**, because *which* thing breaks decides Q7 vs Q8:

| Symptom | Points at |
|---|---|
| Aim/fire feels laggy but consistent | input delay too high → prediction (Mode B) or lower RTT budget |
| Hits feel coarse / "between frames" even at 0 ms | tick granularity → Q8 (raise tick / aim-at-render) |
| Mode B snaps/rubber-bands on jitter or loss | reconciliation cost; tune or model can't carry it |
| Stalls/hitches when a peer lags | the lockstep pacing gotcha (expected; note severity) |

The deliverable is the **smallest RTT at which each mode stops feeling like a playable
gunfight**, per tick rate — not a vibe.

## 9. Decision outcomes (how results resolve Q7/Q8)

```
 Does PURE LOCKSTEP feel playable at realistic mobile RTT (≈80–120 ms)?
   ├─ yes ............................ Q7 = pure lockstep + input delay (cheapest; lean was wrong-but-better)
   └─ no → does AVATAR-LOCAL PREDICTION rescue it (incl. jitter/loss)?
            ├─ yes ................... Q7 = avatar-local prediction (current lean confirmed);
            │                          record the reconciliation approach + "never feed prediction
            │                          back into sim state" as a hard rule for Phase 1
            └─ no → does 60 Hz close the gap?
                     ├─ yes .......... Q8 = raise embodied tick (or aim-sampled-at-render-committed-at-tick)
                     └─ no ........... bigger rethink BEFORE Phase 1: rollback on the embodied
                                       layer, or server-arbitrated FPS hits (breaks pure P2P) —
                                       re-open Q7 at the architecture level
```

Whichever branch wins is recorded as a new decision (`Dn`) resolving/advancing Q7 and Q8,
and the architecture doc's "open tension" section is updated to a settled approach.

## 10. Build order

1. Copy prototype → `prototypes/phase0.5-netfeel/`; strip command-layer extras, keep the
   embodied half.
2. Fixed-tick accumulator (30/60 Hz switch) + render interpolation.
3. ENet host/join by IP; two avatars spawned, one per peer.
4. Input command serialize + exchange + scheduling buffer (Mode A working end to end).
5. Latency injector (delay/jitter/loss) + diagnostics HUD.
6. Mode B: predicted local avatar + reconciliation; hot-swap A/B.
7. Fight feedback (hit/down/respawn) so aim outcomes are legible.
8. Run the §7 matrix; then the real-device pass.

## 11. Risks & gotchas

- **LAN is too fast** — without the injector you'll falsely conclude "lockstep feels
  great." Always test through the injected-RTT presets; the 0 ms cell is only a control.
- **A bad reconciliation = false negative** — Mode B looking terrible may be a prediction
  bug, not the model. Keep Mode B minimal and correct; sanity-check at 0 ms (should be
  invisible) before trusting its high-RTT verdict.
- **Floats ≠ proof** — see §2. Don't let the throwaway's non-determinism leak into a claim
  about Phase 1.
- **Two-peer stalls are expected** — the slowest-peer pacing is real lockstep behaviour;
  measure how bad it feels, don't "fix" it by abandoning lockstep semantics.
- **Throwaway** — like Phase 0, delete both prototype dirs once Q7/Q8 are settled.

---

**On completion:** record the outcome as a `Dn` resolving Q7 (and Q8), update
[`architecture.md`](architecture.md) §"Embodied combat over lockstep" from *open tension*
to *settled approach*, mark Phase 0.5 done in [`roadmap.md`](roadmap.md), and only then
start the Phase 1 engine spine.
