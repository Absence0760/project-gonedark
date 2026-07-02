//! HUD layout editor (PvE-campaign plan WS-D) — the per-layer drag / resize / opacity editor that
//! sits **on top of** the existing touch seams (`touch_controls` → intents + geometry; the
//! `render::touch_controls` screen-space pass, D51). It lets a player re-anchor, resize, and fade
//! the on-screen controls per layer (command vs embodied), keep several saved presets, and reset to
//! the shipped default.
//!
//! ## What this is — and is NOT (the load-bearing scope)
//! - **Presentation / input only, NEVER sim** (D61). A saved layout changes *where a finger has to
//!   land* and *how bright a control draws* — it never reaches `core`, never folds into the per-tick
//!   checksum, never invents a command. The intents it ultimately produces are the SAME ones the
//!   stock layout produces; only the screen geometry feeding the pure `TouchControls::update` seam
//!   moves. So this module adds **zero** lockstep / cross-arch surface (invariant #1/#2/#7).
//! - **Placement, never information (invariant #6).** The editor configures *where controls sit*. It
//!   can NEVER surface strategic intel while embodied: an element that reveals the map / unit roster
//!   / economy ([`HudElement::surfaces_strategic_intel`]) is structurally not editable in the
//!   [`HudLayer::Embodied`] layer — [`HudPreset::set_placement`] *rejects* it, so "the world goes
//!   dark" stays fair by construction, not by discipline. Accessibility cues (colour-blind palettes,
//!   text scale) are a SEPARATE settings surface and out of scope here.
//!
//! ## The pure seam (host-testable, no winit / Android types)
//! [`HudLayoutProfile::resolve_embodied`] maps the active preset's embodied layer → a concrete
//! [`TouchLayout`] (control geometry) + per-control [`Opacity`]. That [`TouchLayout`] feeds the
//! existing [`TouchControls::update`](crate::touch_controls::TouchControls::update) unchanged, so a
//! saved layout drives the raw-touch→intent mapping with no new code path. A profile with no
//! overrides resolves **bit-identically** to the shipped [`TouchLayout::new`] (resolution only
//! mutates elements that carry an explicit override), so enabling the editor changes nothing until
//! the player actually moves something.
//!
//! Floats are fine here for the same reason they are in `touch_controls`: this is host-side
//! presentation/input, the platform side of the PAL seam, quantized to `Fixed` *later* by the
//! `fire`/`locomote` seams (invariant #1).

use crate::touch_controls::TouchLayout;
use std::collections::BTreeMap;

/// The two mutually-exclusive HUD layers — the RTS command view and the embodied FPS view. The
/// editor keeps an independent layout for each (the thumb cluster you want while shooting is not the
/// one you want while commanding).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HudLayer {
    Command,
    Embodied,
}

/// Every element the editor can re-place, across both layers. Split three ways by what an element
/// *is*, which determines where it may be edited and whether it is allowed under avatar-only fog:
/// - **Embodied input controls** — pure controls, zero information.
/// - **Embodied-safe overlays** — alerts + objective text: feedback/orders, NOT strategic intel
///   (the only honest "thread back" while dark, game-design §6 / invariant #6).
/// - **Command-layer surfaces** — some of which *are* strategic intel and so are forbidden while
///   embodied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HudElement {
    // --- Embodied input controls (placement only, no information) ---
    MoveStick,
    Fire,
    Crouch,
    Reload,
    Surface,
    // --- Embodied-safe overlays (presentation, never strategic intel) ---
    AlertIndicator,
    ObjectiveTracker,
    // --- Command-layer surfaces (Minimap/Roster/Economy ARE strategic intel) ---
    CommandPalette,
    Minimap,
    UnitRoster,
    ResourceReadout,
}

impl HudElement {
    /// Every editable element, in a stable order (used by the editor UI + the fairness sweep).
    pub const ALL: [HudElement; 11] = [
        HudElement::MoveStick,
        HudElement::Fire,
        HudElement::Crouch,
        HudElement::Reload,
        HudElement::Surface,
        HudElement::AlertIndicator,
        HudElement::ObjectiveTracker,
        HudElement::CommandPalette,
        HudElement::Minimap,
        HudElement::UnitRoster,
        HudElement::ResourceReadout,
    ];

    /// The five embodied touch controls that resolve to concrete [`TouchLayout`] geometry — the only
    /// elements wired to an *existing* touch seam (WS-D touches nothing else).
    pub const EMBODIED_CONTROLS: [HudElement; 5] = [
        HudElement::MoveStick,
        HudElement::Fire,
        HudElement::Crouch,
        HudElement::Reload,
        HudElement::Surface,
    ];

    /// Does this element reveal **strategic intel** (the map, the unit roster, the economy)? Such an
    /// element going dark is the whole fairness bet (invariant #6) — it may never be editable in the
    /// embodied layer. Alerts + objective text are deliberately NOT intel.
    pub fn surfaces_strategic_intel(self) -> bool {
        matches!(
            self,
            HudElement::Minimap | HudElement::UnitRoster | HudElement::ResourceReadout
        )
    }

    /// May this element be placed/edited in `layer`? Input controls live only in the embodied layer;
    /// the command palette + intel surfaces only in the command layer; alerts + the objective
    /// tracker exist in both. **The invariant-#6 guard is structural here:** nothing that
    /// [`surfaces_strategic_intel`](Self::surfaces_strategic_intel) is editable while embodied.
    pub fn editable_in(self, layer: HudLayer) -> bool {
        match self {
            HudElement::MoveStick
            | HudElement::Fire
            | HudElement::Crouch
            | HudElement::Reload
            | HudElement::Surface => layer == HudLayer::Embodied,
            HudElement::AlertIndicator | HudElement::ObjectiveTracker => true,
            HudElement::CommandPalette
            | HudElement::Minimap
            | HudElement::UnitRoster
            | HudElement::ResourceReadout => layer == HudLayer::Command,
        }
    }

    /// The shipped default placement for this element (the fractions baked into [`TouchLayout::new`]
    /// for the embodied controls; sensible command-layer anchors for the rest). The editor seeds a
    /// drag from here; resolution leaves an un-overridden element on its stock geometry.
    pub fn default_placement(self) -> Placement {
        // Centers are fractions of the viewport; these mirror `TouchLayout::new` so the editor's
        // "reset" point and the renderer's stock layout can never drift (asserted in tests).
        let (cx, cy) = match self {
            // Fixed stick ring anchor — mirrors `TouchLayout::new`'s `stick_base` centre.
            HudElement::MoveStick => (0.15, 0.72),
            HudElement::Fire => (0.84, 0.74),
            HudElement::Crouch => (0.70, 0.86),
            HudElement::Reload => (0.95, 0.50),
            HudElement::Surface => (0.94, 0.08),
            HudElement::AlertIndicator => (0.50, 0.10),
            HudElement::ObjectiveTracker => (0.50, 0.05),
            HudElement::CommandPalette => (0.50, 0.92),
            HudElement::Minimap => (0.12, 0.18),
            HudElement::UnitRoster => (0.50, 0.95),
            HudElement::ResourceReadout => (0.90, 0.05),
        };
        Placement {
            center: (cx, cy),
            scale: 1.0,
            opacity: 1.0,
        }
    }

    /// Stable wire token for [`HudLayoutProfile`] serialization.
    fn token(self) -> &'static str {
        match self {
            HudElement::MoveStick => "move_stick",
            HudElement::Fire => "fire",
            HudElement::Crouch => "crouch",
            HudElement::Reload => "reload",
            HudElement::Surface => "surface",
            HudElement::AlertIndicator => "alert",
            HudElement::ObjectiveTracker => "objective",
            HudElement::CommandPalette => "palette",
            HudElement::Minimap => "minimap",
            HudElement::UnitRoster => "roster",
            HudElement::ResourceReadout => "resources",
        }
    }

    fn from_token(s: &str) -> Option<HudElement> {
        HudElement::ALL.into_iter().find(|e| e.token() == s)
    }
}

/// A single element's placement override: where its center sits (as a fraction of the viewport,
/// `+x` right / `+y` down — the touch screen convention), how big it is relative to its default
/// size, and how opaque it draws. Pure presentation/input data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Center, as a fraction of the viewport in each axis.
    pub center: (f32, f32),
    /// Size multiplier on the element's default radius/extent.
    pub scale: f32,
    /// Draw opacity in `[0, 1]`.
    pub opacity: f32,
}

/// The smallest scale an **embodied input control** may be edited/saved to. The generic
/// [`Placement::clamped`] floor (0.25×) is fine for cosmetic overlays, but a touch button shrunk to
/// a quarter size is barely tappable — and a barely-tappable control while blind is exactly the
/// invariant-#6 "the game robbed me" failure, just self-inflicted through the editor.
const EMBODIED_CONTROL_MIN_SCALE: f32 = 0.6;
/// The minimum opacity the **Surface** (panic-exit) button may be edited/saved to. Every other
/// control can be faded fully out if a player wants a minimal HUD, but the one control that returns
/// you to command — the thing you reach for when you're losing and need to bail — must always be
/// visible enough to locate. (Its hit target survives regardless via the scale floor above; this is
/// the "can I *see* it" half.)
const SURFACE_MIN_OPACITY: f32 = 0.35;

impl Placement {
    /// Clamp to sane editor bounds (center on-screen, scale within a usable band, opacity `[0,1]`).
    /// Keeps a saved/edited layout from putting a control off-screen or at zero size.
    pub fn clamped(self) -> Placement {
        let cl = |v: f32, lo: f32, hi: f32| v.max(lo).min(hi);
        Placement {
            center: (cl(self.center.0, 0.0, 1.0), cl(self.center.1, 0.0, 1.0)),
            scale: cl(self.scale, 0.25, 4.0),
            opacity: cl(self.opacity, 0.0, 1.0),
        }
    }

    /// [`clamped`](Self::clamped) plus stricter, element-aware floors for the **embodied input
    /// controls**: they must stay usable no matter what a saved (or hand-edited) layout requests.
    /// Input controls get a higher minimum scale so they stay tappable, and the **Surface** button
    /// additionally gets a minimum opacity so the panic-exit is always locatable. Cosmetic overlays
    /// (alerts, objective tracker) keep the looser [`clamped`](Self::clamped) bound. This closes the
    /// self-inflicted invariant-#6 gap: a player can't accidentally save a layout that makes their
    /// only "return to command" control tiny or invisible.
    pub fn clamped_for(self, element: HudElement) -> Placement {
        let mut p = self.clamped();
        if HudElement::EMBODIED_CONTROLS.contains(&element) {
            p.scale = p.scale.max(EMBODIED_CONTROL_MIN_SCALE);
            if element == HudElement::Surface {
                p.opacity = p.opacity.max(SURFACE_MIN_OPACITY);
            }
        }
        p
    }
}

/// One layer's overrides — only elements the player actually moved are stored; everything else
/// resolves to its [`default_placement`](HudElement::default_placement). An empty map == the
/// shipped layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerConfig {
    overrides: BTreeMap<HudElement, Placement>,
}

impl LayerConfig {
    /// The effective placement for `element`: its override if set, else the stock default.
    pub fn placement(&self, element: HudElement) -> Placement {
        self.overrides
            .get(&element)
            .copied()
            .unwrap_or_else(|| element.default_placement())
    }

    /// Iterate the explicit overrides (resolution touches only these, preserving stock geometry for
    /// the rest).
    pub fn overrides(&self) -> impl Iterator<Item = (HudElement, Placement)> + '_ {
        self.overrides.iter().map(|(&e, &p)| (e, p))
    }

    /// Whether this layer carries any override (i.e. differs from the shipped default).
    pub fn is_default(&self) -> bool {
        self.overrides.is_empty()
    }
}

/// What can go wrong when editing a placement — only the fairness/scoping rejections (placement
/// values themselves are clamped, never rejected).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudEditError {
    /// The element is not editable in that layer (e.g. the Fire button in the command layer).
    NotInLayer,
    /// **Invariant #6:** a strategic-intel element may never be placed in the embodied layer.
    IntelForbiddenWhileEmbodied,
}

/// A named, complete HUD layout: an independent [`LayerConfig`] for each layer. The unit the editor
/// saves, names, and reloads.
#[derive(Clone, Debug, PartialEq)]
pub struct HudPreset {
    pub name: String,
    command: LayerConfig,
    embodied: LayerConfig,
}

impl HudPreset {
    /// A fresh preset on the shipped default layout for both layers.
    pub fn new(name: impl Into<String>) -> Self {
        HudPreset {
            name: name.into(),
            command: LayerConfig::default(),
            embodied: LayerConfig::default(),
        }
    }

    fn layer(&self, layer: HudLayer) -> &LayerConfig {
        match layer {
            HudLayer::Command => &self.command,
            HudLayer::Embodied => &self.embodied,
        }
    }

    /// The effective placement for `element` in `layer`.
    pub fn placement(&self, layer: HudLayer, element: HudElement) -> Placement {
        self.layer(layer).placement(element)
    }

    /// Drag/resize/fade an element in a layer. Validates the two fairness rules (invariant #6) and
    /// clamps the placement; returns the rejection otherwise. This is the editor's ONLY mutation
    /// path, so the guarantees hold for every saved layout.
    pub fn set_placement(
        &mut self,
        layer: HudLayer,
        element: HudElement,
        placement: Placement,
    ) -> Result<(), HudEditError> {
        if !element.editable_in(layer) {
            return Err(HudEditError::NotInLayer);
        }
        if layer == HudLayer::Embodied && element.surfaces_strategic_intel() {
            // Unreachable given `editable_in` already excludes intel from the embodied layer, but
            // kept as an explicit, independent guard on the load-bearing invariant.
            return Err(HudEditError::IntelForbiddenWhileEmbodied);
        }
        let target = match layer {
            HudLayer::Command => &mut self.command,
            HudLayer::Embodied => &mut self.embodied,
        };
        target.overrides.insert(element, placement.clamped_for(element));
        Ok(())
    }

    /// Drop a single element's override (back to its shipped default).
    pub fn clear(&mut self, layer: HudLayer, element: HudElement) {
        let target = match layer {
            HudLayer::Command => &mut self.command,
            HudLayer::Embodied => &mut self.embodied,
        };
        target.overrides.remove(&element);
    }

    /// Reset BOTH layers to the shipped default (keeps the name).
    pub fn reset_to_default(&mut self) {
        self.command = LayerConfig::default();
        self.embodied = LayerConfig::default();
    }

    /// Whether this preset is the shipped default on both layers.
    pub fn is_default(&self) -> bool {
        self.command.is_default() && self.embodied.is_default()
    }
}

/// Per-control draw opacity resolved for a frame (multiplies the renderer's base alpha). 1.0 == the
/// shipped look. Separate from geometry so the [`render::touch_controls`](gonedark_render) pass can
/// stay a thin consumer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opacity {
    pub stick: f32,
    pub fire: f32,
    pub crouch: f32,
    pub reload: f32,
    pub surface: f32,
}

impl Default for Opacity {
    fn default() -> Self {
        Opacity {
            stick: 1.0,
            fire: 1.0,
            crouch: 1.0,
            reload: 1.0,
            surface: 1.0,
        }
    }
}

/// The embodied layer resolved to concrete geometry + opacity for one viewport — the output of the
/// pure seam. `layout` feeds [`TouchControls::update`](crate::touch_controls::TouchControls::update)
/// unchanged; `opacity` feeds the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedEmbodiedHud {
    pub layout: TouchLayout,
    pub opacity: Opacity,
}

/// The editor's persisted state: several saved [`HudPreset`]s + which one is active. Lives in
/// local/profile config (host-side, OUTSIDE the checksum). Serialized via a small hand-rolled text
/// codec (no new deps; `core` stays dependency-free, the host stays serde-free — the same byte
/// discipline as the rest of the repo).
#[derive(Clone, Debug, PartialEq)]
pub struct HudLayoutProfile {
    presets: Vec<HudPreset>,
    active: usize,
}

impl Default for HudLayoutProfile {
    fn default() -> Self {
        HudLayoutProfile {
            presets: vec![HudPreset::new("Default")],
            active: 0,
        }
    }
}

impl HudLayoutProfile {
    /// Number of saved presets (always ≥ 1).
    pub fn len(&self) -> usize {
        self.presets.len()
    }

    /// Always false — a profile always holds at least the Default preset.
    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    /// Index of the active preset.
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The active preset.
    pub fn active(&self) -> &HudPreset {
        &self.presets[self.active]
    }

    /// The active preset, mutably (the editor edits THROUGH this).
    pub fn active_mut(&mut self) -> &mut HudPreset {
        &mut self.presets[self.active]
    }

    /// All presets (for the select UI).
    pub fn presets(&self) -> &[HudPreset] {
        &self.presets
    }

    /// Add a new preset (cloned from the shipped default) and make it active. Returns its index.
    pub fn add_preset(&mut self, name: impl Into<String>) -> usize {
        self.presets.push(HudPreset::new(name));
        self.active = self.presets.len() - 1;
        self.active
    }

    /// Save a fully-formed preset and make it active. Returns its index.
    pub fn push_preset(&mut self, preset: HudPreset) -> usize {
        self.presets.push(preset);
        self.active = self.presets.len() - 1;
        self.active
    }

    /// Make preset `idx` active (no-op if out of range).
    pub fn select(&mut self, idx: usize) -> bool {
        if idx < self.presets.len() {
            self.active = idx;
            true
        } else {
            false
        }
    }

    /// Remove preset `idx`; never removes the last one (a profile keeps ≥ 1). Re-clamps the active
    /// index. Returns whether it removed anything.
    pub fn remove(&mut self, idx: usize) -> bool {
        if self.presets.len() <= 1 || idx >= self.presets.len() {
            return false;
        }
        self.presets.remove(idx);
        if self.active >= self.presets.len() {
            self.active = self.presets.len() - 1;
        } else if self.active > idx {
            self.active -= 1;
        }
        true
    }

    /// Reset the active preset to the shipped default (the "reset to default" button).
    pub fn reset_active_to_default(&mut self) {
        self.presets[self.active].reset_to_default();
    }

    /// Resolve the active preset's **embodied** layer to concrete [`TouchLayout`] geometry + opacity
    /// for a `width × height` viewport. **The pure seam (WS-D).** Starts from the shipped
    /// [`TouchLayout::new`] and mutates ONLY elements carrying an explicit override, so a default
    /// profile resolves bit-identically to the stock layout.
    pub fn resolve_embodied(&self, width: u32, height: u32) -> ResolvedEmbodiedHud {
        resolve_embodied_layer(self.active().layer(HudLayer::Embodied), width, height)
    }

    /// Like [`resolve_embodied`](Self::resolve_embodied) but applies the physical touch-target floor
    /// for the display `density` (the PAL's `densityDpi / DENSITY_DEFAULT` scale). The live host uses
    /// this so touch controls clear the ~9 mm minimum on a dense phone.
    pub fn resolve_embodied_with_density(
        &self,
        width: u32,
        height: u32,
        density: f32,
    ) -> ResolvedEmbodiedHud {
        resolve_embodied_layer_with_density(
            self.active().layer(HudLayer::Embodied),
            width,
            height,
            density,
        )
    }

    // ---- Serialization (hand-rolled text codec; host-side, never the checksum) ----

    /// Serialize to the line-based config text persisted in the player profile.
    pub fn to_config_string(&self) -> String {
        let mut out = String::new();
        out.push_str("hud_layout v1\n");
        out.push_str(&format!("active {}\n", self.active));
        for preset in &self.presets {
            // Names can't contain a newline; tabs/leading-trailing space are trimmed on read.
            out.push_str(&format!("preset {}\n", preset.name.replace('\n', " ")));
            for (layer, cfg) in [
                (HudLayer::Command, &preset.command),
                (HudLayer::Embodied, &preset.embodied),
            ] {
                let tag = match layer {
                    HudLayer::Command => "command",
                    HudLayer::Embodied => "embodied",
                };
                for (element, p) in cfg.overrides() {
                    out.push_str(&format!(
                        "  {tag} {} {} {} {} {}\n",
                        element.token(),
                        p.center.0,
                        p.center.1,
                        p.scale,
                        p.opacity,
                    ));
                }
            }
        }
        out
    }

    /// Parse the persisted config text. Never panics: a malformed line is an error, so a corrupt
    /// profile falls back to the default rather than silently mis-placing controls.
    pub fn from_config_string(s: &str) -> Result<HudLayoutProfile, HudConfigError> {
        let mut presets: Vec<HudPreset> = Vec::new();
        let mut active = 0usize;
        let mut saw_header = false;

        for raw in s.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let kw = parts.next().unwrap_or("");
            // The version header must come first — anything before it is a malformed file.
            if !saw_header && kw != "hud_layout" {
                return Err(HudConfigError::BadHeader);
            }
            match kw {
                "hud_layout" => {
                    saw_header = parts.next() == Some("v1");
                    if !saw_header {
                        return Err(HudConfigError::BadHeader);
                    }
                }
                "active" => {
                    active = parts
                        .next()
                        .and_then(|v| v.parse().ok())
                        .ok_or(HudConfigError::BadField)?;
                }
                "preset" => {
                    // Everything after "preset " is the (possibly spaced) name.
                    let name = line["preset".len()..].trim().to_string();
                    presets.push(HudPreset::new(name));
                }
                "command" | "embodied" => {
                    let preset = presets.last_mut().ok_or(HudConfigError::OrphanOverride)?;
                    let layer = if kw == "command" {
                        HudLayer::Command
                    } else {
                        HudLayer::Embodied
                    };
                    let element = parts
                        .next()
                        .and_then(HudElement::from_token)
                        .ok_or(HudConfigError::BadField)?;
                    let mut num = || -> Result<f32, HudConfigError> {
                        parts
                            .next()
                            .and_then(|v| v.parse().ok())
                            .ok_or(HudConfigError::BadField)
                    };
                    let placement = Placement {
                        center: (num()?, num()?),
                        scale: num()?,
                        opacity: num()?,
                    };
                    // Re-validate through the same guard the editor uses: a hand-edited config can
                    // never smuggle an intel element into the embodied layer (invariant #6).
                    preset
                        .set_placement(layer, element, placement)
                        .map_err(HudConfigError::Rejected)?;
                }
                _ => return Err(HudConfigError::UnknownKeyword),
            }
        }

        if !saw_header {
            return Err(HudConfigError::BadHeader);
        }
        if presets.is_empty() {
            presets.push(HudPreset::new("Default"));
        }
        if active >= presets.len() {
            active = 0;
        }
        Ok(HudLayoutProfile { presets, active })
    }
}

/// A profile-config parse failure (mirrors `core::persist`'s "never produce a divergent state
/// silently" discipline — a bad file is an error to fall back on, not a corrupt layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudConfigError {
    BadHeader,
    BadField,
    UnknownKeyword,
    OrphanOverride,
    Rejected(HudEditError),
}

/// Resolve one embodied [`LayerConfig`] to [`TouchLayout`] geometry + [`Opacity`]. Only overridden
/// elements move; everything else keeps its stock geometry, so the default case is byte-identical to
/// [`TouchLayout::new`]. Free function so the seam is testable without a `Game`.
pub fn resolve_embodied_layer(
    cfg: &LayerConfig,
    width: u32,
    height: u32,
) -> ResolvedEmbodiedHud {
    resolve_embodied_over_base(cfg, TouchLayout::new(width, height), width, height)
}

/// Like [`resolve_embodied_layer`], but builds the base geometry with a physical touch-target floor
/// from the display `density` ([`TouchLayout::with_density`]) so buttons stay tappable on a dense
/// phone. This is the density-aware entry the live host uses; the density-less variant stays for the
/// pure tests (byte-identical to the stock layout).
pub fn resolve_embodied_layer_with_density(
    cfg: &LayerConfig,
    width: u32,
    height: u32,
    density: f32,
) -> ResolvedEmbodiedHud {
    resolve_embodied_over_base(cfg, TouchLayout::with_density(width, height, density), width, height)
}

/// Apply a layer's placement overrides on top of a supplied base [`TouchLayout`] — the shared body
/// of both resolve entry points (with/without the density floor), so the override math can't drift.
fn resolve_embodied_over_base(
    cfg: &LayerConfig,
    base: TouchLayout,
    width: u32,
    height: u32,
) -> ResolvedEmbodiedHud {
    let mut layout = base;
    let mut opacity = Opacity::default();
    let w = width.max(1) as f32;
    let h = height.max(1) as f32;

    for (element, p) in cfg.overrides() {
        // Re-apply the element-aware floors here too: a hand-edited/corrupt config can reach this
        // resolve path without going through `set_placement`, so the Surface-visibility / control-
        // tappability guarantees (invariant #6) must hold at resolution, not just at edit time.
        let p = p.clamped_for(element);
        let cx = p.center.0 * w;
        let cy = p.center.1 * h;
        match element {
            HudElement::MoveStick => {
                // Re-anchor the fixed stick ring to the new centre and scale its radius (the ring is
                // both the visible target and the max-deflection distance).
                layout.stick_base.cx = cx;
                layout.stick_base.cy = cy;
                layout.stick_base.r *= p.scale;
                opacity.stick = p.opacity;
            }
            HudElement::Fire => {
                layout.fire.cx = cx;
                layout.fire.cy = cy;
                layout.fire.r *= p.scale;
                opacity.fire = p.opacity;
            }
            HudElement::Crouch => {
                layout.crouch.cx = cx;
                layout.crouch.cy = cy;
                layout.crouch.r *= p.scale;
                opacity.crouch = p.opacity;
            }
            HudElement::Reload => {
                layout.reload.cx = cx;
                layout.reload.cy = cy;
                layout.reload.r *= p.scale;
                opacity.reload = p.opacity;
            }
            HudElement::Surface => {
                layout.surface.cx = cx;
                layout.surface.cy = cy;
                layout.surface.r *= p.scale;
                opacity.surface = p.opacity;
            }
            // Non-control elements (alerts/objective/command surfaces) carry no `TouchLayout`
            // geometry — they are not part of the embodied touch seam. Ignored here by design.
            _ => {}
        }
    }

    ResolvedEmbodiedHud { layout, opacity }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::touch_controls::TouchControls;
    use gonedark_pal::TouchSample;

    const W: u32 = 1280;
    const H: u32 = 720;

    fn t(id: u64, x: f32, y: f32) -> TouchSample {
        TouchSample { id, x, y }
    }

    // ---- The pure seam: saved layout → geometry ----

    #[test]
    fn default_profile_resolves_bit_identically_to_stock_layout() {
        let profile = HudLayoutProfile::default();
        let resolved = profile.resolve_embodied(W, H);
        assert_eq!(
            resolved.layout,
            TouchLayout::new(W, H),
            "an un-edited profile must not perturb the shipped layout"
        );
        assert_eq!(resolved.opacity, Opacity::default());
    }

    #[test]
    fn default_placements_match_the_stock_touch_layout() {
        // The editor's "reset point" fractions must match `TouchLayout::new`, or reset would jump
        // controls. Resolve a layer that overrides each control with its OWN default → no movement.
        let stock = TouchLayout::new(W, H);
        for element in HudElement::EMBODIED_CONTROLS {
            let mut cfg = LayerConfig::default();
            cfg.overrides
                .insert(element, element.default_placement());
            let r = resolve_embodied_layer(&cfg, W, H);
            match element {
                HudElement::Fire => {
                    assert!((r.layout.fire.cx - stock.fire.cx).abs() < 0.5);
                    assert!((r.layout.fire.cy - stock.fire.cy).abs() < 0.5);
                    assert!((r.layout.fire.r - stock.fire.r).abs() < 1e-3);
                }
                HudElement::Crouch => {
                    assert!((r.layout.crouch.cx - stock.crouch.cx).abs() < 0.5);
                    assert!((r.layout.crouch.cy - stock.crouch.cy).abs() < 0.5);
                }
                HudElement::Reload => {
                    assert!((r.layout.reload.cx - stock.reload.cx).abs() < 0.5);
                    assert!((r.layout.reload.cy - stock.reload.cy).abs() < 0.5);
                }
                HudElement::Surface => {
                    assert!((r.layout.surface.cx - stock.surface.cx).abs() < 0.5);
                    assert!((r.layout.surface.cy - stock.surface.cy).abs() < 0.5);
                }
                HudElement::MoveStick => {
                    // Stick ring centre + radius match the stock ring.
                    assert!((r.layout.stick_base.cx - stock.stick_base.cx).abs() < 0.5);
                    assert!((r.layout.stick_base.cy - stock.stick_base.cy).abs() < 0.5);
                    assert!((r.layout.stick_base.r - stock.stick_base.r).abs() < 1e-3);
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn resizing_a_button_scales_its_radius() {
        let mut preset = HudPreset::new("p");
        let mut p = HudElement::Fire.default_placement();
        p.scale = 2.0;
        preset.set_placement(HudLayer::Embodied, HudElement::Fire, p).unwrap();
        let mut profile = HudLayoutProfile::default();
        profile.push_preset(preset);
        let r = profile.resolve_embodied(W, H);
        let stock = TouchLayout::new(W, H);
        assert!((r.layout.fire.r - stock.fire.r * 2.0).abs() < 1e-3);
    }

    #[test]
    fn opacity_override_flows_into_the_resolved_opacity() {
        let mut preset = HudPreset::new("dim");
        let mut p = HudElement::Fire.default_placement();
        p.opacity = 0.3;
        preset.set_placement(HudLayer::Embodied, HudElement::Fire, p).unwrap();
        let mut profile = HudLayoutProfile::default();
        profile.push_preset(preset);
        let r = profile.resolve_embodied(W, H);
        assert!((r.opacity.fire - 0.3).abs() < 1e-6);
        assert_eq!(r.opacity.crouch, 1.0, "un-edited controls stay fully opaque");
    }

    // ---- The pure seam: saved layout → raw-touch → intent ----

    #[test]
    fn moving_the_fire_button_moves_where_a_tap_fires() {
        // Re-anchor Fire to the lower-LEFT (well clear of its stock lower-right spot), then a tap at
        // the NEW location must fire and a tap at the OLD location must not.
        let stock = TouchLayout::new(W, H);
        let (old_x, old_y) = (stock.fire.cx, stock.fire.cy);

        let mut preset = HudPreset::new("lefty");
        let mut p = HudElement::Fire.default_placement();
        // Upper area of the right (look) half — clear of the stock lower-right spot, and NOT inside
        // the left stick zone (a finger there would be claimed by the move stick, not the button).
        p.center = (0.60, 0.25);
        preset.set_placement(HudLayer::Embodied, HudElement::Fire, p).unwrap();
        let mut profile = HudLayoutProfile::default();
        profile.push_preset(preset);

        let resolved = profile.resolve_embodied(W, H);
        let new_x = resolved.layout.fire.cx;
        let new_y = resolved.layout.fire.cy;
        assert!((new_x - old_x).abs() > 100.0, "the button actually moved");

        // Feed the SAME resolved layout that the renderer would draw through the existing input seam.
        let mut tc = TouchControls::new();
        let out = tc.update(&resolved.layout, &[t(1, new_x, new_y)]);
        assert!(out.fire, "a tap at the relocated Fire button fires");

        let mut tc2 = TouchControls::new();
        let out2 = tc2.update(&resolved.layout, &[t(1, old_x, old_y)]);
        assert!(!out2.fire, "a tap where Fire USED to be no longer fires");
    }

    #[test]
    fn relocating_the_stick_ring_moves_where_the_move_stick_claims() {
        // Put the stick ring in the upper-right corner; a finger at the new centre should now drive
        // the stick, and a finger at the stock lower-left ring should not.
        let stock = TouchLayout::new(W, H);
        let mut preset = HudPreset::new("flip");
        let mut p = HudElement::MoveStick.default_placement();
        p.center = (0.80, 0.20);
        preset.set_placement(HudLayer::Embodied, HudElement::MoveStick, p).unwrap();
        let mut profile = HudLayoutProfile::default();
        profile.push_preset(preset);
        let resolved = profile.resolve_embodied(W, H);

        let ring = resolved.layout.stick_base;
        // The relocated ring really is in the upper-right (away from its stock lower-left home).
        assert!(ring.cx > 0.5 * W as f32 && ring.cy < 0.5 * H as f32);

        let mut tc = TouchControls::new();
        let out = tc.update(&resolved.layout, &[t(1, ring.cx, ring.cy)]);
        assert!(out.hud.stick_active, "finger in the relocated ring claims the stick");

        // A finger at the ring's OLD stock home no longer claims the stick.
        let mut tc2 = TouchControls::new();
        let out2 = tc2.update(&resolved.layout, &[t(1, stock.stick_base.cx, stock.stick_base.cy)]);
        assert!(!out2.hud.stick_active, "the stock lower-left spot no longer claims the stick");
    }

    // ---- Invariant #6: placement not information ----

    #[test]
    fn no_embodied_editable_element_surfaces_strategic_intel() {
        // The load-bearing fairness guard: every element editable while embodied is information-free.
        for element in HudElement::ALL {
            if element.editable_in(HudLayer::Embodied) {
                assert!(
                    !element.surfaces_strategic_intel(),
                    "{element:?} is editable while embodied but surfaces strategic intel — \
                     that breaks 'the world goes dark' (invariant #6)"
                );
            }
        }
    }

    #[test]
    fn intel_elements_cannot_be_placed_in_the_embodied_layer() {
        let mut preset = HudPreset::new("cheat");
        for intel in [HudElement::Minimap, HudElement::UnitRoster, HudElement::ResourceReadout] {
            assert!(intel.surfaces_strategic_intel());
            let err = preset
                .set_placement(HudLayer::Embodied, intel, intel.default_placement())
                .unwrap_err();
            assert_eq!(err, HudEditError::NotInLayer);
        }
        // And the embodied layer stays empty — nothing leaked in.
        assert!(preset.embodied.is_default());
    }

    #[test]
    fn embodied_controls_keep_a_usable_size_floor_when_edited_tiny() {
        // A player can't save an embodied input control shrunk below the usable-scale floor — a
        // barely-tappable touch control while blind is the invariant-#6 "the game robbed me"
        // failure, self-inflicted. The generic clamp floor (0.25) is overridden to 0.6 for controls.
        let mut preset = HudPreset::new("tiny");
        let tiny = Placement {
            center: (0.5, 0.5),
            scale: 0.01,
            opacity: 1.0,
        };
        for ctrl in HudElement::EMBODIED_CONTROLS {
            preset.set_placement(HudLayer::Embodied, ctrl, tiny).unwrap();
        }
        let resolved = resolve_embodied_layer(&preset.embodied, 1280, 720);
        // Every control's radius stays at least the floor fraction of its stock size (never the
        // 0.01 the player asked for). We check via the stored override's clamped scale.
        for ctrl in HudElement::EMBODIED_CONTROLS {
            let got = preset
                .embodied
                .overrides()
                .find(|(e, _)| *e == ctrl)
                .map(|(_, p)| p.scale)
                .expect("override stored");
            assert!(
                got >= EMBODIED_CONTROL_MIN_SCALE - 1e-6,
                "{ctrl:?} scale {got} floored to {EMBODIED_CONTROL_MIN_SCALE}"
            );
        }
        // And the resolved layout is a real, non-degenerate touch layout (radii > 0).
        assert!(resolved.layout.surface.r > 0.0);
        assert!(resolved.layout.fire.r > 0.0);
    }

    #[test]
    fn density_aware_resolve_floors_touch_targets_on_a_dense_screen() {
        // The live host resolves via `resolve_embodied_with_density`, which must apply the physical
        // mm touch-target floor — so on a small, dense phone the buttons come out LARGER than the
        // bare-fraction (`resolve_embodied`, density-less) layout. A default profile (no overrides).
        let profile = HudLayoutProfile::default();
        let (w, h, density) = (720, 360, 3.0);
        let bare = profile.resolve_embodied(w, h);
        let floored = profile.resolve_embodied_with_density(w, h, density);
        // Every round control is at least as large, and the small ones (Surface) strictly larger.
        assert!(floored.layout.surface.r > bare.layout.surface.r, "dense floor grows Surface");
        assert!(floored.layout.fire.r >= bare.layout.fire.r);
        // Density 1.0 (a non-dense display) reproduces the bare layout — no gratuitous change.
        let unit = profile.resolve_embodied_with_density(1280, 720, 1.0);
        let bare_hd = profile.resolve_embodied(1280, 720);
        assert_eq!(unit.layout.surface.r, bare_hd.layout.surface.r);
    }

    #[test]
    fn surface_button_cannot_be_faded_invisible() {
        // The Surface (panic-exit) button additionally floors its opacity so it's always locatable —
        // you must be able to find your way back to command when you're losing (invariant #6).
        let mut preset = HudPreset::new("ghost");
        let invisible = Placement {
            center: (0.94, 0.08),
            scale: 1.0,
            opacity: 0.0,
        };
        preset
            .set_placement(HudLayer::Embodied, HudElement::Surface, invisible)
            .unwrap();
        let resolved = resolve_embodied_layer(&preset.embodied, 1280, 720);
        assert!(
            resolved.opacity.surface >= SURFACE_MIN_OPACITY - 1e-6,
            "Surface opacity {} floored to {SURFACE_MIN_OPACITY}",
            resolved.opacity.surface
        );
        // A non-panic control (Fire) may still be faded fully out for a minimal HUD.
        let mut preset2 = HudPreset::new("minimal");
        preset2
            .set_placement(
                HudLayer::Embodied,
                HudElement::Fire,
                Placement {
                    center: (0.84, 0.74),
                    scale: 1.0,
                    opacity: 0.0,
                },
            )
            .unwrap();
        let r2 = resolve_embodied_layer(&preset2.embodied, 1280, 720);
        assert_eq!(r2.opacity.fire, 0.0, "non-panic controls keep the looser bound");
    }

    #[test]
    fn embodied_controls_cannot_be_placed_in_the_command_layer() {
        let mut preset = HudPreset::new("x");
        let err = preset
            .set_placement(HudLayer::Command, HudElement::Fire, HudElement::Fire.default_placement())
            .unwrap_err();
        assert_eq!(err, HudEditError::NotInLayer);
    }

    // ---- Presets + reset-to-default ----

    #[test]
    fn presets_add_select_and_reset_to_default() {
        let mut profile = HudLayoutProfile::default();
        assert_eq!(profile.len(), 1);

        let idx = profile.add_preset("Southpaw");
        assert_eq!(idx, 1);
        assert_eq!(profile.active_index(), 1);

        // Edit the active preset, then reset it.
        let mut p = HudElement::Fire.default_placement();
        p.center = (0.2, 0.8);
        profile
            .active_mut()
            .set_placement(HudLayer::Embodied, HudElement::Fire, p)
            .unwrap();
        assert!(!profile.active().is_default());
        profile.reset_active_to_default();
        assert!(profile.active().is_default(), "reset restores the shipped layout");

        // Select back to the original.
        assert!(profile.select(0));
        assert_eq!(profile.active_index(), 0);
        assert!(!profile.select(99), "out-of-range select is a no-op");
    }

    #[test]
    fn remove_keeps_at_least_one_preset_and_reclamps_active() {
        let mut profile = HudLayoutProfile::default();
        profile.add_preset("a");
        profile.add_preset("b"); // active = 2
        assert_eq!(profile.len(), 3);
        assert!(profile.remove(2));
        assert_eq!(profile.active_index(), 1, "active re-clamped after removing it");
        assert!(profile.remove(0));
        assert!(!profile.remove(0), "never removes the last surviving preset");
        assert_eq!(profile.len(), 1);
    }

    // ---- Persistence round-trip ----

    #[test]
    fn config_round_trips_through_text() {
        let mut profile = HudLayoutProfile::default();
        let mut p = HudElement::Fire.default_placement();
        p.center = (0.31, 0.77);
        p.scale = 1.5;
        p.opacity = 0.6;
        profile
            .active_mut()
            .set_placement(HudLayer::Embodied, HudElement::Fire, p)
            .unwrap();
        // A command-layer edit (intel element legitimately allowed there).
        profile.add_preset("Tablet");
        let mut mp = HudElement::Minimap.default_placement();
        mp.center = (0.15, 0.2);
        profile
            .active_mut()
            .set_placement(HudLayer::Command, HudElement::Minimap, mp)
            .unwrap();

        let text = profile.to_config_string();
        let back = HudLayoutProfile::from_config_string(&text).unwrap();
        assert_eq!(back, profile, "profile survives a text round-trip");
    }

    #[test]
    fn parsing_rejects_an_intel_element_smuggled_into_the_embodied_layer() {
        // A hand-edited/corrupt config can't bypass the fairness guard.
        let text = "hud_layout v1\nactive 0\npreset Hacked\n  embodied minimap 0.1 0.1 1 1\n";
        let err = HudLayoutProfile::from_config_string(text).unwrap_err();
        assert_eq!(err, HudConfigError::Rejected(HudEditError::NotInLayer));
    }

    #[test]
    fn parsing_a_bad_header_is_an_error_not_a_panic() {
        assert_eq!(
            HudLayoutProfile::from_config_string("garbage\n").unwrap_err(),
            HudConfigError::BadHeader
        );
        assert_eq!(
            HudLayoutProfile::from_config_string("").unwrap_err(),
            HudConfigError::BadHeader
        );
    }
}
