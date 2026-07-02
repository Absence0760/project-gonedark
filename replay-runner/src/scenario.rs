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
