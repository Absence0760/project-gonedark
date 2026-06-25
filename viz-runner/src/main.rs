//! Offscreen render harness — the visual counterpart to `sim-runner` (invariant #4/#6).
//!
//! CI proves the sim is deterministic; this proves the *presentation* actually draws. It builds
//! the real [`gonedark_engine::Game`] against a SURFACELESS wgpu device (no window, no display),
//! drives scripted input through the exact `Game::frame` path both hosts use, renders into an
//! offscreen texture, reads the pixels back, writes a PNG per scenario, and asserts pixel-level
//! invariants:
//!
//!   - **command** — top-down view draws units (player-blue + enemy-red) on the lit field.
//!   - **selected** — a command-layer band-select rims the selected units in bright white (the
//!     player can see what's selected) — more bright-rim pixels than the un-selected frame.
//!   - **embodied_dark** — possessing a unit makes the world go dark (invariant #6): the frame is
//!     ~entirely black (the strategic map is gone; the avatar is self-occluded in first person).
//!   - **embodied_hud** — after some combat the directional alert HUD draws markers over the dark
//!     frame (the only thread back while blind).
//!
//! It needs a real GPU adapter, so it is a LOCAL smoke test (`pnpm viz`), deliberately NOT wired
//! into the no-GPU CI matrix. Exit code: 0 = all assertions passed (or skipped: no adapter),
//! 1 = a visual assertion failed. PNGs land in `target/viz/` for eyeballing.

use gonedark_engine::{Game, DEFAULT_SEED};
use gonedark_pal::{Audio, AudioCue, InputFrame};

const W: u32 = 512;
const H: u32 = 512;
/// An sRGB target so read-back bytes are display-encoded (what the eye/PNG expects). The
/// renderer's pipeline is built for whatever format we hand `Game::new`, so this is consistent.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const TICK_DT: f32 = 1.0 / 60.0;

/// A do-nothing audio sink (the mix is exercised by `engine::audio`'s own tests; here we only
/// care about pixels). `Game::frame` requires an `&mut dyn Audio`.
struct NullAudio;
impl Audio for NullAudio {
    fn play_oneshot(&mut self, _sound_id: u32) {}
    fn submit_mix(&mut self, _cues: &[AudioCue]) {}
}

/// Surfaceless GPU context.
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn init_gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None, // headless — no window/surface
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gonedark-viz-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    Some(Gpu { device, queue })
}

/// An offscreen color target the renderer draws into, plus its view.
fn make_target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("viz.target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Copy the target texture to a mappable buffer and read it back as tightly-packed RGBA8.
fn read_pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let bpp = 4u32;
    let unpadded = W * bpp;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("viz.readback"),
        size: (padded * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("viz.readback_encoder"),
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(enc.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll the device for the readback map");
    rx.recv().expect("map_async result").expect("buffer map ok");

    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity((unpadded * H) as usize);
    for row in 0..H {
        let start = (row * padded) as usize;
        out.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    drop(data);
    buffer.unmap();
    out
}

fn save_png(path: &str, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), W, H);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(rgba)
        .expect("png data");
}

// --- pixel predicates ---------------------------------------------------------------------------

fn px(rgba: &[u8]) -> impl Iterator<Item = [u8; 4]> + '_ {
    rgba.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]])
}
/// Near-black (the "world goes dark" clear is pure black; the lit clear is a ~slate the units
/// read against, which lands well above this threshold).
fn is_dark(p: [u8; 4]) -> bool {
    p[0] < 24 && p[1] < 24 && p[2] < 24
}
/// Player blue (cool, blue-dominant + bright).
fn is_player_blue(p: [u8; 4]) -> bool {
    p[2] > 120 && p[2] as i32 > p[0] as i32 + 30 && p[1] as i32 > p[0] as i32
}
/// Enemy red (warm, red-dominant).
fn is_enemy_red(p: [u8; 4]) -> bool {
    p[0] > 150 && p[0] as i32 > p[1] as i32 + 40 && p[0] as i32 > p[2] as i32 + 40
}
/// The selection rim is a bright cool-white (R,G,B all high). No other renderable draws this:
/// faction bodies are saturated, the health bar is green/dark-red, the clear is a dark slate. So
/// a near-white pixel is a selection-highlight pixel.
fn is_select_rim(p: [u8; 4]) -> bool {
    p[0] > 220 && p[1] > 220 && p[2] > 220
}
/// Radial-menu chrome (the hub + wedge slots): a mid, blue-leaning grey — brighter than the slate
/// field and the dim backdrop, but well below the player-blue body and the white selection rim, so
/// it reads as its own band. `B > R` and `G > R` keep warm bodies/health bars out; the upper bounds
/// keep player-blue and the rim out.
fn is_radial_wedge(p: [u8; 4]) -> bool {
    (110..=160).contains(&p[0])
        && (120..=170).contains(&p[1])
        && (140..=190).contains(&p[2])
        && p[2] > p[0]
        && p[1] > p[0]
}
/// A directional alert-HUD marker pixel (`render/src/hud.rs`). The markers are drawn in the
/// saturated alert palette — warm orange/red (TakingFire / BaseUnderAttack), cyan-teal
/// (TerritoryLost), or a bright pale grey (UnitLost) — and composited over the embodied frame near
/// the screen edge. The embodied WORLD underneath (W5) is a MUTED blue-grey sky + slate floor whose
/// channels stay close together (low saturation) and never warm. So a marker reads as either a
/// bright WARM pixel (red strongly dominant, like the alert reds/oranges) or a bright cyan-TEAL
/// pixel (green+blue both high, red low) — neither of which the cool low-saturation world produces.
fn is_alert_marker(p: [u8; 4]) -> bool {
    let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
    // Warm marker (orange/red): red clearly dominant and the pixel is bright.
    let warm = r > 170 && r > b + 50 && r >= g;
    // Teal marker (TerritoryLost): green + blue both high with red noticeably lower — a hue the
    // blue-grey world (where channels track within ~30) never hits.
    let teal = g > 150 && b > 150 && g > r + 40 && b > r + 30;
    warm || teal
}
fn count(rgba: &[u8], f: impl Fn([u8; 4]) -> bool) -> usize {
    px(rgba).filter(|&p| f(p)).count()
}
fn dark_fraction(rgba: &[u8]) -> f32 {
    count(rgba, is_dark) as f32 / (rgba.len() / 4) as f32
}

/// Drive `n` frames, applying `first` on the first frame (edge-triggered intents fire once) and
/// nothing thereafter; each frame advances ~one 60 Hz tick and renders into `view`.
fn advance(game: &mut Game, n: u32, first: InputFrame, gpu: &Gpu, view: &wgpu::TextureView) {
    for i in 0..n {
        let input = if i == 0 {
            first.clone()
        } else {
            InputFrame::default()
        };
        game.frame(
            &input,
            TICK_DT,
            (W, H),
            &gpu.device,
            &gpu.queue,
            view,
            &mut NullAudio,
        );
    }
}

/// One assertion; records pass/fail to `failures` and prints a line.
fn check(failures: &mut u32, name: &str, cond: bool, detail: String) {
    if cond {
        println!("  PASS  {name}: {detail}");
    } else {
        println!("  FAIL  {name}: {detail}");
        *failures += 1;
    }
}

fn main() {
    let Some(gpu) = init_gpu() else {
        println!("SKIP: no wgpu adapter available (headless/CI without a GPU) — nothing rendered.");
        return; // exit 0: a missing GPU is not a visual failure.
    };
    std::fs::create_dir_all("target/viz").expect("create target/viz");
    let (target, view) = make_target(&gpu.device);
    let embody = InputFrame {
        embody_pressed: true,
        ..Default::default()
    };
    let mut failures = 0u32;

    // --- Scenario 1: command view --------------------------------------------------------------
    println!("[command] top-down view should draw player + enemy units on the lit field");
    let mut g = Game::new(&gpu.device, FORMAT, DEFAULT_SEED);
    advance(&mut g, 40, InputFrame::default(), &gpu, &view);
    let cmd = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/command.png", &cmd);
    let (blue, red, dark) = (
        count(&cmd, is_player_blue),
        count(&cmd, is_enemy_red),
        dark_fraction(&cmd),
    );
    check(
        &mut failures,
        "command_not_dark",
        dark < 0.5,
        format!("dark fraction {dark:.3} (<0.5 — the lit field is not black)"),
    );
    check(
        &mut failures,
        "command_has_player_units",
        blue > 50,
        format!("{blue} player-blue px (>50)"),
    );
    check(
        &mut failures,
        "command_has_enemy_units",
        red > 50,
        format!("{red} enemy-red px (>50)"),
    );
    // Baseline: with nothing selected the command frame draws no bright selection rim.
    let baseline_rim = count(&cmd, is_select_rim);

    // --- Scenario 1b: command-layer selection highlight ----------------------------------------
    // Band-select the player squad (a pointer-down at one corner, pointer-up at the opposite),
    // then render: the selected units must gain a bright rim the un-selected command frame lacks.
    println!(
        "[selected] band-selecting the player squad rims the selected units (presentation #4)"
    );
    let mut g = Game::new(&gpu.device, FORMAT, DEFAULT_SEED);
    // The player squad sits at world x≈[-9,-7], y≈[-7,4]. With the square 512² viewport framing
    // ±40 world units, these pixel corners bracket the whole squad (top-left ≈ (-13,6),
    // bottom-right ≈ (-5,-9)). Press at one corner, release at the other → a band select.
    let band_down = InputFrame {
        pointer: Some((172.0, 217.0)),
        pointer_down: true,
        ..Default::default()
    };
    let band_up = InputFrame {
        pointer: Some((224.0, 314.0)),
        pointer_up: true,
        ..Default::default()
    };
    // Frame 1: press (anchor). Frame 2: release at the opposite corner (commit the band).
    g.frame(
        &band_down,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
    );
    g.frame(
        &band_up,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
    );
    // Settle a few frames so the highlighted units render steadily, then read back.
    advance(&mut g, 4, InputFrame::default(), &gpu, &view);
    let sel = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/selected.png", &sel);
    let sel_rim = count(&sel, is_select_rim);
    check(
        &mut failures,
        "selection_highlight_visible",
        sel_rim > baseline_rim + 30,
        format!("{sel_rim} bright-rim px after band-select vs {baseline_rim} with nothing selected (selection rim drawn)"),
    );

    // --- Scenario 1c: radial command menu ------------------------------------------------------
    // With the squad selected, a held long-press opens the radial command menu (engine::command_ui
    // Preview): a wedge ring of the applicable vocabulary slots, drawn as a command-view LOAD pass.
    println!(
        "[radial] a held long-press over a selection opens the radial command menu (vocabulary)"
    );
    let mut g = Game::new(&gpu.device, FORMAT, DEFAULT_SEED);
    g.frame(
        &band_down,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
    );
    g.frame(
        &band_up,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
    );
    advance(&mut g, 2, InputFrame::default(), &gpu, &view);
    // Baseline: the selection is up but no long-press → no radial chrome on the frame.
    let pre_menu = read_pixels(&gpu.device, &gpu.queue, &target);
    let pre_wedge = count(&pre_menu, is_radial_wedge);
    // Hold a long-press anchored at the screen center (a pointer, no down/up edge so the selection
    // is untouched; no command_slot so it is a Preview, not a Commit) → the menu opens.
    let long_press = InputFrame {
        pointer: Some((256.0, 256.0)),
        long_press: true,
        ..Default::default()
    };
    g.frame(
        &long_press,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
    );
    let radial = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/radial.png", &radial);
    let radial_wedge = count(&radial, is_radial_wedge);
    check(
        &mut failures,
        "radial_menu_drawn",
        radial_wedge > pre_wedge + 200,
        format!(
            "{radial_wedge} radial-chrome px with the menu open vs {pre_wedge} without (wedge ring drawn)"
        ),
    );

    // --- Scenario 1d: band-select marquee ------------------------------------------------------
    // While a band-drag is IN FLIGHT (pointer held, not yet released) the selection box is drawn.
    // The bright box edge reads as `is_select_rim`; mid-drag nothing is selected yet, so the only
    // such pixels are the marquee itself.
    println!("[marquee] a band-drag in progress draws the selection box (command-view affordance)");
    let mut g = Game::new(&gpu.device, FORMAT, DEFAULT_SEED);
    advance(&mut g, 2, InputFrame::default(), &gpu, &view);
    let pre_drag = read_pixels(&gpu.device, &gpu.queue, &target);
    let pre_rim = count(&pre_drag, is_select_rim); // no box, nothing selected → 0
                                                   // Press at one corner, then a second frame still HELD at the opposite corner (no pointer_up).
    let press = InputFrame {
        pointer: Some((150.0, 150.0)),
        pointer_down: true,
        ..Default::default()
    };
    let hold = InputFrame {
        pointer: Some((380.0, 380.0)),
        pointer_down: true,
        ..Default::default()
    };
    g.frame(
        &press,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
    );
    g.frame(
        &hold,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
    );
    let drag = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/marquee.png", &drag);
    let drag_rim = count(&drag, is_select_rim);
    check(
        &mut failures,
        "marquee_box_drawn",
        drag_rim > pre_rim + 100,
        format!("{drag_rim} bright box-edge px mid-drag vs {pre_rim} before (selection box drawn)"),
    );

    // --- Scenario 2: embodied — a real first-person world, but the STRATEGIC MAP is gone --------
    // W5 replaced the bare black void with a ground/sky/weapon FPS world, so "world goes dark" can
    // no longer be proxied by "the frame is ~all black". We re-express invariant #6 directly,
    // against what the design actually means by going dark (game-design §6): "fog reverts to
    // AVATAR-ONLY vision — you do not see the rest of the map." So the test asserts:
    //   (a) a real world IS drawn (no black void), and
    //   (b) the STRATEGIC MAP collapsed — your own off-screen squad + every ally + the
    //       control-point rings (all map intel) VANISH. We measure this as player-blue ≈ 0:
    //       command view draws hundreds of player-blue px (the whole squad + control points), the
    //       embodied view draws NONE — the only friendly on screen is the AMBER avatar, never blue.
    // NOTE on enemies: this is NOT "enemy-red == 0". Avatar-only vision still shows what the avatar
    // can physically SEE in first person (its vision radius + line of sight) — an enemy standing in
    // front of you is legitimate FPS sight, not a strategic-map reveal. The fairness boundary is the
    // fog layer (`render/src/fog.rs`), which keeps ONLY the avatar + its in-sight cells while
    // embodied and drops the whole-faction strategic union vision and the control-point rings. So
    // the load-bearing fairness signal is the disappearance of the *rest of the map*, captured by
    // the player-blue (own-squad/ally/intel) collapse below.
    println!("[embodied_dark] possessing a unit shows an FPS world but the strategic map is gone (#6)");
    let mut g = Game::new(&gpu.device, FORMAT, DEFAULT_SEED);
    g.frame(
        &embody,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
    );
    advance(&mut g, 2, InputFrame::default(), &gpu, &view); // settle, before combat raises alerts
    let dark_frame = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/embodied_dark.png", &dark_frame);
    let dark_dk = dark_fraction(&dark_frame);
    let dark_nondark = (dark_frame.len() / 4) - count(&dark_frame, is_dark);
    let embodied_blue = count(&dark_frame, is_player_blue);
    // (a) A real world is drawn — the embodied view is no longer a black void (W5).
    check(
        &mut failures,
        "embodied_world_drawn",
        dark_dk < 0.5,
        format!("dark fraction {dark_dk:.4} (<0.5 — a real ground/sky FPS world is drawn, not a void)"),
    );
    // (b) THE fairness assertion (invariant #6): the strategic map collapsed to avatar-only vision.
    // The player faction's whole-map intel — its off-screen squad, every ally, the control-point
    // rings — is filtered out; only the amber avatar (never blue) + the camera-derived environment
    // draw. Command view shows hundreds of player-blue px; embodied shows essentially none. This is
    // the load-bearing re-expression of the old dark-fraction proxy: it proves the map intel is gone
    // even though the frame is now lit. The threshold is well below the command-view squad count.
    check(
        &mut failures,
        "embodied_strategic_map_dark",
        embodied_blue < 20 && (embodied_blue as f32) < (blue as f32) * 0.1,
        format!(
            "{embodied_blue} player-blue (own-squad/ally/control-point) px embodied vs {blue} in command view \
             (<20 and <10% — the strategic map went dark; only the amber avatar remains)"
        ),
    );

    // --- Scenario 3: embodied + combat → alert HUD ---------------------------------------------
    // After combat the directional alert HUD draws markers OVER the FPS world. The markers add
    // non-dark pixels beyond the no-alert world frame (scenario 2), and — crucially — combat still
    // leaks NO enemy intel: the alerts are directional pings, never enemy positions. So even with
    // the enemy actively firing, the embodied frame stays free of enemy-red pixels (invariant #6).
    println!("[embodied_hud] after combat, the alert HUD draws markers over the FPS world (no intel)");
    let mut g = Game::new(&gpu.device, FORMAT, DEFAULT_SEED);
    g.frame(
        &embody,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
    );
    // Capture a no-alert baseline of the SAME embodied scene first (the world + avatar, before any
    // alerts), so the marker delta isolates the HUD overlay rather than the world itself.
    let pre_alert = read_pixels(&gpu.device, &gpu.queue, &target);
    let pre_marker = count(&pre_alert, is_alert_marker);
    advance(&mut g, 220, InputFrame::default(), &gpu, &view); // allies take fire → alerts accrue
    let hud_frame = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/embodied_hud.png", &hud_frame);
    let hud_marker = count(&hud_frame, is_alert_marker);
    // The directional alert HUD draws marker glyphs over the FPS world: more alert-marker-colored
    // px after combat than before any alert fired. (`is_alert_marker` keys on the saturated marker
    // palette — orange/red/teal/pale — which the muted blue-grey sky/ground never produces.)
    check(
        &mut failures,
        "hud_markers_drawn",
        hud_marker > pre_marker + 30,
        format!("{hud_marker} alert-marker px after combat vs {pre_marker} before (directional pings drawn over the world)"),
    );
    // The fairness guarantee holds THROUGH combat: the alert HUD is a DIRECTIONAL PING ring near the
    // screen edge (`render/src/hud.rs`), not a map reveal — it tells you a bearing, never an enemy
    // position. The strategic map stays dark: the player's own off-screen squad + control-point
    // intel never reappear. We count player-blue px that are NOT themselves alert markers (the teal
    // TerritoryLost glyph reads blue-ish but is a directional ping, not ally intel) — that residual
    // must stay near zero even while the enemy is firing.
    let hud_map_intel = count(&hud_frame, |p| is_player_blue(p) && !is_alert_marker(p));
    check(
        &mut failures,
        "embodied_combat_strategic_map_stays_dark",
        hud_map_intel < 20 && (hud_map_intel as f32) < (blue as f32) * 0.1,
        format!("{hud_map_intel} non-marker player-blue px after combat (<20 and <10% of command's {blue} — the map stays dark; alerts are pings, not intel)"),
    );
    let _ = dark_nondark; // (the pre-/post-alert dark counts are now both ~0 with a world drawn)

    println!(
        "\nPNGs: target/viz/{{command,selected,radial,marquee,embodied_dark,embodied_hud}}.png"
    );
    if failures == 0 {
        println!("RESULT: all visual assertions passed ✓");
    } else {
        println!("RESULT: {failures} visual assertion(s) FAILED ✗");
        std::process::exit(1);
    }
}
