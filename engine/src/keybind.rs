//! Pure, **winit-free** key-rebind model for the desktop host's Settings rebind editor (the D75
//! follow-up "the rebind editor still owed"). It lives in `engine` so the rebind / conflict-detection
//! / persistence logic is unit-tested with no window or GPU: the desktop `app` maps
//! `winit::KeyCode` ↔ [`KeyId`] (and egui's key events ↔ [`KeyId`]) at its own boundary, keeping this
//! seam free of any windowing/GPU crate (invariant #2 — `engine` never pulls in `winit`/`wgpu`).
//!
//! **Presentation only.** A keybind picks which *physical key* fires a host-side action (pause,
//! fullscreen, the debug overlay); it never reaches the deterministic sim, so it is not fixed-point,
//! not checksummed, and cannot desync lockstep (invariants #1/#4). The action *effects* are the same
//! host toggles the desktop always had — this just makes the trigger key data instead of a hardcoded
//! `KeyCode` match.
//!
//! Enums that persist are stored by **stable ordinal** ([`KeyId::index`]/[`KeyId::from_index`],
//! [`GameAction::index`]/[`GameAction::from_index`]) — the same forward-compatible codec pattern
//! `shell::QualityChoice::index`/`from_index` uses, so a *renamed* variant can't silently invalidate a
//! saved blob and an out-of-range ordinal decodes to a default rather than panicking.

/// A physical-key identifier — a platform-neutral mirror of the `winit::KeyCode` subset the desktop
/// host binds. Deliberately **not** `winit::KeyCode`: `engine` depends on no windowing crate
/// (invariant #2), so the `app` layer converts at its boundary (`winit::KeyCode` ↔ `KeyId` in
/// `main.rs`, egui `Key` ↔ `KeyId` in `shell.rs`). Serialized by stable ordinal in [`KeyId::ALL`]
/// order.
///
/// The vocabulary is intentionally the non-modifier keys a discrete toggle can bind to (letters,
/// digits, function keys, and common navigation/editing keys). Bare modifiers (Alt/Ctrl/Shift) are
/// **not** here: the one host action that uses a modifier (hold-Left-Alt to free the cursor) is a
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

/// A rebindable host-side action on the desktop Settings rebind editor. These are exactly the
/// discrete key toggles `app/src/main.rs` owns (its `KeyCode::` matches): pause the match, toggle
/// borderless fullscreen, and toggle the debug overlay. The *gameplay* keymap (move/fire/embody/
/// build/train/…) is decoded in `pal-desktop`'s `DesktopInput`, a separate crate outside this
/// editor's scope, so it is not listed here; and hold-Left-Alt (free the cursor) is a held-modifier
/// gesture, not a discrete trigger, so it stays hardcoded in `app`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum GameAction {
    /// Pause / resume the running match (host-side session overlay — never the sim).
    Pause,
    /// Toggle borderless fullscreen (the window mode; applies on every screen).
    ToggleFullscreen,
    /// Toggle the debug hitbox/facet overlay (command view; a presentation toggle).
    ToggleDebugOverlay,
}

impl GameAction {
    /// Every action in the fixed persistence / display order — the ordinal contract for the encoded
    /// keybind blob (each action's key is written at its `ALL` position).
    pub const ALL: [GameAction; 3] = [
        GameAction::Pause,
        GameAction::ToggleFullscreen,
        GameAction::ToggleDebugOverlay,
    ];

    /// The human-readable label for the action row. ASCII only (egui default-font rule).
    pub fn label(self) -> &'static str {
        match self {
            GameAction::Pause => "Pause / resume",
            GameAction::ToggleFullscreen => "Toggle fullscreen",
            GameAction::ToggleDebugOverlay => "Toggle debug overlay",
        }
    }

    /// The shipped default key for the action — the desktop's historical hardcoded binding (Esc /
    /// F11 / F3). [`KeybindMap::default`] is built from these, and they are guaranteed conflict-free.
    pub fn default_key(self) -> KeyId {
        match self {
            GameAction::Pause => KeyId::Escape,
            GameAction::ToggleFullscreen => KeyId::F11,
            GameAction::ToggleDebugOverlay => KeyId::F3,
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
/// feedback. Conflict-avoidance is the load-bearing rule: **two actions can never share a key.**
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RebindOutcome {
    /// The action was bound to the new key.
    Bound,
    /// The action already held that key — nothing changed.
    Unchanged,
    /// Rejected: the key is already owned by another action (returned so the UI can name it). The
    /// existing binding is left untouched — the player must free the key first (rebind the other
    /// action, or reset defaults).
    Conflict(GameAction),
}

/// The live key→action bindings for the rebindable host actions. Stored as one [`KeyId`] per action,
/// indexed by [`GameAction::index`]. The map is an **invariant-holding** type: it starts conflict-free
/// (the defaults are distinct) and [`rebind`](Self::rebind) refuses any change that would make two
/// actions share a key, so a live map always has unique bindings. Pure data — no window, no GPU, no
/// sim; the `app` boundary is what converts real key events into [`KeyId`] and calls
/// [`action_for`](Self::action_for) to route a press.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeybindMap {
    /// One key per action, indexed by `GameAction::index()`.
    keys: [KeyId; GameAction::ALL.len()],
}

impl Default for KeybindMap {
    /// The shipped defaults: each action bound to its [`GameAction::default_key`] (Esc / F11 / F3).
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

    /// The action a physical key is bound to, if any — the **conflict-detection / reverse-lookup**
    /// primitive. The `app` layer calls this per key press to route it to its action; the rebind flow
    /// calls it to find which action already owns a candidate key. `None` means the key is unbound.
    pub fn action_for(&self, key: KeyId) -> Option<GameAction> {
        GameAction::ALL
            .into_iter()
            .find(|&a| self.keys[a.index()] == key)
    }

    /// Attempt to bind `action` to `key`, upholding the no-shared-keys invariant. Returns
    /// [`RebindOutcome::Unchanged`] if it already holds the key, [`RebindOutcome::Conflict`] (leaving
    /// the map untouched) if another action owns it, else assigns it and returns
    /// [`RebindOutcome::Bound`]. Pure — the Settings capture flow calls this after the `app` boundary
    /// resolves the pressed key to a [`KeyId`].
    pub fn rebind(&mut self, action: GameAction, key: KeyId) -> RebindOutcome {
        if self.keys[action.index()] == key {
            return RebindOutcome::Unchanged;
        }
        // `action_for` can only report a *different* action here: the same-action case returned above.
        if let Some(owner) = self.action_for(key) {
            return RebindOutcome::Conflict(owner);
        }
        self.keys[action.index()] = key;
        RebindOutcome::Bound
    }

    /// Restore every action to its shipped default binding — the rebind editor's reset-to-defaults.
    pub fn reset(&mut self) {
        *self = KeybindMap::default();
    }

    /// Whether any two actions share a key (the invariant this type upholds should make this always
    /// `false` for a live map). Used only by [`decode`](Self::decode) to reject a hand-corrupted blob.
    fn has_conflict(&self) -> bool {
        for i in 0..self.keys.len() {
            for j in (i + 1)..self.keys.len() {
                if self.keys[i] == self.keys[j] {
                    return true;
                }
            }
        }
        false
    }

    /// Encode the map to a compact, stable string: each action's [`KeyId`] ordinal in
    /// [`GameAction::ALL`] order, comma-separated (e.g. `"48,10,2"` for the Esc/F11/F3 defaults). The
    /// counterpart of `shell`'s ordinal fields; the shell-prefs codec stores this as one value. A
    /// save→load round-trip is stable because every field is a stable ordinal.
    pub fn encode(&self) -> String {
        GameAction::ALL
            .iter()
            .map(|a| self.key_for(*a).index().to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Tolerantly decode an [`encode`](Self::encode) string back to a map. Every field that is missing,
    /// unparseable, or an out-of-range ordinal keeps *that action's* shipped default; then, if the
    /// result would violate the no-shared-keys invariant (only reachable from a hand-edited/corrupt
    /// blob), the whole map falls back to defaults. This **never panics** — an empty or garbage blob
    /// decodes to the shipped defaults, mirroring the shell codec's corruption-safety contract.
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
        // A duplicate can only come from a corrupt/hand-edited blob (encode of a live map is always
        // conflict-free). Reject the whole thing to a known-good default rather than ship a map that
        // silently steals a key from another action.
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
        assert_eq!(map.key_for(GameAction::Pause), KeyId::Escape);
        assert_eq!(map.key_for(GameAction::ToggleFullscreen), KeyId::F11);
        assert_eq!(map.key_for(GameAction::ToggleDebugOverlay), KeyId::F3);
        assert!(!map.has_conflict(), "shipped defaults must not share a key");
    }

    #[test]
    fn action_for_is_the_reverse_of_key_for() {
        let map = KeybindMap::default();
        for a in GameAction::ALL {
            assert_eq!(map.action_for(map.key_for(a)), Some(a));
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
    fn reset_restores_defaults() {
        let mut map = KeybindMap::default();
        map.rebind(GameAction::Pause, KeyId::P);
        map.rebind(GameAction::ToggleDebugOverlay, KeyId::G);
        map.reset();
        assert_eq!(map, KeybindMap::default());
    }

    #[test]
    fn encode_decode_round_trips_defaults_and_a_remapped_map() {
        // Defaults round-trip.
        let def = KeybindMap::default();
        assert_eq!(KeybindMap::decode(&def.encode()), def);

        // A remapped map round-trips too — the identity contract.
        let mut map = KeybindMap::default();
        assert_eq!(map.rebind(GameAction::Pause, KeyId::P), RebindOutcome::Bound);
        assert_eq!(
            map.rebind(GameAction::ToggleDebugOverlay, KeyId::G),
            RebindOutcome::Bound
        );
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
        //   field 1 = "16"   (KeyId::E, valid) → Fullscreen becomes E
        //   field 2 = ""     (missing) → DebugOverlay keeps F3
        let m = KeybindMap::decode("9999,16,");
        assert_eq!(m.key_for(GameAction::Pause), KeyId::Escape);
        assert_eq!(m.key_for(GameAction::ToggleFullscreen), KeyId::E);
        assert_eq!(m.key_for(GameAction::ToggleDebugOverlay), KeyId::F3);
    }

    #[test]
    fn decode_rejects_a_duplicate_key_blob_to_defaults() {
        // A hand-edited blob that binds two actions to the same ordinal (Esc = 48) is corrupt: the
        // whole map falls back to defaults rather than shipping a shared-key map.
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
