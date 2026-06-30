# Content-tooling plan — authoring extensive campaigns & battlefields

> **Status: PLANNED.** Resolves [Q15](../open-questions.md) → [D76](../decisions.md): missions and
> battlefields become **external RON data files** behind a **host-side `engine` loader** that drives a
> new **serde-free `ScenarioBuilder` in `core`**. This plan is the build-out of that decision — the
> authoring infrastructure that turns mission/map creation from an engineer-recompile task into a
> designer-edit-a-file task. Design context: [`pve-campaign.md`](../pve-campaign.md) (the campaign this
> feeds), [`pve-campaign-plan.md`](pve-campaign-plan.md) (the pillar this is the tooling half of).

---

## Why this exists

The engine is systems-complete, end-to-end playable, and already has the **content *substrate***: the
host-side objective system ([D59](../decisions.md), `engine::objectives`), the mission registry
([D58](../decisions.md)–[D61](../decisions.md), `engine::mission_registry`), difficulty + modifiers
(`core::mission_tuning`), and one worked mission (*Seize*). What it does **not** have is a way to
author **volume** — every mission and every battlefield today is **hand-written Rust** in
`core::scenario` (`seed_seize_mission`, `seed_skirmish`, …), so each one is a recompile and demands a
Rust toolchain. "Extensive campaigns and battlefields" is exactly the thing that loop throttles.

[D76](../decisions.md) settles the format question (RON, host-side). This plan builds it. The load-bearing
property to preserve throughout: **the data layer is host-side and the loader is a float airlock**, so
no authored file can leak a float into the sim (invariant #1), drag a dependency into `core` (invariant
#2), or add per-tick checksum surface (invariant #7).

## What this plan is **not**

- **Not the Operations-hub menu.** Presenting the campaign (mission-select/briefing/unlock-graph UI) is
  [`pve-campaign-plan.md`](pve-campaign-plan.md) **WS-B**, [D32](../decisions.md)-blocked on native
  shells. This plan unblocks *authoring* content; WS-B unblocks *presenting* it. They compose but are
  independent.
- **Not the PvP net layer.** A PvP battlefield is the *same* `MapSpec` with two human commanders instead
  of a scripted one — the content format is **mode-agnostic**. PvP's remaining gap is the live transport
  + matchmaking (Phase 3, [`phase-3-plan.md`](phase-3-plan.md)) and [Q17](../open-questions.md) input
  fairness — out of scope here.
- **Not Lua/scripting.** Data, not behaviour ([D76](../decisions.md)). The scripting second pass is
  deferred until a set-piece genuinely needs control flow ([Q16](../open-questions.md)).
- **Not narrative depth.** Light briefings only ([Q16](../open-questions.md)); the format carries the
  briefing text fields and nothing more.

---

## The load-bearing architecture call

**The data layer lives host-side, in `engine`; `core` stays serde-free.** `core` carries no serde
dependency today and must not gain one (invariant #2). So the format is two pieces, mirroring the
objective-system split exactly:

```
   *.mission.ron  ──parse+validate──►  MissionSpec        (engine::mission_format — owns serde/RON)
   *.map.ron      ──parse+validate──►  MapSpec                    │
        │  (float airlock: every number → integer → Fixed)        │  drives, never folds
        │   deny_unknown_fields · range-checked · fails LOUD      ▼
        ▼                                              core::scenario::ScenarioBuilder   (serde-free,
   host-side, NEVER in the per-tick checksum                       │     deterministic, fixed-point)
                                                                   ▼
                                              a seeded Sim  ──►  per-tick checksum  (UNCHANGED by the
                                                                  format; same footing as a hand seeder)
```

- **`core::scenario::ScenarioBuilder`** is a thin typed builder over the spawn/build primitives the
  hand-written seeders already call privately — `spawn(kind, cell, faction, stance)`,
  `build_camp(cell, faction)`, `set_income(period)`, `set_army(faction, army)`, `control_point(cell)`.
  Programmatic, fixed-point, no parser. `seed_seize_mission` is refactored to call it (so it's the
  living oracle), not deleted.
- **`engine::mission_format`** owns the RON dependency and the validation. It is the **only** place a
  text number becomes a sim number, and it does so through `Fixed::from_*` on **integers** — there is
  no `f32`/`f64` in the type graph from file to sim.

This keeps the airlock in one auditable module, `core` clean, and the determinism matrix covering a
data-loaded mission for free (it already covers "a seeded `Sim` is bit-identical").

---

## Workstreams

Each owns a new module, following the repo's "extract a pure testable seam" pattern. Edits to shared
files (`core/scenario.rs`, `engine/mission_registry.rs`) stay small, additive, region-disjoint.

### CT-A — `core::scenario::ScenarioBuilder` *(the spine)*

The serde-free, deterministic builder API over the existing private spawn/build primitives. Refactor
`seed_seize_mission` (and ideally `seed_skirmish`) to build *through* it, proving the API expresses the
missions we already ship before any file format rides on it.

**Tests (same commit):** the builder reproduces `seed_seize_mission`'s `Sim` **byte-identically** (same
opening checksum) — the oracle the whole format leans on; float-free (the determinism guard greps it);
green **dev + release**; `determinism.yml` matrix unchanged.

### CT-B — RON mission format + host-side loader (`engine::mission_format`)

The `MissionSpec` schema (`#[derive(Deserialize)]`, `deny_unknown_fields`) + the **float-airlock**
parser/validator that maps it onto the CT-A builder. Numeric fields are integers (cells; fixed-point
milli-units for HP/rates/distances); the loader range-validates and **fails loudly** on bad input.
A `MissionSpec` carries: scenario params (forces, armies, income, the `ScenarioModifiers` from
`core::mission_tuning`), an objective set (the `ObjectiveKind` vocabulary from `engine::objectives`),
the `Difficulty` tier, the briefing text, and a `map:` reference (CT-C).

**Proof obligation:** ship a `missions/seize.mission.ron` whose loaded `Sim` is **byte-identical** to
CT-A's code-built *Seize* (asserted in a test against the opening checksum). The format is a faithful
re-expression, not a second code path.

**Tests:** schema round-trip; the byte-identical Seize oracle; a battery of **rejection** tests (a
float literal, an unknown field, an out-of-range cell, a dangling entity ref) each fail at load with a
clear error; no-GPU, ships in `cargo test`.

### CT-C — Battlefield/map format (`*.map.ron` → `MapSpec`)

Factor the **spatial half** of a scenario into a reusable `MapSpec`: terrain map-id (the existing
`persist` terrain-by-map-id, [D28](../decisions.md)), control-point positions, cover-prop placements
(crate/tree/rock/barricade/turret, [D50](../decisions.md)), and named spawn zones a `MissionSpec`
populates. A mission references a map by id, so **one battlefield backs many missions** (the
Operations-hub replay model) and the same `MapSpec` serves a PvP skirmish (two human commanders, same
ground).

**Tests:** a map round-trips; cover/control-point placements land at the authored cells; the same map
under two different `MissionSpec`s yields two deterministic (and correctly *different*) `Sim`s; spawn
zones reject overlap/out-of-bounds at load.

### CT-D — Data-backed mission registry + content hot-reload

Give `engine::mission_registry` a path that loads `MissionDef`s from a **content directory** of
`*.mission.ron`/`*.map.ron` instead of hardcoded `MissionDef::new(...)`. The hardcoded
`default_registry()` stays as the test/fallback baseline and the CT-A oracle. Add **between-match
hot-reload** (re-scan + re-validate the content dir on return-to-title) — the primary mitigation for
Rust's weak engine reload ([D10](../decisions.md), [`roadmap.md`](../roadmap.md) dev-workflow scripting
lane).

**Tests:** a registry built from a content dir resolves the same nodes as `default_registry()`;
reload picks up an added/edited file; a malformed file is rejected without taking the registry down.

### CT-E — Complete the objective archetype vocabulary in the format

The `ObjectiveKind` enum already models Capture / Eliminate / Survive / Reach / Escort
([D59](../decisions.md)); wire the remaining **mission archetypes** from
[`pve-campaign.md`](../pve-campaign.md) §3 (Hold, Push, Assassinate/Extract) as authorable objective
sets in the format — composition of existing evaluators, **no new sim**. This is what lets the format
express the full archetype list rather than just *Seize*.

**Tests:** each archetype's objective set drives to both a win and a loss against a synthetic
`SimEvent` stream; an authored `*.mission.ron` per archetype loads + evaluates correctly.

### CT-F — Content-lint harness (`pnpm content:check`)

A headless, no-GPU `cargo`-test + script that loads **every** shipped `*.mission.ron`/`*.map.ron`,
asserting: schema-valid, float-free, builds a deterministic `Sim`, all entity/map references resolve,
objective targets exist in the seeded world. CI-able (no GPU) — the standing guard that authored
content can never silently break determinism or dangle a reference. Mirrors the
`viz-runner`/`--metrics` "verify it *plays*, not just *computes*" discipline at the content layer.

**Tests:** the harness itself round-trips on the shipped content; a deliberately-broken fixture fails
the lint with a precise diagnostic.

### CT-G — Procedural map generator + PvP-symmetry validator

The **"generate at volume"** deliverable — the script-not-binary content ethos ([D41](../decisions.md)/[D46](../decisions.md))
applied to battlefields. A **seed-deterministic** generator emits valid `*.map.ron` files from a seed +
parameters (size, cover density, control-point count, symmetry mode): scattered cover/props, and
control-point + spawn-zone placement. Output is **git-diffable text** run through the CT-F lint, so
generated maps are regenerable *and* self-verifying; **same seed → identical map** (the generator uses
its own RNG — it never touches the sim). This is what makes "author extensive battlefields" a scripted
content task rather than hand-work — and it is squarely buildable: producing and *linting* maps at
volume needs no human in the loop, only `cargo`/script + CT-F.

- **PvP-symmetry validator.** For competitive maps, assert **mirror/rotational fairness**: spawns,
  control points, and cover are symmetric under the map's declared symmetry (point or mirror), so
  neither side has a structural edge — pillar 4 fairness and the LEAD-protection *symmetric*-PvP shape
  ([`positioning.md`](../positioning/positioning.md) §3). Extends the CT-F lint with a `--pvp` check.
- **Terrain generation — decided: content-addressed ([D77](../decisions.md), resolves [Q22](../open-questions.md)).**
  Cover, control points, and spawns are pure *placement* data the generator writes freely over an
  existing terrain. *Novel terrain shape itself* was the one real fork — `core::terrain` is a `MapId`
  (`u16`) **registry** (`Terrain::from_map_id` rebuilds the cover/LoS grid; the reconnect snapshot
  serializes only the map-id, **not** the grid — [D28](../decisions.md)). [D77](../decisions.md) locks
  **content-addressed terrain**: the map carries its fixed-point cover grid as data, `MapId` widens from
  a registry index to a **content-hash digest** of the grid's canonical bytes, and `persist`/reconnect
  keeps serializing **only the id** (the peer rebuilds the grid from the shared content set). So CT-G
  emits a generated terrain grid **plus its content hash**, the CT-F lint verifies the hash matches the
  grid, and a missing/mismatched id is a hard match-setup failure (never a desync). **The placement half
  needs none of this** (generate over an existing terrain id); the terrain half now **builds against
  D77** rather than being gated on an open question.

**Tests:** same seed → **byte-identical** `*.map.ron`; every generated map passes CT-F; the symmetry
validator **rejects** a deliberately-asymmetric fixture and **accepts** a mirrored one; a batch of
generated maps each build a deterministic `Sim`. No GPU; ships in `cargo test` + the content-lint
script.

---

## Sequencing

```
   CT-A (builder) ──► CT-B (mission format) ──► CT-D (registry + hot-reload) ──► (author at volume)
                          │                          ▲
                          ├─► CT-C (map format) ──┬──┘   (battlefields; reused across missions + PvP)
                          │                       └─► CT-G (procedural generator + PvP-symmetry; content-addressed terrain per D77)
                          ├─► CT-E (archetypes)          (full verb vocabulary in data)
                          └─► CT-F (content lint)        (CI guard; CT-G extends it with the --pvp check)
```

CT-A is the spine and the natural first slice — prove the builder re-expresses the missions we already
ship before any file format rides on it. CT-B is the airlock; CT-C/CT-E/CT-F layer on. CT-D is the
payoff (designers author without a recompile); **CT-G is the volume multiplier** — once the `*.map.ron`
format exists, a scripted generator sprays battlefields and the lint proves each one valid. After this
lands, *extensive* PvE campaign content is a **design** task; *extensive* battlefields are
generated-and-linted `*.map.ron` artifacts that **also** feed PvP (symmetry-checked) the moment its net
layer exists.

## Determinism & fairness guardrails (apply to every WS)

- **Data layer host-side in `engine`; `core` stays serde-free** (invariant #2) — the loader, not the
  sim, owns RON.
- **The loader is the float airlock** — every numeric field is an integer → `Fixed`; **no `f32`/`f64`
  path from file to sim** (invariant #1). The determinism guard greps the loader.
- **`deny_unknown_fields` + range-validated + fail-loud at load** — a bad file errors host-side, never
  silently desyncs.
- **The data file never enters the checksum** — only the seeded `Sim` does, exactly as a hand-written
  seeder does today (invariant #7); the cross-arch matrix needs no new coverage for the format itself.
- **Byte-identical oracle** — the data-loaded *Seize* must match the code-built `seed_seize_mission`'s
  opening checksum, proving the format is a faithful re-expression.
- **Modifiers reshape the situation, never the balance numbers** ([D30](../decisions.md)) — the format
  exposes `ScenarioModifiers`, not combat/economy tuning constants.
- **The generator is offline tooling, not sim code** (CT-G) — it has its **own** seeded RNG, emits a
  `*.map.ron`, and never touches `core`/the sim; its output is just another data file through the same
  airlock + lint. Same seed → byte-identical map (regenerable, git-diffable).

## Verification

- Per WS: unit/integration tests **in the same commit**, green `cargo test` **dev + release** (the
  CLAUDE.md floor); the determinism + 2-peer lockstep runners agree; run [`/check`](../../.claude/commands)
  before each non-trivial commit, and [`/safe-edit`](../../.claude/commands) for **CT-A** (it touches the
  `core` seeding path).
- Content-level: **CT-F** is the standing guard — every shipped mission/map loads, lints float-free, and
  builds a deterministic `Sim` with all references resolved, in CI.
- End-to-end: a designer authors a new `*.mission.ron` against an existing `*.map.ron`, hot-reloads it,
  and plays it start→finish on desktop — **no recompile**. That round-trip *is* the acceptance test for
  the pillar.
