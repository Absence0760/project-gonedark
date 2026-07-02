//! The bundled replay **scenarios** — a seed + a builder + a scripted per-tick command stream.
//!
//! A replay is identified by its [`Scenario`] tag plus the RNG `seed`. Playback re-runs the
//! builder against the same seed to reconstruct the *seeded world* (spawns, bases, terrain — the
//! parts that are not commands), then feeds back the recorded command log. So the scenario builder
//! must be **pure and deterministic**: same tag + same seed ⇒ byte-identical starting world. Both
//! bundled scenarios reuse a real `core::scenario` seeder so they exercise the shipping seed paths.

use std::collections::BTreeMap;

use gonedark_core::components::{Order, Stance, UnitKind, Vec2};
use gonedark_core::fixed::Fixed;
use gonedark_core::lockstep::PeerId;
use gonedark_core::scenario;
use gonedark_core::sim::{Command, Sim};

/// Which bundled scenario a replay was recorded against. The `u8` tag is stored in the artifact
/// header so playback dispatches to the identical builder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    /// The two-base skirmish ([`scenario::seed_skirmish`]) driven by a scripted command stream:
    /// the Player troop attack-moves, the base produces, the troop is embodied and fires/moves in
    /// first person, then surfaces — a non-trivial mix of Move / production / Fire / embodiment
    /// commands so the recorded log is meaningful, not an idle sim.
    SkirmishScript,
}

impl Scenario {
    /// The default (and currently only) scenario.
    pub const DEFAULT: Scenario = Scenario::SkirmishScript;

    pub fn tag(self) -> u8 {
        match self {
            Scenario::SkirmishScript => 0,
        }
    }

    pub fn from_tag(tag: u8) -> Option<Scenario> {
        match tag {
            0 => Some(Scenario::SkirmishScript),
            _ => None,
        }
    }

    /// Parse the CLI scenario token (round-trips with [`Scenario::token`]).
    pub fn parse(token: &str) -> Option<Scenario> {
        match token {
            "skirmish" => Some(Scenario::SkirmishScript),
            _ => None,
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Scenario::SkirmishScript => "skirmish",
        }
    }
}

/// A built scenario: the seeded sim plus the scripted commands keyed by the tick they execute on.
/// A `BTreeMap` keeps iteration deterministic; commands within a tick keep insertion order (the
/// stable application order `Sim::step` relies on). This is the SAME shape sim-runner uses.
pub struct Built {
    pub sim: Sim,
    pub scripted: BTreeMap<u64, Vec<Command>>,
}

impl Built {
    pub fn commands_for(&self, tick: u64) -> &[Command] {
        self.scripted.get(&tick).map(Vec::as_slice).unwrap_or(&[])
    }
}

fn fx(n: i32) -> Fixed {
    Fixed::from_int(n)
}
fn v(x: i32, y: i32) -> Vec2 {
    Vec2::new(fx(x), fx(y))
}

/// Build `scenario` seeded from `seed`. Deterministic: same args ⇒ byte-identical world + script.
pub fn build(scenario: Scenario, seed: u64) -> Built {
    match scenario {
        Scenario::SkirmishScript => build_skirmish(seed),
    }
}

fn build_skirmish(seed: u64) -> Built {
    let mut sim = Sim::new(seed);
    // Reuse the real skirmish seeder (invariant #2: shared core seed path). It sets armies, posts,
    // both operational bases, and one starting troop per side, and hands back their handles.
    let sk = scenario::seed_skirmish(&mut sim);

    let mut scripted: BTreeMap<u64, Vec<Command>> = BTreeMap::new();

    // Tick 1: the command layer opens — the Player troop attack-moves toward the enemy base, the
    // enemy troop is put on a short patrol (order-system coverage), and the Player base queues a
    // unit that finishes mid-run (production coverage).
    scripted.insert(
        1,
        vec![
            Command::AttackMove {
                entity: sk.player_troop,
                target: v(30, 0),
            },
            Command::SetOrder {
                entity: sk.enemy_troop,
                order: Order::Patrol {
                    a: v(30, 0),
                    b: v(30, 8),
                    toward_b: true,
                },
            },
            Command::QueueProduction {
                camp: sk.player_base,
                unit: UnitKind::Rifleman,
            },
        ],
    );

    // Tick 40: possess the Player troop — go dark (invariant #5). From here the troop is driven by
    // live-player intents, not orders, so the next several ticks exercise the embodied command path.
    scripted.insert(40, vec![Command::Embody { entity: sk.player_troop }]);

    // Ticks 41..=70: an embodied burst — crouch, then walk (+X) and fire (+X) on cadence. These are
    // exactly the intents the FPS host emits per tick; each rides the recorded log like any order.
    scripted.insert(
        41,
        vec![Command::Crouch {
            entity: sk.player_troop,
            crouched: true,
        }],
    );
    for t in 42..=70u64 {
        let mut cmds = vec![Command::Locomote {
            entity: sk.player_troop,
            dir: v(1, 0),
        }];
        // Fire every third tick so shots space out past the weapon cooldown.
        if t % 3 == 0 {
            cmds.push(Command::Fire {
                entity: sk.player_troop,
                dir: v(1, 0),
            });
        }
        scripted.insert(t, cmds);
    }

    // Tick 90: a second production order, and change the enemy troop's stance (stance coverage).
    scripted.insert(
        90,
        vec![
            Command::QueueProduction {
                camp: sk.player_base,
                unit: UnitKind::Rifleman,
            },
            Command::SetStance {
                entity: sk.enemy_troop,
                stance: Stance::FireAtWill,
            },
        ],
    );

    // Tick 120: surface — eject back to command and resume order control (Move coverage).
    scripted.insert(
        120,
        vec![
            Command::Surface { entity: sk.player_troop },
            Command::Move {
                entity: sk.player_troop,
                target: v(-10, 4),
            },
        ],
    );

    Built { sim, scripted }
}

// ===========================================================================
// Multi-peer scenarios — a lockstep PvP match's per-peer command log.
// ===========================================================================

/// The peer count of the bundled multi-peer skirmish: two commanders (peer 0 the Player side,
/// peer 1 the Enemy side), exactly the 2-client lockstep session `core::lockstep`'s headline test
/// drives.
pub const SKIRMISH_PEER_COUNT: u32 = 2;

/// A built multi-peer scenario: the seeded sim plus the scripted commands keyed **first by the
/// tick they execute on, then by the peer that issued them**.
///
/// This is the identical shape a live [`Lockstep`](gonedark_core::lockstep::Lockstep) session
/// buffers per tick (`tick -> [per-peer command set]`). The load-bearing rule — matching lockstep
/// exactly — is that a tick's executed set is every peer's commands **concatenated in ascending
/// peer order**. Both levels are `BTreeMap`s, so iteration is by key: the merge is deterministic
/// regardless of the order the per-peer sets were recorded/inserted in (the whole point of
/// multi-peer replay ordering — see [`MultiReplay::merged_for`](crate::MultiReplay::merged_for)).
pub struct BuiltMulti {
    pub sim: Sim,
    pub peer_count: u32,
    pub scripted: BTreeMap<u64, BTreeMap<PeerId, Vec<Command>>>,
}

/// Build the multi-peer form of `scenario` seeded from `seed`. Deterministic: same args ⇒
/// byte-identical world + per-peer script.
pub fn build_multi(scenario: Scenario, seed: u64) -> BuiltMulti {
    match scenario {
        Scenario::SkirmishScript => build_skirmish_multi(seed),
    }
}

/// Push `cmd` onto peer `peer`'s command set for `tick`, creating the tick/peer slots on demand.
fn push(
    scripted: &mut BTreeMap<u64, BTreeMap<PeerId, Vec<Command>>>,
    tick: u64,
    peer: PeerId,
    cmd: Command,
) {
    scripted
        .entry(tick)
        .or_default()
        .entry(peer)
        .or_default()
        .push(cmd);
}

/// The two-commander skirmish: the SAME seeded world as [`build_skirmish`], but the commands are
/// split across two peers exactly as a real 2-client match would issue them — peer 0 drives the
/// Player troop + base (attack-move, production, embodiment burst, surface), peer 1 drives the
/// Enemy troop (patrol, stance). Each tick's executed set is the two peers' commands merged in
/// ascending peer order, so this exercises the full multi-peer record → order → playback path.
fn build_skirmish_multi(seed: u64) -> BuiltMulti {
    let mut sim = Sim::new(seed);
    // Same shared seeder as the single-peer scenario (invariant #2): identical starting world.
    let sk = scenario::seed_skirmish(&mut sim);

    let mut scripted: BTreeMap<u64, BTreeMap<PeerId, Vec<Command>>> = BTreeMap::new();

    // ---- Peer 0: the Player commander ----
    // Tick 1: the Player troop attack-moves toward the enemy base and the base queues a unit.
    push(
        &mut scripted,
        1,
        0,
        Command::AttackMove {
            entity: sk.player_troop,
            target: v(30, 0),
        },
    );
    push(
        &mut scripted,
        1,
        0,
        Command::QueueProduction {
            camp: sk.player_base,
            unit: UnitKind::Rifleman,
        },
    );
    // Tick 40: possess the Player troop — go dark (invariant #5).
    push(&mut scripted, 40, 0, Command::Embody { entity: sk.player_troop });
    // Tick 41: crouch, then walk (+X) and fire (+X) on cadence — the embodied FPS intents.
    push(
        &mut scripted,
        41,
        0,
        Command::Crouch {
            entity: sk.player_troop,
            crouched: true,
        },
    );
    for t in 42..=70u64 {
        push(
            &mut scripted,
            t,
            0,
            Command::Locomote {
                entity: sk.player_troop,
                dir: v(1, 0),
            },
        );
        if t % 3 == 0 {
            push(
                &mut scripted,
                t,
                0,
                Command::Fire {
                    entity: sk.player_troop,
                    dir: v(1, 0),
                },
            );
        }
    }
    // Tick 90: a second production order.
    push(
        &mut scripted,
        90,
        0,
        Command::QueueProduction {
            camp: sk.player_base,
            unit: UnitKind::Rifleman,
        },
    );
    // Tick 120: surface and resume order control.
    push(&mut scripted, 120, 0, Command::Surface { entity: sk.player_troop });
    push(
        &mut scripted,
        120,
        0,
        Command::Move {
            entity: sk.player_troop,
            target: v(-10, 4),
        },
    );

    // ---- Peer 1: the Enemy commander ----
    // Tick 1: put the enemy troop on a short patrol (order-system coverage). This is a peer-1
    // command on the SAME tick peer 0 issues its attack-move — the case multi-peer ordering must
    // resolve deterministically (peer 0's set first, then peer 1's).
    push(
        &mut scripted,
        1,
        1,
        Command::SetOrder {
            entity: sk.enemy_troop,
            order: Order::Patrol {
                a: v(30, 0),
                b: v(30, 8),
                toward_b: true,
            },
        },
    );
    // Tick 90: change the enemy troop's stance (again a shared tick with peer 0).
    push(
        &mut scripted,
        90,
        1,
        Command::SetStance {
            entity: sk.enemy_troop,
            stance: Stance::FireAtWill,
        },
    );

    BuiltMulti {
        sim,
        peer_count: SKIRMISH_PEER_COUNT,
        scripted,
    }
}
