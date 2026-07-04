//! D79 cross-language mirror parity — the Rust source-of-truth half.
//!
//! The Android Compose shell hand-mirrors small slices of Rust engine data (the **D79 mirror**
//! pattern): the campaign graph (`engine::mission_registry::default_campaign()` ↔
//! `CampaignModel.kt`) and the battlefield table (`engine::map_library::BATTLEFIELDS` ↔
//! `Battlefield.kt`). Those two sides live in separate build systems (cargo vs gradle), so no
//! single unit test can see both — and they have drifted **silently** in the past (the Kotlin
//! campaign lagged at 12 vs 15 nodes; `Battlefield.kt` was missing library maps).
//!
//! This test closes that gap with **one committed canonical fixture** as the shared contract:
//!
//!   1. This Rust test derives the *mirror-relevant facts* (counts, ids, gating, scene tokens,
//!      conflict years/names, map ids) from the **live** Rust data and asserts they match the
//!      committed fixture `parity/d79-mirror.txt` — so the fixture can never go stale behind the
//!      Rust source (a campaign/map edit that isn't re-blessed fails HERE).
//!   2. The Kotlin `MirrorParityTest` reads the **same** committed fixture and asserts its own
//!      mirror matches it — so a Rust change that isn't hand-mirrored into Kotlin fails THERE.
//!
//! One source of truth, checked from both languages. Regenerate the fixture after an intentional
//! campaign/map change with:
//!
//! ```text
//! UPDATE_D79_FIXTURE=1 cargo test -p gonedark-engine --test d79_mirror_parity
//! ```
//!
//! ## Scope — minimal, stable facts only (NOT prose)
//!
//! The fixture pins only the *structural* facts a mirror must agree on: node/conflict/operation
//! counts, ordered node ids + gating (prerequisites) + scene tokens + operation grouping, conflict
//! names + years, operation names + conflict links, and the ordered battlefield table (id + how it
//! boots) with the derived map-id set. It deliberately does **not** pin briefing/summary/blurb
//! *prose* — that copy drifts for good editorial reasons and is pinned verbatim by the existing
//! per-node tests on each side, not here. Keep this list minimal when adding a new mirror surface.
//!
//! This is host-side *presentation metadata*, never sim state — it reads `default_campaign()` /
//! `BATTLEFIELDS` (both GPU-free, checksum-free) and folds nothing into the sim (invariants #1/#7).

use gonedark_core::campaign::{MissionId, NodeId};
use gonedark_engine::map_library::{BattlefieldKind, BATTLEFIELDS};
use gonedark_engine::mission_registry::default_campaign;
use gonedark_engine::Scene;

use std::path::PathBuf;

/// Header written at the top of the fixture — documents that it is generated, and how to re-bless.
const FIXTURE_HEADER: &str = "\
# D79 Rust<->Android mirror parity fixture — GENERATED, do not hand-edit.
# Regenerate after an intentional campaign/map change:
#   UPDATE_D79_FIXTURE=1 cargo test -p gonedark-engine --test d79_mirror_parity
# The Rust half (engine/tests/d79_mirror_parity.rs) pins the live Rust data to this file;
# the Kotlin half (android/.../MirrorParityTest.kt) pins the Android mirror to the same file.
";

/// The shell scene token a campaign node's mission maps to — the exact `sceneToken` string the
/// Android `campaignNodes` mirror carries (`mission1`/`mission2`/`mission3`), via the same
/// [`Scene::for_mission`] seam the shells launch through. Panics if a node ever names a mission
/// with no shell scene (a content bug the shells could not launch — caught loudly, never guessed).
fn scene_token(mission: MissionId) -> &'static str {
    match Scene::for_mission(mission) {
        Some(Scene::Mission1) => "mission1",
        Some(Scene::Mission2) => "mission2",
        Some(Scene::Mission3) => "mission3",
        other => panic!("campaign mission {mission:?} has no shell scene token (got {other:?})"),
    }
}

/// Build the canonical `key = value` mirror facts from the live Rust data, in a fixed, stable
/// emission order. The Kotlin side rebuilds the identical key set from *its* mirror and compares.
fn canonical_facts() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |k: String, v: String| out.push((k, v));

    let campaign = default_campaign();
    let conflicts = campaign.conflicts();
    let operations = campaign.operations();

    push("campaign.conflict_count".into(), conflicts.len().to_string());
    push("campaign.operation_count".into(), operations.len().to_string());
    push("campaign.node_count".into(), campaign.len().to_string());

    for (i, c) in conflicts.iter().enumerate() {
        push(format!("campaign.conflict.{i:02}.name"), c.name.clone());
        push(format!("campaign.conflict.{i:02}.start_year"), c.start_year.to_string());
        push(format!("campaign.conflict.{i:02}.end_year"), c.end_year.to_string());
    }
    for (i, op) in operations.iter().enumerate() {
        push(format!("campaign.operation.{i:02}.name"), op.name.clone());
        push(format!("campaign.operation.{i:02}.conflict"), op.conflict.0.to_string());
    }
    for i in 0..campaign.len() {
        let node = campaign.node(NodeId(i as u32)).expect("node id in range");
        push(format!("campaign.node.{i:02}.scene_token"), scene_token(node.mission).into());
        push(
            format!("campaign.node.{i:02}.operation"),
            node.operation.map(|o| o.0.to_string()).unwrap_or_default(),
        );
        let mut prereqs: Vec<u32> = node.prerequisites.iter().map(|p| p.0).collect();
        prereqs.sort_unstable();
        let prereqs = prereqs.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        push(format!("campaign.node.{i:02}.prerequisites"), prereqs);
    }

    push("battlefields.count".into(), BATTLEFIELDS.len().to_string());
    let mut map_ids: Vec<&str> = Vec::new();
    for (i, bf) in BATTLEFIELDS.iter().enumerate() {
        push(format!("battlefield.{i:02}.id"), bf.id.to_string());
        let boot = match bf.kind {
            BattlefieldKind::Scene(token) => format!("scene:{token}"),
            BattlefieldKind::LibraryMap(id) => {
                map_ids.push(id);
                format!("map:{id}")
            }
        };
        push(format!("battlefield.{i:02}.boot"), boot);
    }
    push("maps.count".into(), map_ids.len().to_string());
    push("maps.ids".into(), map_ids.join(","));

    out
}

/// Serialize the facts to the committed text form: the header, then one `key=value` line each.
fn serialize(facts: &[(String, String)]) -> String {
    let mut s = String::from(FIXTURE_HEADER);
    for (k, v) in facts {
        s.push_str(k);
        s.push('=');
        s.push_str(v);
        s.push('\n');
    }
    s
}

/// The committed fixture, resolved from this crate's manifest dir so `cargo test` finds it from any
/// working directory (repo-root `parity/d79-mirror.txt`, a sibling of the `engine/` crate).
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../parity/d79-mirror.txt")
}

#[test]
fn rust_mirror_facts_match_the_committed_fixture() {
    let generated = serialize(&canonical_facts());
    let path = fixture_path();

    // `UPDATE_D79_FIXTURE=1` re-blesses the fixture from live Rust data (run in the same commit as
    // an intentional campaign/map change), then also mirror it into Kotlin. Otherwise it is a pure
    // read-only assertion so CI stays deterministic and side-effect-free.
    if std::env::var_os("UPDATE_D79_FIXTURE").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("create parity dir");
        std::fs::write(&path, &generated).expect("write fixture");
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read the D79 parity fixture at {}: {e}. Regenerate with \
             `UPDATE_D79_FIXTURE=1 cargo test -p gonedark-engine --test d79_mirror_parity`.",
            path.display()
        )
    });

    assert_eq!(
        generated, committed,
        "\n\nThe live Rust campaign/battlefield data no longer matches the committed D79 parity \
         fixture ({}). If you changed the campaign or the battlefield table on purpose, re-bless \
         the fixture AND update the Android mirror (CampaignModel.kt / Battlefield.kt) in the same \
         commit:\n    UPDATE_D79_FIXTURE=1 cargo test -p gonedark-engine --test d79_mirror_parity\n",
        path.display()
    );
}
