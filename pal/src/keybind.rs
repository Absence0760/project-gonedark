//! Pure, **platform-neutral** key-rebind model for the desktop keyboard bindings — the host
//! toggles (the D90 rebind editor) *and*, since Q27 closed, the gameplay keymap
//! (move/fire/embody/build/train/upgrade/…) that `pal-desktop`'s `DesktopInput` decodes. It lives
//! in `gonedark-pal` (zero deps — no window, no GPU, no sim) because it is shared vocabulary on
//! both sides of the PAL seam: `engine`/`app` own the rebind editor + persistence, while
//! `pal-desktop` consumes a **host-owned** [`KeybindMap`] to decode real key events — without the
//! PAL backend ever depending on `engine` (invariant #2; the same reasoning that puts
//! [`InputFrame`](crate::InputFrame) here). `engine::keybind` re-exports this module, so the D90
//! call sites are unchanged.
//!
//! **Presentation only.** A keybind picks which *physical key* fires an action; it never reaches
//! the deterministic sim, so it is not fixed-point, not checksummed, and cannot desync lockstep
//! (invariants #1/#4). The action *effects* are the same intents the desktop always had — this
//! just makes the trigger key data instead of a hardcoded `KeyCode` match.
//!
//! **Layered conflicts.** The desktop keymap deliberately shares keys across *mode-exclusive*
//! views (D42): `R` queues a Rifleman in the command view AND reloads while embodied — the engine
//! only ever reads one of the two, so one key is unambiguous. The conflict rule encodes that:
//! every action has a [`BindLayer`] (`Global` / `Command` / `Embodied`), and two actions may share
//! a key **iff their layers can never be active at the same time** (`Command` vs `Embodied`).
//! `Global` actions (host toggles, movement) conflict with everything.
//!
//! Enums that persist are stored by **stable ordinal** ([`KeyId::index`]/[`KeyId::from_index`],
//! [`GameAction::index`]/[`GameAction::from_index`]) — the same forward-compatible codec pattern
//! `shell::QualityChoice::index`/`from_index` uses, so a *renamed* variant can't silently
//! invalidate a saved blob and an out-of-range ordinal decodes to a default rather than panicking.
//! [`GameAction::ALL`] is **append-only**: the three D90 host toggles keep ordinals 0–2, so a
//! pre-Q27 saved blob decodes its host rebinds and leaves the gameplay keys at their defaults.

/// A physical-key identifier — a platform-neutral mirror of the `winit::KeyCode` subset the desktop
/// host binds. Deliberately **not** `winit::KeyCode`: this crate depends on no windowing crate
/// (invariant #2), so the platform boundary converts (`winit::KeyCode` ↔ `KeyId` in
/// `pal-desktop`/`app`, egui `Key` ↔ `KeyId` in the shell). Serialized by stable ordinal in
/// [`KeyId::ALL`] order.
///
/// The vocabulary is intentionally the non-modifier keys a binding can target (letters, digits,
/// function keys, and common navigation/editing keys). Bare modifiers (Alt/Ctrl/Shift) are **not**
/// here: the one host action that uses a modifier (hold-Left-Alt to free the cursor) is a
/// held-modifier gesture, not a discrete rebindable trigger, so it stays hardcoded in `app`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum KeyId {
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Escape,
    Tab,
    Space,
    Enter,
    Backspace,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Minus,
    Equals,
    Backquote,
}

impl KeyId {
    /// Every key in the fixed persistence order — the **ordinal contract**. Append-only: adding a key
    /// goes at the end so existing saved ordinals never shift (mirrors `QualityChoice::ALL`).
    pub const ALL: [KeyId; 66] = [
        KeyId::F1,
        KeyId::F2,
        KeyId::F3,
        KeyId::F4,
        KeyId::F5,
        KeyId::F6,
        KeyId::F7,
        KeyId::F8,
        KeyId::F9,
        KeyId::F10,
        KeyId::F11,
        KeyId::F12,
        KeyId::A,
        KeyId::B,
        KeyId::C,
        KeyId::D,
        KeyId::E,
        KeyId::F,
        KeyId::G,
        KeyId::H,
        KeyId::I,
        KeyId::J,
        KeyId::K,
        KeyId::L,
        KeyId::M,
        KeyId::N,
        KeyId::O,
        KeyId::P,
        KeyId::Q,
        KeyId::R,
        KeyId::S,
        KeyId::T,
        KeyId::U,
        KeyId::V,
        KeyId::W,
        KeyId::X,
        KeyId::Y,
        KeyId::Z,
        KeyId::Digit0,
        KeyId::Digit1,
        KeyId::Digit2,
        KeyId::Digit3,
        KeyId::Digit4,
        KeyId::Digit5,
        KeyId::Digit6,
        KeyId::Digit7,
        KeyId::Digit8,
        KeyId::Digit9,
        KeyId::Escape,
        KeyId::Tab,
        KeyId::Space,
        KeyId::Enter,
        KeyId::Backspace,
        KeyId::Insert,
        KeyId::Delete,
        KeyId::Home,
        KeyId::End,
        KeyId::PageUp,
        KeyId::PageDown,
        KeyId::Up,
        KeyId::Down,
        KeyId::Left,
        KeyId::Right,
        KeyId::Minus,
        KeyId::Equals,
        KeyId::Backquote,
    ];

    /// The short on-screen label for the key (the binding readout). ASCII only — it renders in egui's
    /// default font and must never tofu (the shell's default-font rule).
    pub fn label(self) -> &'static str {
        match self {
            KeyId::F1 => "F1",
            KeyId::F2 => "F2",
            KeyId::F3 => "F3",
            KeyId::F4 => "F4",
            KeyId::F5 => "F5",
            KeyId::F6 => "F6",
            KeyId::F7 => "F7",
            KeyId::F8 => "F8",
            KeyId::F9 => "F9",
            KeyId::F10 => "F10",
            KeyId::F11 => "F11",
            KeyId::F12 => "F12",
            KeyId::A => "A",
            KeyId::B => "B",
            KeyId::C => "C",
            KeyId::D => "D",
            KeyId::E => "E",
            KeyId::F => "F",
            KeyId::G => "G",
            KeyId::H => "H",
            KeyId::I => "I",
            KeyId::J => "J",
            KeyId::K => "K",
            KeyId::L => "L",
            KeyId::M => "M",
            KeyId::N => "N",
            KeyId::O => "O",
            KeyId::P => "P",
            KeyId::Q => "Q",
            KeyId::R => "R",
            KeyId::S => "S",
            KeyId::T => "T",
            KeyId::U => "U",
            KeyId::V => "V",
            KeyId::W => "W",
            KeyId::X => "X",
            KeyId::Y => "Y",
            KeyId::Z => "Z",
            KeyId::Digit0 => "0",
            KeyId::Digit1 => "1",
            KeyId::Digit2 => "2",
            KeyId::Digit3 => "3",
            KeyId::Digit4 => "4",
            KeyId::Digit5 => "5",
            KeyId::Digit6 => "6",
            KeyId::Digit7 => "7",
            KeyId::Digit8 => "8",
            KeyId::Digit9 => "9",
            KeyId::Escape => "Esc",
            KeyId::Tab => "Tab",
            KeyId::Space => "Space",
            KeyId::Enter => "Enter",
            KeyId::Backspace => "Backspace",
            KeyId::Insert => "Insert",
            KeyId::Delete => "Delete",
            KeyId::Home => "Home",
            KeyId::End => "End",
            KeyId::PageUp => "PageUp",
            KeyId::PageDown => "PageDown",
            KeyId::Up => "Up",
            KeyId::Down => "Down",
            KeyId::Left => "Left",
            KeyId::Right => "Right",
            KeyId::Minus => "Minus",
            KeyId::Equals => "Equals",
            KeyId::Backquote => "Backquote",
        }
    }

    /// This key's stable ordinal in [`KeyId::ALL`] — the persisted value.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&k| k == self).unwrap_or(0)
    }

    /// The key at persisted ordinal `i`, or `None` for an out-of-range ordinal (the tolerant decode
    /// side — the caller substitutes a default, mirroring `QualityChoice::from_index` but reporting
    /// the miss so the decoder can keep an action's *own* default rather than a blanket fallback).
    pub fn from_index(i: usize) -> Option<KeyId> {
        Self::ALL.get(i).copied()
    }
}

/// The mutually-exclusive input context a bound action fires in — the **conflict domain** for
/// [`KeybindMap::rebind`]. The command layer and the embodiment layer are mutually exclusive in
/// time (the game's core loop), so a `Command` action and an `Embodied` action may share a
/// physical key without ambiguity (the shipped `R` = train Rifleman / reload). `Global` actions
/// are live in both (host toggles apply on every screen; movement drives both the command camera
/// and embodied locomotion), so they conflict with everything.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum BindLayer {
    /// Active in every context (host toggles, movement) — conflicts with all layers.
    Global,
    /// Command-view only (the RTS layer): embody, orders, production.
    Command,
    /// Embodied only (the FPS layer): surface, jump, crouch, reload, fire mode.
    Embodied,
}

impl BindLayer {
    /// Whether two layers can ever be active at the same time — the sharing rule: actions whose
    /// layers conflict may **not** share a key; `Command` vs `Embodied` may.
    pub fn conflicts_with(self, other: BindLayer) -> bool {
        self == BindLayer::Global || other == BindLayer::Global || self == other
    }
}

/// A rebindable desktop key action. The first three are the `app`-owned **host toggles** the D90
/// editor shipped with (their ordinals 0–2 are frozen — a pre-Q27 saved blob must keep decoding);
/// the rest are the **gameplay** keymap `pal-desktop`'s `DesktopInput` decodes through a host-owned
/// [`KeybindMap`] (Q27). Mouse buttons are not here — the classic-RTS button split (D42: left
/// select/fire, right command/aim) is a design lock, not a preference; and hold-Left-Alt (free the
/// cursor) is a held-modifier gesture, not a discrete trigger, so it stays hardcoded in `app`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum GameAction {
    /// Pause / resume the running match (host-side session overlay — never the sim).
    Pause,
    /// Toggle borderless fullscreen (the window mode; applies on every screen).
    ToggleFullscreen,
    /// Toggle the debug hitbox/facet overlay (command view; a presentation toggle).
    ToggleDebugOverlay,
    /// Possess the selected unit (command view; the one-shot embody edge).
    Embody,
    /// Surface back to command (embodied; the one-shot eject edge).
    Surface,
    /// Move up / forward (held; command-camera pan + embodied locomotion).
    MoveUp,
    /// Move down / back (held).
    MoveDown,
    /// Move left (held).
    MoveLeft,
    /// Move right (held).
    MoveRight,
    /// Embodied cosmetic jump (one-shot edge).
    Jump,
    /// Hold to open the radial order/stance menu (command view; a HELD level, not an edge).
    OrderMenu,
    /// Order/stance vocabulary slot 1 (command view; one-shot).
    OrderSlot1,
    OrderSlot2,
    OrderSlot3,
    OrderSlot4,
    OrderSlot5,
    OrderSlot6,
    OrderSlot7,
    OrderSlot8,
    OrderSlot9,
    OrderSlot10,
    /// Place a Camp at the cursor's ground point (command view; one-shot).
    BuildCamp,
    /// Queue a Rifleman at the active camp (command view; one-shot). Shares its default key (`R`)
    /// with [`GameAction::Reload`] across the mode-exclusive layers.
    TrainRifleman,
    /// Queue a Heavy at the active camp (command view; one-shot).
    TrainHeavy,
    /// Upgrade the active camp (command view; one-shot).
    UpgradeCamp,
    /// Toggle crouch posture (embodied; one-shot — the sim inverts posture, so never a held level).
    Crouch,
    /// Start a reload (embodied; one-shot).
    Reload,
    /// Toggle the fire mode, semi ⇄ auto (embodied; one-shot).
    SelectFire,
}

impl GameAction {
    /// Every action in the fixed persistence / display order — the ordinal contract for the encoded
    /// keybind blob (each action's key is written at its `ALL` position). **Append-only**: the three
    /// D90 host toggles stay at ordinals 0–2 so pre-Q27 blobs keep decoding.
    pub const ALL: [GameAction; 28] = [
        GameAction::Pause,
        GameAction::ToggleFullscreen,
        GameAction::ToggleDebugOverlay,
        GameAction::Embody,
        GameAction::Surface,
        GameAction::MoveUp,
        GameAction::MoveDown,
        GameAction::MoveLeft,
        GameAction::MoveRight,
        GameAction::Jump,
        GameAction::OrderMenu,
        GameAction::OrderSlot1,
        GameAction::OrderSlot2,
        GameAction::OrderSlot3,
        GameAction::OrderSlot4,
        GameAction::OrderSlot5,
        GameAction::OrderSlot6,
        GameAction::OrderSlot7,
        GameAction::OrderSlot8,
        GameAction::OrderSlot9,
        GameAction::OrderSlot10,
        GameAction::BuildCamp,
        GameAction::TrainRifleman,
        GameAction::TrainHeavy,
        GameAction::UpgradeCamp,
        GameAction::Crouch,
        GameAction::Reload,
        GameAction::SelectFire,
    ];

    /// The human-readable label for the action row. ASCII only (egui default-font rule).
    pub fn label(self) -> &'static str {
        match self {
            GameAction::Pause => "Pause / resume",
            GameAction::ToggleFullscreen => "Toggle fullscreen",
            GameAction::ToggleDebugOverlay => "Toggle debug overlay",
            GameAction::Embody => "Embody unit",
            GameAction::Surface => "Surface to command",
            GameAction::MoveUp => "Move forward / pan up",
            GameAction::MoveDown => "Move back / pan down",
            GameAction::MoveLeft => "Move left",
            GameAction::MoveRight => "Move right",
            GameAction::Jump => "Jump",
            GameAction::OrderMenu => "Order menu (hold)",
            GameAction::OrderSlot1 => "Order slot 1",
            GameAction::OrderSlot2 => "Order slot 2",
            GameAction::OrderSlot3 => "Order slot 3",
            GameAction::OrderSlot4 => "Order slot 4",
            GameAction::OrderSlot5 => "Order slot 5",
            GameAction::OrderSlot6 => "Order slot 6",
            GameAction::OrderSlot7 => "Order slot 7",
            GameAction::OrderSlot8 => "Order slot 8",
            GameAction::OrderSlot9 => "Order slot 9",
            GameAction::OrderSlot10 => "Order slot 10",
            GameAction::BuildCamp => "Build camp",
            GameAction::TrainRifleman => "Train rifleman",
            GameAction::TrainHeavy => "Train heavy",
            GameAction::UpgradeCamp => "Upgrade camp",
            GameAction::Crouch => "Crouch",
            GameAction::Reload => "Reload",
            GameAction::SelectFire => "Fire mode (semi/auto)",
        }
    }

    /// The shipped default key for the action — the desktop's historical hardcoded binding (the D90
    /// host toggles plus the classic-RTS gameplay keymap `DesktopInput` used to hardcode).
    /// [`KeybindMap::default`] is built from these; they are conflict-free under the layer rule
    /// (`R` is shared by [`TrainRifleman`](Self::TrainRifleman)/[`Reload`](Self::Reload), whose
    /// layers never overlap). The pre-Q27 hardcoded `V` reload *secondary* is retired — a rebindable
    /// map holds one key per action, and the player can simply bind Reload to `V`.
    pub fn default_key(self) -> KeyId {
        match self {
            GameAction::Pause => KeyId::Escape,
            GameAction::ToggleFullscreen => KeyId::F11,
            GameAction::ToggleDebugOverlay => KeyId::F3,
            GameAction::Embody => KeyId::E,
            GameAction::Surface => KeyId::Q,
            GameAction::MoveUp => KeyId::W,
            GameAction::MoveDown => KeyId::S,
            GameAction::MoveLeft => KeyId::A,
            GameAction::MoveRight => KeyId::D,
            GameAction::Jump => KeyId::Space,
            GameAction::OrderMenu => KeyId::F,
            GameAction::OrderSlot1 => KeyId::Digit1,
            GameAction::OrderSlot2 => KeyId::Digit2,
            GameAction::OrderSlot3 => KeyId::Digit3,
            GameAction::OrderSlot4 => KeyId::Digit4,
            GameAction::OrderSlot5 => KeyId::Digit5,
            GameAction::OrderSlot6 => KeyId::Digit6,
            GameAction::OrderSlot7 => KeyId::Digit7,
            GameAction::OrderSlot8 => KeyId::Digit8,
            GameAction::OrderSlot9 => KeyId::Digit9,
            GameAction::OrderSlot10 => KeyId::Digit0,
            GameAction::BuildCamp => KeyId::B,
            GameAction::TrainRifleman => KeyId::R,
            GameAction::TrainHeavy => KeyId::H,
            GameAction::UpgradeCamp => KeyId::U,
            GameAction::Crouch => KeyId::C,
            GameAction::Reload => KeyId::R,
            GameAction::SelectFire => KeyId::X,
        }
    }

    /// The [`BindLayer`] this action fires in — its conflict domain. Host toggles and movement are
    /// `Global` (live everywhere); the rest split between the mutually-exclusive command /
    /// embodiment layers exactly as the engine consumes them.
    pub fn layer(self) -> BindLayer {
        match self {
            GameAction::Pause
            | GameAction::ToggleFullscreen
            | GameAction::ToggleDebugOverlay
            | GameAction::MoveUp
            | GameAction::MoveDown
            | GameAction::MoveLeft
            | GameAction::MoveRight => BindLayer::Global,
            GameAction::Embody
            | GameAction::OrderMenu
            | GameAction::OrderSlot1
            | GameAction::OrderSlot2
            | GameAction::OrderSlot3
            | GameAction::OrderSlot4
            | GameAction::OrderSlot5
            | GameAction::OrderSlot6
            | GameAction::OrderSlot7
            | GameAction::OrderSlot8
            | GameAction::OrderSlot9
            | GameAction::OrderSlot10
            | GameAction::BuildCamp
            | GameAction::TrainRifleman
            | GameAction::TrainHeavy
            | GameAction::UpgradeCamp => BindLayer::Command,
            GameAction::Surface
            | GameAction::Jump
            | GameAction::Crouch
            | GameAction::Reload
            | GameAction::SelectFire => BindLayer::Embodied,
        }
    }

    /// For the ten order/stance vocabulary actions, the **zero-based wire slot** the engine's
    /// `command_ui` expects (`OrderSlot1` → `0` … `OrderSlot10` → `9`); `None` for every other
    /// action. Kept here so the `pal-desktop` decode and any future backend agree on the slot
    /// numbering without re-deriving it.
    pub fn order_slot(self) -> Option<u8> {
        match self {
            GameAction::OrderSlot1 => Some(0),
            GameAction::OrderSlot2 => Some(1),
            GameAction::OrderSlot3 => Some(2),
            GameAction::OrderSlot4 => Some(3),
            GameAction::OrderSlot5 => Some(4),
            GameAction::OrderSlot6 => Some(5),
            GameAction::OrderSlot7 => Some(6),
            GameAction::OrderSlot8 => Some(7),
            GameAction::OrderSlot9 => Some(8),
            GameAction::OrderSlot10 => Some(9),
            _ => None,
        }
    }

    /// This action's stable ordinal in [`GameAction::ALL`].
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&a| a == self).unwrap_or(0)
    }

    /// The action at persisted ordinal `i`, or `None` if out of range.
    pub fn from_index(i: usize) -> Option<GameAction> {
        Self::ALL.get(i).copied()
    }
}

/// The outcome of a [`KeybindMap::rebind`] attempt — the pure decision the Settings UI renders as
/// feedback. Conflict-avoidance is the load-bearing rule: **two actions whose layers overlap can
/// never share a key** (mode-exclusive layers may — see [`BindLayer`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RebindOutcome {
    /// The action was bound to the new key.
    Bound,
    /// The action already held that key — nothing changed.
    Unchanged,
    /// Rejected: the key is already owned by an action in an overlapping layer (returned so the UI
    /// can name it). The existing binding is left untouched — the player must free the key first
    /// (rebind the other action, or reset defaults).
    Conflict(GameAction),
}

/// The live key→action bindings for every rebindable desktop action — host toggles + the gameplay
/// keymap. Stored as one [`KeyId`] per action, indexed by [`GameAction::index`]. The map is an
/// **invariant-holding** type: it starts conflict-free (the defaults respect the layer rule) and
/// [`rebind`](Self::rebind) refuses any change that would make two overlapping-layer actions share
/// a key, so a live map is always unambiguous in every input context. Pure data — no window, no
/// GPU, no sim. The **host owns the instance** (`app`'s `SettingsState`): the app boundary converts
/// real key events into [`KeyId`] for its own toggles, and pushes a copy into `pal-desktop`'s
/// `DesktopInput` each frame so the PAL decode resolves gameplay keys through the same map
/// (invariant #2 — the PAL consumes neutral data, never `engine` types).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeybindMap {
    /// One key per action, indexed by `GameAction::index()`.
    keys: [KeyId; GameAction::ALL.len()],
}

impl Default for KeybindMap {
    /// The shipped defaults: each action bound to its [`GameAction::default_key`].
    fn default() -> Self {
        let mut keys = [KeyId::Escape; GameAction::ALL.len()];
        for a in GameAction::ALL {
            keys[a.index()] = a.default_key();
        }
        KeybindMap { keys }
    }
}

impl KeybindMap {
    /// The key currently bound to `action`.
    pub fn key_for(&self, action: GameAction) -> KeyId {
        self.keys[action.index()]
    }

    /// The **first** action (in [`GameAction::ALL`] order) a physical key is bound to, if any.
    /// Host toggles come first in `ALL`, and a key bound to a `Global` action is exclusively held
    /// (the layer rule), so the host's key routing stays unambiguous. A key shared across the
    /// mode-exclusive layers returns its command-layer owner — callers that care about *every*
    /// owner (the PAL decode) iterate `ALL` and compare [`key_for`](Self::key_for). `None` means
    /// the key is unbound.
    pub fn action_for(&self, key: KeyId) -> Option<GameAction> {
        GameAction::ALL
            .into_iter()
            .find(|&a| self.keys[a.index()] == key)
    }

    /// Attempt to bind `action` to `key`, upholding the layered no-shared-keys invariant. Returns
    /// [`RebindOutcome::Unchanged`] if it already holds the key, [`RebindOutcome::Conflict`]
    /// (leaving the map untouched) if an overlapping-layer action owns it, else assigns it and
    /// returns [`RebindOutcome::Bound`]. Pure — the Settings capture flow calls this after the
    /// `app` boundary resolves the pressed key to a [`KeyId`].
    pub fn rebind(&mut self, action: GameAction, key: KeyId) -> RebindOutcome {
        if self.keys[action.index()] == key {
            return RebindOutcome::Unchanged;
        }
        // Only an action in a layer that can be live at the same time blocks the bind; a
        // mode-exclusive sibling (Command vs Embodied) may share the key.
        if let Some(owner) = self.conflicting_owner(action, key) {
            return RebindOutcome::Conflict(owner);
        }
        self.keys[action.index()] = key;
        RebindOutcome::Bound
    }

    /// The first *other* action holding `key` whose layer overlaps `action`'s — the conflict the
    /// rebind (and decode validation) rejects. `None` when the key is free for `action`.
    fn conflicting_owner(&self, action: GameAction, key: KeyId) -> Option<GameAction> {
        GameAction::ALL.into_iter().find(|&b| {
            b != action
                && self.keys[b.index()] == key
                && b.layer().conflicts_with(action.layer())
        })
    }

    /// Restore every action to its shipped default binding — the rebind editor's reset-to-defaults.
    pub fn reset(&mut self) {
        *self = KeybindMap::default();
    }

    /// Whether any two overlapping-layer actions share a key (the invariant this type upholds
    /// should make this always `false` for a live map). Used only by [`decode`](Self::decode) to
    /// reject a hand-corrupted blob; a cross-layer share (the shipped `R`) is legitimate.
    fn has_conflict(&self) -> bool {
        for i in 0..GameAction::ALL.len() {
            for j in (i + 1)..GameAction::ALL.len() {
                let (a, b) = (GameAction::ALL[i], GameAction::ALL[j]);
                if self.keys[i] == self.keys[j] && a.layer().conflicts_with(b.layer()) {
                    return true;
                }
            }
        }
        false
    }

    /// Encode the map to a compact, stable string: each action's [`KeyId`] ordinal in
    /// [`GameAction::ALL`] order, comma-separated (the D90 blob, now 28 fields — the first three
    /// are still the host toggles, so the format is a strict extension). The shell-prefs codec
    /// stores this as one value. A save→load round-trip is stable because every field is a stable
    /// ordinal.
    pub fn encode(&self) -> String {
        GameAction::ALL
            .iter()
            .map(|a| self.key_for(*a).index().to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Tolerantly decode an [`encode`](Self::encode) string back to a map. Every field that is
    /// missing, unparseable, or an out-of-range ordinal keeps *that action's* shipped default — so
    /// a pre-Q27 three-field blob decodes its host-toggle rebinds and leaves every gameplay key at
    /// its default. Then, if the result would violate the layered no-shared-keys invariant (a
    /// hand-edited/corrupt blob, or a legacy host rebind now colliding with a gameplay default),
    /// the whole map falls back to defaults rather than ship an ambiguous key. This **never
    /// panics** — an empty or garbage blob decodes to the shipped defaults, mirroring the shell
    /// codec's corruption-safety contract.
    pub fn decode(s: &str) -> KeybindMap {
        // Start from the defaults so a short/empty blob leaves the unspecified actions at their
        // shipped key (and an all-garbage blob reconstructs the default map exactly).
        let mut keys = KeybindMap::default().keys;
        for (i, field) in s.split(',').enumerate() {
            if i >= keys.len() {
                break;
            }
            if let Some(k) = field.trim().parse::<usize>().ok().and_then(KeyId::from_index) {
                keys[i] = k;
            }
        }
        let map = KeybindMap { keys };
        // A layer-overlapping duplicate can only come from a corrupt/hand-edited/stale blob (encode
        // of a live map is always conflict-free). Reject the whole thing to a known-good default
        // rather than ship a map that silently steals a key from another action.
        if map.has_conflict() {
            KeybindMap::default()
        } else {
            map
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bind_the_historical_keys_and_are_conflict_free() {
        let map = KeybindMap::default();
        // The D90 host toggles.
        assert_eq!(map.key_for(GameAction::Pause), KeyId::Escape);
        assert_eq!(map.key_for(GameAction::ToggleFullscreen), KeyId::F11);
        assert_eq!(map.key_for(GameAction::ToggleDebugOverlay), KeyId::F3);
        // A sample of the Q27 gameplay defaults (the keys `DesktopInput` used to hardcode).
        assert_eq!(map.key_for(GameAction::Embody), KeyId::E);
        assert_eq!(map.key_for(GameAction::Surface), KeyId::Q);
        assert_eq!(map.key_for(GameAction::MoveUp), KeyId::W);
        assert_eq!(map.key_for(GameAction::Jump), KeyId::Space);
        assert_eq!(map.key_for(GameAction::OrderMenu), KeyId::F);
        assert_eq!(map.key_for(GameAction::OrderSlot10), KeyId::Digit0);
        assert_eq!(map.key_for(GameAction::BuildCamp), KeyId::B);
        assert_eq!(map.key_for(GameAction::Crouch), KeyId::C);
        assert_eq!(map.key_for(GameAction::SelectFire), KeyId::X);
        assert!(!map.has_conflict(), "shipped defaults must not conflict");
    }

    #[test]
    fn every_action_has_a_default_and_no_overlapping_layer_shares_one() {
        // Default-map completeness: every action resolves to a key, and no two actions whose
        // layers can be live together share one (the flat-uniqueness rule of D90, relaxed only
        // across the mode-exclusive command/embodied split).
        let map = KeybindMap::default();
        for (i, &a) in GameAction::ALL.iter().enumerate() {
            let key = map.key_for(a); // total: every action has a binding
            for &b in &GameAction::ALL[i + 1..] {
                if map.key_for(b) == key {
                    assert!(
                        !a.layer().conflicts_with(b.layer()),
                        "{a:?} and {b:?} share {key:?} but their layers overlap"
                    );
                }
            }
        }
    }

    #[test]
    fn the_shipped_r_key_is_shared_across_the_mode_exclusive_layers() {
        // R = train-Rifleman (command) AND reload (embodied) — the D42 mode-exclusive share the
        // layer rule exists to keep. `action_for` reports the first owner in ALL order (the
        // command-layer one); the PAL decode iterates ALL, so both fire.
        let map = KeybindMap::default();
        assert_eq!(map.key_for(GameAction::TrainRifleman), KeyId::R);
        assert_eq!(map.key_for(GameAction::Reload), KeyId::R);
        assert_eq!(map.action_for(KeyId::R), Some(GameAction::TrainRifleman));
    }

    #[test]
    fn action_for_is_the_reverse_of_key_for_on_exclusively_held_keys() {
        let map = KeybindMap::default();
        for a in GameAction::ALL {
            let owner = map.action_for(map.key_for(a)).expect("bound key resolves");
            // The resolved owner holds the same key; it IS `a` unless the key is legitimately
            // shared across layers (the shipped R), where the first owner in ALL order wins.
            assert_eq!(map.key_for(owner), map.key_for(a));
        }
        // An unbound key routes to nothing (so a stray press does nothing).
        assert_eq!(map.action_for(KeyId::J), None);
    }

    #[test]
    fn rebind_to_a_free_key_binds_and_reroutes() {
        let mut map = KeybindMap::default();
        assert_eq!(map.rebind(GameAction::Pause, KeyId::P), RebindOutcome::Bound);
        assert_eq!(map.key_for(GameAction::Pause), KeyId::P);
        // The new key now routes to Pause; the old one (Esc) is free.
        assert_eq!(map.action_for(KeyId::P), Some(GameAction::Pause));
        assert_eq!(map.action_for(KeyId::Escape), None);
    }

    #[test]
    fn rebind_to_the_same_key_is_a_no_op() {
        let mut map = KeybindMap::default();
        assert_eq!(
            map.rebind(GameAction::Pause, KeyId::Escape),
            RebindOutcome::Unchanged
        );
        assert_eq!(map.key_for(GameAction::Pause), KeyId::Escape);
    }

    #[test]
    fn rebind_to_a_taken_key_is_rejected_and_names_the_owner() {
        let mut map = KeybindMap::default();
        // F11 belongs to ToggleFullscreen; binding Pause to it must be refused, naming the owner.
        assert_eq!(
            map.rebind(GameAction::Pause, KeyId::F11),
            RebindOutcome::Conflict(GameAction::ToggleFullscreen)
        );
        // The map is untouched — Pause still on Esc, Fullscreen still on F11 (invariant held).
        assert_eq!(map.key_for(GameAction::Pause), KeyId::Escape);
        assert_eq!(map.key_for(GameAction::ToggleFullscreen), KeyId::F11);
        assert!(!map.has_conflict());
    }

    #[test]
    fn rebind_conflicts_follow_the_layer_rule() {
        // Same layer: BuildCamp (command) can't take TrainRifleman's R (command).
        let mut map = KeybindMap::default();
        assert_eq!(
            map.rebind(GameAction::BuildCamp, KeyId::R),
            RebindOutcome::Conflict(GameAction::TrainRifleman)
        );
        // Global vs anything: Pause (global) can't take MoveUp's W (global) or Crouch's C (embodied).
        assert_eq!(
            map.rebind(GameAction::Pause, KeyId::W),
            RebindOutcome::Conflict(GameAction::MoveUp)
        );
        assert_eq!(
            map.rebind(GameAction::Pause, KeyId::C),
            RebindOutcome::Conflict(GameAction::Crouch)
        );
        // Anything vs Global: Jump (embodied) can't take MoveLeft's A (global).
        assert_eq!(
            map.rebind(GameAction::Jump, KeyId::A),
            RebindOutcome::Conflict(GameAction::MoveLeft)
        );
        // Cross-layer is allowed: Surface (embodied) may share Embody's E (command) — the
        // toggle-style bind a player might genuinely want.
        assert_eq!(map.rebind(GameAction::Surface, KeyId::E), RebindOutcome::Bound);
        assert_eq!(map.key_for(GameAction::Embody), KeyId::E);
        assert_eq!(map.key_for(GameAction::Surface), KeyId::E);
        assert!(!map.has_conflict(), "a cross-layer share is not a conflict");
    }

    #[test]
    fn reset_restores_defaults() {
        let mut map = KeybindMap::default();
        map.rebind(GameAction::Pause, KeyId::P);
        map.rebind(GameAction::Jump, KeyId::G);
        map.reset();
        assert_eq!(map, KeybindMap::default());
    }

    #[test]
    fn encode_decode_round_trips_defaults_and_a_remapped_map() {
        // Defaults round-trip.
        let def = KeybindMap::default();
        assert_eq!(KeybindMap::decode(&def.encode()), def);

        // A remapped map (host + gameplay rebinds) round-trips too — the identity contract.
        let mut map = KeybindMap::default();
        assert_eq!(map.rebind(GameAction::Pause, KeyId::P), RebindOutcome::Bound);
        assert_eq!(map.rebind(GameAction::Jump, KeyId::G), RebindOutcome::Bound);
        assert_eq!(map.rebind(GameAction::Embody, KeyId::T), RebindOutcome::Bound);
        assert_eq!(
            map.rebind(GameAction::ToggleFullscreen, KeyId::Backquote),
            RebindOutcome::Bound
        );
        assert_eq!(KeybindMap::decode(&map.encode()), map);
    }

    #[test]
    fn decode_tolerates_garbage_and_out_of_range() {
        // Total garbage → shipped defaults (never panics).
        assert_eq!(KeybindMap::decode(""), KeybindMap::default());
        assert_eq!(KeybindMap::decode("not,a,blob"), KeybindMap::default());
        // Out-of-range / partly-bad ordinals keep each action's own default.
        //   field 0 = "9999" (out of range) → Pause keeps Esc
        //   field 1 = "18"   (KeyId::G, valid + free) → Fullscreen becomes G
        //   field 2 = ""     (missing) → DebugOverlay keeps F3
        let m = KeybindMap::decode("9999,18,");
        assert_eq!(m.key_for(GameAction::Pause), KeyId::Escape);
        assert_eq!(m.key_for(GameAction::ToggleFullscreen), KeyId::G);
        assert_eq!(m.key_for(GameAction::ToggleDebugOverlay), KeyId::F3);
        // …and the unspecified gameplay tail keeps its defaults.
        assert_eq!(m.key_for(GameAction::Embody), KeyId::E);
        assert_eq!(m.key_for(GameAction::SelectFire), KeyId::X);
    }

    #[test]
    fn decode_accepts_a_pre_q27_three_field_blob() {
        // A D90-era blob carries only the three host toggles. It must keep decoding: the host
        // rebinds land, the gameplay keys (absent from the blob) stay at their defaults.
        // "27,10,2" = Pause→P, Fullscreen→F11, DebugOverlay→F3.
        let m = KeybindMap::decode("27,10,2");
        assert_eq!(m.key_for(GameAction::Pause), KeyId::P);
        assert_eq!(m.key_for(GameAction::ToggleFullscreen), KeyId::F11);
        assert_eq!(m.key_for(GameAction::ToggleDebugOverlay), KeyId::F3);
        assert_eq!(m.key_for(GameAction::MoveUp), KeyId::W);
        assert_eq!(m.key_for(GameAction::Reload), KeyId::R);
        // But a legacy host rebind that now collides with a gameplay default (Pause→B vs
        // BuildCamp's B; Global conflicts with Command) is a real ambiguity → whole map falls
        // back to the shipped defaults, per the corruption contract.
        let m = KeybindMap::decode("13,10,2");
        assert_eq!(m, KeybindMap::default());
    }

    #[test]
    fn decode_rejects_a_duplicate_key_blob_to_defaults() {
        // A hand-edited blob that binds two overlapping-layer actions to the same ordinal
        // (Esc = 48 for both Pause and Fullscreen) is corrupt: the whole map falls back to
        // defaults rather than shipping a shared-key map.
        let m = KeybindMap::decode("48,48,2");
        assert_eq!(m, KeybindMap::default());
        assert!(!m.has_conflict());
    }

    #[test]
    fn key_and_action_ordinals_are_stable_and_total() {
        // Every KeyId round-trips through its ordinal, and ALL has no gaps/dupes.
        for (i, &k) in KeyId::ALL.iter().enumerate() {
            assert_eq!(k.index(), i);
            assert_eq!(KeyId::from_index(i), Some(k));
        }
        assert_eq!(KeyId::from_index(KeyId::ALL.len()), None);
        for (i, &a) in GameAction::ALL.iter().enumerate() {
            assert_eq!(a.index(), i);
            assert_eq!(GameAction::from_index(i), Some(a));
        }
        // The D90 host-toggle ordinals are FROZEN (pre-Q27 blobs address them by position).
        assert_eq!(GameAction::Pause.index(), 0);
        assert_eq!(GameAction::ToggleFullscreen.index(), 1);
        assert_eq!(GameAction::ToggleDebugOverlay.index(), 2);
    }

    #[test]
    fn order_slots_map_to_the_zero_based_wire_slots() {
        // The ten vocabulary actions map onto wire slots 0–9 in order; nothing else has a slot.
        let slots: Vec<u8> = GameAction::ALL
            .into_iter()
            .filter_map(GameAction::order_slot)
            .collect();
        assert_eq!(slots, (0..10).collect::<Vec<u8>>());
        assert_eq!(GameAction::Embody.order_slot(), None);
        assert_eq!(GameAction::Pause.order_slot(), None);
    }

    #[test]
    fn labels_are_non_empty_ascii() {
        for &k in &KeyId::ALL {
            assert!(!k.label().is_empty() && k.label().is_ascii());
        }
        for a in GameAction::ALL {
            assert!(!a.label().is_empty() && a.label().is_ascii());
        }
    }
}
