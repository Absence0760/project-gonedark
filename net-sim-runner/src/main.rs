//! Headless 2-peer lockstep driver (invariant #7, docs/phase-3-plan.md §"Workstream B"
//! step 2, D27).
//!
//! Drives **two** in-process `core::lockstep::Lockstep` instances — peer 0 issues the
//! "player" commands, peer 1 the "enemy" commands — over a **deterministic** in-process
//! channel (a single seeded `core::rng::Rng` drives loss + jitter, so the run is itself
//! bit-identical across arch, with no sockets). Each peer owns its own `Sim` and steps it
//! with the merged command set `Lockstep::try_advance` hands back (peers merged in fixed
//! peer order — the same stable application order `Sim::step` relies on).
//!
//! Every tick it **asserts** both peers agree on the per-tick checksum, and that the agreed
//! stream equals a no-network single-`Sim` reference applying the same merged commands. Any
//! disagreement is a cross-client desync — a real bug (invariant #7) — so it prints a
//! `::error::` line to stderr and `exit(1)` rather than emitting a wrong stream.
//!
//! The agreed `<tick> <checksum>` stream goes to **stdout** so CI can diff it across arch
//! (a separately-named `net-checksums-<target>.txt` artifact — never dumped into the
//! existing `checksums-*.txt` glob, whose `compare` job requires all entries identical).
//!
//! Usage: `gonedark-net-sim-runner [ticks] [delay]`  (defaults: 300 ticks, delay 2).
//! Everything is integer/fixed-point and spawn/merge order is stable, so it is float-free
//! and deterministic — the determinism guard greps this crate's tests too (no `f32`/`f64`).

use gonedark_core::components::{BuildingKind, EntityKind, Faction, Order, Stance, UnitKind, Vec2};
use gonedark_core::economy::{self, Resources};
use gonedark_core::ecs::Entity;
use gonedark_core::fixed::Fixed;
use gonedark_core::lockstep::{Lockstep, PeerId};
use gonedark_core::rng::Rng;
use gonedark_core::sim::{Command, Sim};
use gonedark_core::territory::ControlPoint;

/// Scene seed — fixed so every peer (and the reference) builds the identical world.
const SCENE_SEED: u64 = 0x9E3779B97F4A7C15;
/// Channel seed — fixed so the loss/jitter draw sequence is itself deterministic.
const NET_SEED: u64 = 0xD1CE_F00D_BAAD_F00D;

fn fx(n: i32) -> Fixed {
    Fixed::from_int(n)
}

fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(fx(x), fx(y))
}

/// Spawn a Rifleman of `faction` at `(x, y)`, set to engage at will, and return its handle.
fn spawn_rifleman(sim: &mut Sim, x: i32, y: i32, faction: Faction) -> Entity {
    let (health, weapon) = economy::unit_stats(UnitKind::Rifleman);
    let e = sim.world.spawn();
    let i = e.index as usize;
    sim.world.kind[i] = EntityKind::Unit;
    sim.world.faction[i] = faction;
    sim.world.pos[i] = v(x, y);
    sim.world.health[i] = health;
    sim.world.weapon[i] = weapon;
    sim.world.stance[i] = Stance::FireAtWill;
    e
}

/// Stable handles into the shared scene. Spawn order is fixed, so these are bit-identical
/// across every `Sim` built by [`scene`].
struct Handles {
    p: [Entity; 3],
    e: [Entity; 3],
    camp: Entity,
}

/// Build the shared two-faction scene into `sim` and return its handles. A small phase2-like
/// scene split across two sides: three player riflemen + a player camp (peer 0 commands),
/// three enemy riflemen (peer 1 commands), one contested control point.
fn scene(sim: &mut Sim) -> Handles {
    sim.resources = Resources::new(100_000);
    sim.territory.points.push(ControlPoint::neutral(Vec2::ZERO));
    let p = [
        spawn_rifleman(sim, -5, 0, Faction::Player),
        spawn_rifleman(sim, -5, 3, Faction::Player),
        spawn_rifleman(sim, -6, 1, Faction::Player),
    ];
    let e = [
        spawn_rifleman(sim, 5, 0, Faction::Enemy),
        spawn_rifleman(sim, 5, 3, Faction::Enemy),
        spawn_rifleman(sim, 6, 1, Faction::Enemy),
    ];
    let camp = economy::build(
        &mut sim.world,
        &mut sim.resources,
        Faction::Player,
        BuildingKind::Camp,
        v(-20, 20),
    )
    .expect("camp affordable at 100k resources");
    Handles { p, e, camp }
}

/// The scripted command set peer `peer` submits for execution tick `t` (with input `delay`).
///
/// Peer 0 drives the player units, peer 1 the enemy — the split that makes the lockstep
/// per-tick **merge in fixed peer order** genuinely load-bearing (commands really do come
/// from both sides). Exercises a spread of the `Command` vocabulary across two active ticks;
/// quiet otherwise. First inputs land at `t == delay` (ticks `[0, delay)` are warmup).
fn script(h: &Handles, peer: PeerId, t: u64, delay: u64) -> Vec<Command> {
    if t == delay {
        match peer {
            0 => vec![
                Command::AttackMove {
                    entity: h.p[0],
                    target: v(5, 0),
                },
                Command::SetOrder {
                    entity: h.p[1],
                    order: Order::Patrol {
                        a: v(-5, 3),
                        b: v(-5, -8),
                        toward_b: true,
                    },
                },
                Command::SetStance {
                    entity: h.p[2],
                    stance: Stance::FireAtWill,
                },
                Command::SetRetreatThreshold {
                    entity: h.p[1],
                    fraction: Fixed::from_ratio(1, 3),
                },
                Command::Embody { entity: h.p[0] },
                Command::QueueProduction {
                    camp: h.camp,
                    unit: UnitKind::Rifleman,
                },
            ],
            _ => vec![
                Command::AttackMove {
                    entity: h.e[0],
                    target: v(-5, 0),
                },
                Command::SetStance {
                    entity: h.e[1],
                    stance: Stance::HoldFire,
                },
                Command::Move {
                    entity: h.e[2],
                    target: v(0, 5),
                },
            ],
        }
    } else if t == delay + 25 {
        match peer {
            0 => vec![Command::Surface { entity: h.p[0] }],
            _ => vec![Command::SetOrder {
                entity: h.e[0],
                order: Order::FallBack(v(8, 8)),
            }],
        }
    } else {
        Vec::new()
    }
}

/// A deterministic in-process channel. A single seeded RNG drives per-frame loss and jitter;
/// the variable per-frame delay produces reordering. No sockets — and because the draw
/// sequence is seeded, the whole transport is bit-identical across arch.
struct Net {
    rng: Rng,
    base_delay: u64,
    jitter: u32,
    loss_num: u32,
    loss_den: u32,
    inflight: Vec<(u64, PeerId, Vec<u8>)>, // (deliver-at iteration, recipient, bytes)
}

impl Net {
    fn new(seed: u64, base_delay: u64, jitter: u32, loss_num: u32, loss_den: u32) -> Self {
        Net {
            rng: Rng::new(seed),
            base_delay,
            jitter,
            loss_num,
            loss_den,
            inflight: Vec::new(),
        }
    }

    fn send(&mut self, now: u64, to: PeerId, frames: Vec<Vec<u8>>) {
        for bytes in frames {
            if self.loss_den > 0 && self.rng.below(self.loss_den) < self.loss_num {
                continue; // dropped this round (a later resend will carry it)
            }
            let jit = if self.jitter > 0 {
                self.rng.below(self.jitter + 1) as u64
            } else {
                0
            };
            self.inflight.push((now + self.base_delay + jit, to, bytes));
        }
    }

    fn deliver_due(&mut self, now: u64, sessions: &mut [Lockstep]) {
        let drained = std::mem::take(&mut self.inflight);
        for (due, to, bytes) in drained {
            if due <= now {
                sessions[to as usize]
                    .deliver(&bytes)
                    .expect("well-formed frame from the deterministic channel");
            } else {
                self.inflight.push((due, to, bytes));
            }
        }
    }
}

/// Knobs for the run, so tests can vary delay/channel while `main` uses sane defaults.
#[derive(Clone, Copy)]
struct Config {
    ticks: u64,
    delay: u64,
    base_delay: u64,
    jitter: u32,
    loss_num: u32,
    loss_den: u32,
    net_seed: u64,
}

impl Config {
    /// Default channel for `main`: a touch of latency + jitter + light loss so the gate,
    /// reorder tolerance, and loss-tolerant resend window are all exercised, while staying
    /// fast to converge. Still fully deterministic (seeded).
    fn for_run(ticks: u64, delay: u64) -> Self {
        Config {
            ticks,
            delay,
            base_delay: 1,
            jitter: 2,
            loss_num: 1,
            loss_den: 6,
            net_seed: NET_SEED,
        }
    }
}

/// The agreed stream and a flag for whether the two peers and the reference all matched.
struct Outcome {
    stream: Vec<u64>,
    agreed: bool,
}

/// Run the 2-peer lockstep session and the no-network reference in lockstep, asserting
/// per-tick agreement as it goes. Returns the agreed checksum stream (one entry per executed
/// tick `0..ticks`). `agreed` is false on the first divergence (and the stream stops there).
fn run(cfg: Config) -> Outcome {
    let ticks = cfg.ticks;
    let delay = cfg.delay;
    debug_assert!(ticks > delay, "ticks must exceed the input delay");

    // Two peer sims + a no-network reference, all built from the identical seeded scene.
    let mut sims = [Sim::new(SCENE_SEED), Sim::new(SCENE_SEED)];
    let h = scene(&mut sims[0]);
    let _ = scene(&mut sims[1]); // identical handles by determinism

    let mut refsim = Sim::new(SCENE_SEED);
    let _ = scene(&mut refsim);

    // Reference stream: one sim fed the merged (peer-0 then peer-1) command set each tick.
    let mut refsums = Vec::with_capacity(ticks as usize);
    for t in 0..ticks {
        let mut merged = Vec::new();
        if t >= delay {
            merged.extend(script(&h, 0, t, delay));
            merged.extend(script(&h, 1, t, delay));
        }
        refsim.step(&merged);
        refsums.push(refsim.checksum());
    }

    // Both peers submit their full script up front (the host would submit once per tick;
    // submitting ahead is equivalent and keeps the pump loop purely about delivery/advance).
    let mut sessions = [Lockstep::new(2, 0, delay), Lockstep::new(2, 1, delay)];
    for k in 0..(ticks - delay) {
        let t = delay + k;
        sessions[0].submit(script(&h, 0, t, delay));
        sessions[1].submit(script(&h, 1, t, delay));
    }

    let mut net = Net::new(
        cfg.net_seed,
        cfg.base_delay,
        cfg.jitter,
        cfg.loss_num,
        cfg.loss_den,
    );
    let mut sums: [Vec<u64>; 2] = [
        Vec::with_capacity(ticks as usize),
        Vec::with_capacity(ticks as usize),
    ];

    let mut it = 0u64;
    loop {
        let f0 = sessions[0].drain_outbound();
        net.send(it, 1, f0);
        let f1 = sessions[1].drain_outbound();
        net.send(it, 0, f1);
        net.deliver_due(it, &mut sessions);

        for i in 0..2 {
            while let Some(cmds) = sessions[i].try_advance() {
                sims[i].step(&cmds);
                sums[i].push(sims[i].checksum());
            }
        }
        if sessions[0].next_tick() >= ticks && sessions[1].next_tick() >= ticks {
            break;
        }
        it += 1;
        // A clean/light channel must converge well within this bound; failing to is itself a
        // (different) bug worth surfacing loudly rather than hanging CI forever.
        if it >= 10_000_000 {
            eprintln!(
                "::error::lockstep failed to converge (delay={delay}, loss={}/{}) — \
                 not a clean desync but a stall; treat as a real bug (invariant #7)",
                cfg.loss_num, cfg.loss_den
            );
            std::process::exit(1);
        }
    }

    // Both peers ran exactly `ticks` ticks (0..ticks). Assert agreement at every one, and
    // against the reference. The agreed stream is peer 0's (== peer 1's once checked).
    let n = ticks as usize;
    for t in 0..n {
        let a = sums[0][t];
        let b = sums[1][t];
        let r = refsums[t];
        if a != b || a != r {
            eprintln!(
                "::error::cross-client desync at tick {t}: peer0={a:016x} peer1={b:016x} \
                 reference={r:016x} (delay={delay}) — a real lockstep bug (invariant #7)"
            );
            return Outcome {
                stream: sums[0][..t].to_vec(),
                agreed: false,
            };
        }
    }

    Outcome {
        stream: sums[0].clone(),
        agreed: true,
    }
}

fn emit(tick: u64, checksum: u64) {
    println!("{tick} {checksum:016x}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ticks: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(300);
    let delay: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2);

    if ticks <= delay {
        eprintln!("::error::ticks ({ticks}) must exceed delay ({delay})");
        std::process::exit(2);
    }

    let outcome = run(Config::for_run(ticks, delay));
    for (t, &c) in outcome.stream.iter().enumerate() {
        emit(t as u64, c);
    }
    if !outcome.agreed {
        // The desync detail was already printed to stderr by `run`.
        std::process::exit(1);
    }
    eprintln!(
        "ok: 2-peer lockstep agreed with the no-network reference over {ticks} ticks \
         (delay {delay})"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(ticks: u64, delay: u64) -> Config {
        Config::for_run(ticks, delay)
    }

    /// The full agreed stream, asserting the run actually agreed (so a test that wanted the
    /// stream never silently gets a truncated desync prefix).
    fn agreed_stream(cfg: Config) -> Vec<u64> {
        let o = run(cfg);
        assert!(o.agreed, "run did not agree across peers/reference");
        o.stream
    }

    #[test]
    fn stream_is_deterministic() {
        // The whole run — scene, scripted commands, seeded channel — built twice is identical.
        // This is the property the cross-arch CI diff relies on, asserted here on one arch.
        assert_eq!(agreed_stream(cfg(300, 2)), agreed_stream(cfg(300, 2)));
    }

    #[test]
    fn peers_agree_and_match_reference() {
        // `run` asserts both peers agree with the no-network reference every tick; a clean
        // return with `agreed` means that held for the whole run.
        let o = run(cfg(300, 2));
        assert!(o.agreed, "peers must agree with the reference every tick");
        assert_eq!(o.stream.len(), 300, "one checksum per executed tick");
    }

    #[test]
    fn equals_no_network_reference_directly() {
        // Independently rebuild the no-network reference here and confirm the agreed stream
        // equals it — a direct check that the network path adds no divergence of its own.
        let delay = 2u64;
        let ticks = 200u64;
        let stream = agreed_stream(cfg(ticks, delay));

        let mut refsim = Sim::new(SCENE_SEED);
        let h = scene(&mut refsim);
        let mut refsums = Vec::with_capacity(ticks as usize);
        for t in 0..ticks {
            let mut merged = Vec::new();
            if t >= delay {
                merged.extend(script(&h, 0, t, delay));
                merged.extend(script(&h, 1, t, delay));
            }
            refsim.step(&merged);
            refsums.push(refsim.checksum());
        }
        assert_eq!(stream, refsums);
    }

    #[test]
    fn agrees_across_several_delays() {
        // The merge/gate is delay-independent: peers + reference agree at a spread of delays.
        for delay in [1u64, 2, 5, 8] {
            let o = run(cfg(120, delay));
            assert!(o.agreed, "desync at delay {delay}");
            assert_eq!(o.stream.len(), 120);
        }
    }

    #[test]
    fn merge_is_genuinely_two_sided() {
        // Guard against a regression where the scene/script accidentally lands all commands on
        // one peer (which would make the fixed-peer-order merge untested). Both peers' first
        // post-warmup submit must be non-empty.
        let delay = 2u64;
        let h_sim = &mut Sim::new(SCENE_SEED);
        let h = scene(h_sim);
        assert!(
            !script(&h, 0, delay, delay).is_empty(),
            "peer 0 issues commands"
        );
        assert!(
            !script(&h, 1, delay, delay).is_empty(),
            "peer 1 issues commands"
        );
    }

    #[test]
    fn stream_evolves_not_frozen() {
        // The checksum must change as the sim advances — proof the run does real work, not a
        // frozen world that would make "agreement" trivially true.
        let stream = agreed_stream(cfg(100, 2));
        assert_ne!(
            stream[0],
            stream[stream.len() - 1],
            "checksum should evolve"
        );
    }
}
