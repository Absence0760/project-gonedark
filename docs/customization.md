# Customization — gunsmith, cosmetics, HUD layout *(working design)*

> Status: living design doc. Three customization surfaces, each bounded by a locked
> invariant so none of them can become pay-to-win or unfair. The *why* is
> [`decisions.md`](decisions.md) [D60](decisions.md) (weapons) / [D61](decisions.md) (HUD);
> both reaffirm [D13](decisions.md) (cosmetic-only monetization) and pillar 4 (*the cost
> must always feel fair* — [`game-design.md`](game-design.md) §2).

The hard rule that shapes all three: **nothing a player can unlock or buy may grant a net
power advantage.** Weapon *function* is horizontal (trades, not upgrades); weapon *looks*,
and the whole HUD layout, are presentation-only. This is the only customization model that
doesn't contradict the fairness argument the entire game rests on.

---

## 1. Weapon customization — the horizontal gunsmith

A **Call of Duty: Mobile gunsmith** in shape — attachment slots on the embodied weapon
([D51](decisions.md)) — but **every attachment is a trade, never an upgrade.**

```
   ┌─────────────── RIFLE · loadout ───────────────┐
   │ Barrel    [ Long ]  +range          −ADS speed │
   │ Optic     [ 2× ]    +precision      −hipfire   │
   │ Grip      [ Vert ]  +recoil ctrl    −handling  │
   │ Mag       [ Ext ]   +ammo           −reload spd │
   │ Stock     [ Light ] +move-while-aim −stability  │
   └────────────────────────────────────────────────┘
     net power across any full build ≈ constant — you
     pick a SHAPE (sniper / brawler / runner), not a TIER
```

- **Sidegrade, by design rule.** Each attachment spends one stat to buy another. There is
  **no strictly-dominant build** — the same anti-degeneracy discipline the balance-metrics
  harness already enforces on units ([D30](decisions.md): a strictly-dominated Heavy was a
  *bug*). A loadout expresses a playstyle (long-range marksman vs close-quarters runner),
  not a power level. This is what lets the gunsmith carry real depth into PvP without
  touching [D13](decisions.md).
- **Unlocks are content, not power.** Playing the campaign ([`pve-campaign.md`](pve-campaign.md))
  unlocks *more attachment options and unit types to try* — a wider palette, never a higher
  ceiling. A new player and a veteran field loadouts of equal power; the veteran just has
  more *shapes* available and the skill to exploit them.

### Determinism — loadout stat deltas ARE sim state

This is the one place customization crosses into the simulation, so it gets the full
fixed-point treatment:

- Attachment deltas are **fixed-point (Q16.16, [D17](decisions.md))**, applied to the unit's
  weapon component **at match start** as deterministic **match-setup input** — chosen on the
  command layer before the dive, never mutated live.
- Because the resulting weapon stats live in the weapon component, they are **folded into the
  per-tick checksum** automatically (`Sim::fold`, [D28](decisions.md)) — a loadout desync
  would be caught by the cross-arch matrix (invariant #7) like any other sim divergence.
- **No floats, ever** (invariant #1): the attachment table is integer Q16.16; range, ADS
  time, recoil, reload, and handling are all fixed-point quantities the combat system
  ([`core/src/combat.rs`](../core/src/combat.rs)) already speaks.

The cosmetic half of the weapon (below) is the opposite — it must *never* reach the sim.

---

## 2. Cosmetics — presentation-only, the D13 store

Skins, paint, camo, charms, reticle colour, kill effects. These are the **only** purchasable
goods ([D13](decisions.md)) and they are **strictly presentation-layer**:

- They ride the decoupled sim/render split (invariant #4) for free — a skin is render data,
  full stop. It **cannot** affect determinism, hitboxes, **silhouette readability**, or the
  embodied-unit tell ([D33](decisions.md) — a cosmetic must not make the gone-dark marker
  easier or harder to read). Those are the hard guardrails already written into D13.
- They feed the cosmetic-only store and its per-platform billing rails ([Q9](open-questions.md)).
- Earnable *and* purchasable: higher-difficulty campaign clears drop cosmetic variants
  (the Destiny-2 "chase a look" hook), and the same catalogue is the store.

The line between §1 and §2 is the line between *function* and *identity*: function is
horizontal and free; identity is the thing you pay for. Selling **identity, not advantage**
is the whole monetization thesis ([`game-design.md`](game-design.md) §12).

---

## 3. Mobile HUD customization — the layout editor

A **CoD: Mobile / Mobile Legends: Bang Bang** style layout editor: the player can move,
resize, and set the opacity of **every** on-screen control, and save presets.

- **Per-layer presets.** The command layer and the embodied layer are *different control
  sets* (RTS selection/orders vs twin-stick + Fire/Crouch/Reload/Surface — [D51](decisions.md)),
  so each gets its own editable layout. A thumb-reach that's right for driving a tank is
  wrong for marquee-selecting a squad; the editor respects that split.
- **Multiple saved presets + reset-to-default**, so a player can keep a left-handed layout,
  a tablet layout, a phone layout.
- **Pure presentation / input-mapping — never sim.** The editor changes *where a control is
  and what raw touch maps to which intent*; it is the visible build-out of the existing
  roadmap item *"Touch-layout / rebind editor."* It plugs into the host-tested touch seam
  that maps `InputFrame.touches` → intents + HUD geometry
  ([`engine`](../engine/src/lib.rs) `touch_controls`, [D51](decisions.md)) and renders
  through the screen-space HUD pass ([`render::touch_controls`](../render/src/lib.rs)). It
  lives in the native **Settings** shell ([D32](decisions.md)).

### The hard constraint — invariant #6

A layout editor configures **placement, never information.** It may **not** add, reveal, or
relocate any element that surfaces strategic intel while embodied — no "drag a minimap onto
the FPS view," no enemy-position readout, nothing that defeats *going dark*
([`game-design.md`](game-design.md) §6). The going-dark alert channel stays the directional
flash + audio it already is; the editor can reposition the *alert indicator*, not turn it
into a map. Accessibility cues (the colorblind / hard-of-hearing equivalent of the alert
channel — a roadmap settings item) are a **separate** surface from this cosmetic layout
editor and are not optional.

---

## 4. Summary — three surfaces, three guardrails

| Surface | Touches sim? | Guardrail | Decision |
|---|---|---|---|
| **Gunsmith** (weapon function) | **Yes** — fixed-point deltas, checksum-folded | Horizontal sidegrades; no dominant build | [D60](decisions.md) |
| **Cosmetics** (weapon/unit looks) | No — render-only | Can't touch determinism / hitbox / silhouette / tell | [D13](decisions.md)/[D60](decisions.md) |
| **HUD layout editor** | No — input-mapping + screen-space draw | Placement not information; obeys invariant #6 | [D61](decisions.md) |

Build sequencing for all three is in [`pve-campaign-plan.md`](pve-campaign-plan.md)
(WS-C gunsmith, WS-D HUD editor); cosmetics ride the Phase 4 store surface.
