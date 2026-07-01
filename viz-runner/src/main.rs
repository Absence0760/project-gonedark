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

use gonedark_core::components::Faction;
use gonedark_engine::{Game, PaletteMode, Scene, DEFAULT_SEED};
use gonedark_pal::{Audio, AudioCue, InputFrame, ThermalSensor, ThermalState};

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

/// A nominal, never-throttling thermal sensor (the viz only cares about pixels, not the render
/// backoff). `Game::frame` requires an `&dyn ThermalSensor`; this reports `Nominal` every frame so
/// the tuning loop stays at full quality during a viz run.
struct NullThermal;
impl ThermalSensor for NullThermal {
    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }
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

// --- arbitrary-size target / readback / png (for the 16:9 title-backdrop scene) -----------------
// The default helpers above bake in the square 512² (`W`/`H`); the title backdrop is rendered at a
// wide 16:9 so a viewport-stretch bug would show (the project memo: "viz is square so render 16:9 to
// catch it"). These mirror `make_target`/`read_pixels`/`save_png` with explicit dimensions.

fn make_target_sized(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("viz.target.sized"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
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

fn read_pixels_sized(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let bpp = 4u32;
    let unpadded = w * bpp;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("viz.readback.sized"),
        size: (padded * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("viz.readback_encoder.sized"),
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
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
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
    let mut out = Vec::with_capacity((unpadded * h) as usize);
    for row in 0..h {
        let start = (row * padded) as usize;
        out.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    drop(data);
    buffer.unmap();
    out
}

fn save_png_sized(path: &str, rgba: &[u8], w: u32, h: u32) {
    let file = std::fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
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
/// A saturated faction-GREEN token pixel — green clearly dominant over both red and blue. Used by the
/// WS-D accessibility scene: under the tritanopia ramp the player faction is green, a hue the default
/// command frame (cool blue tokens + blue-grey terrain) essentially never produces, so a jump in this
/// count is a clean signal the alternate ramp was baked into the command-view tokens.
fn is_faction_green(p: [u8; 4]) -> bool {
    p[1] > 110 && p[1] as i32 > p[0] as i32 + 35 && p[1] as i32 > p[2] as i32 + 25
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
/// A command-view debug muzzle-flash pixel (`render::debug` `COLOR_MUZZLE` = linear
/// `[1.0, 0.95, 0.55]`, sRGB-encoded by the target ≈ `(255, 249, 196)`): a hot near-white-yellow,
/// R+G both near-max with G clearly above B. The bounds hug that one colour to exclude the other
/// warm overlay chrome: the cone arc (`COLOR_CONE` ≈ (255,211,124), G too low / B too low), the
/// enemy range ring (≈ (255,170,160), G far too low), the tank SIDE facet (≈ (255,234,124), B too
/// low), and the white selection rim (B ≈ 255). Verified against a pre-combat baseline (~0).
fn is_muzzle_flash(p: [u8; 4]) -> bool {
    p[0] > 248 && p[1] > 240 && (182..=214).contains(&p[2]) && (p[1] as i32 - p[2] as i32) > 32
}
/// A WS-4 hitmarker pixel (`render::hud` `HITMARKER_COLOR` = white `[1,1,1]`, sRGB ≈ (255,255,255)):
/// a bright near-white where ALL THREE channels are high — crucially including BLUE. This is what
/// makes it distinct from everything else in the embodied frame: the muzzle flash is a WARM yellow
/// (blue stays ~196, see `is_muzzle_flash`); the FPS world is a muted, low-saturation blue-grey that
/// never gets this bright; and the only near-white alert glyph (the pale-grey UnitLost marker) rides
/// the screen-EDGE ring, never the center. So a high-blue near-white pixel at frame center reads
/// only as the centered hitmarker "X".
fn is_hitmarker(p: [u8; 4]) -> bool {
    p[0] > 235 && p[1] > 235 && p[2] > 235
}
/// A command-view **unit-kind glyph** pixel (CP-9 / WS-C — `render::token_icons`). The kind icons are
/// drawn as the last command pass: flat, UNSHADED, faction-tinted glyphs at the pure theme colour,
/// composited over the tokens. That makes them brighter and more saturated than the LIT 3D token
/// meshes underneath (whose fragment colours are multiplied down by the mesh lighting). The player
/// glyph is `theme::PLAYER` (a bright blue with a HIGH green channel); the mesh player token, being
/// shaded, never reaches this brightness in both blue AND green at once. So a pixel that is bright-blue
/// *and* bright-green *and* blue-dominant reads as a flat player-faction glyph, not a shaded token.
fn is_kind_glyph(p: [u8; 4]) -> bool {
    p[2] > 185 && p[1] > 150 && p[2] as i32 > p[0] as i32 + 45 && p[1] as i32 > p[0] as i32 + 20
}
fn count(rgba: &[u8], f: impl Fn([u8; 4]) -> bool) -> usize {
    px(rgba).filter(|&p| f(p)).count()
}
/// Count pixels matching `f` inside a centered square crop of half-width `half` px. The hitmarker
/// draws dead-center (NDC `(0,0)`), so cropping to the center isolates it from the edge-ring alert
/// markers and the bottom-anchored weapon viewmodel — the established coarse-bucket viz style,
/// localized in space rather than only in color.
fn count_center(rgba: &[u8], half: u32, f: impl Fn([u8; 4]) -> bool) -> usize {
    let (cx, cy) = (W / 2, H / 2);
    let mut n = 0;
    for y in cy.saturating_sub(half)..(cy + half).min(H) {
        for x in cx.saturating_sub(half)..(cx + half).min(W) {
            let i = ((y * W + x) * 4) as usize;
            if f([rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]) {
                n += 1;
            }
        }
    }
    n
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
            &NullThermal,
        );
    }
}

/// Drive `n` frames holding the SAME input every frame — for level/held inputs like `fire` that the
/// real host re-emits each frame (unlike [`advance`], which applies its input only on frame 0).
fn advance_holding(game: &mut Game, n: u32, held: InputFrame, gpu: &Gpu, view: &wgpu::TextureView) {
    for _ in 0..n {
        game.frame(
            &held,
            TICK_DT,
            (W, H),
            &gpu.device,
            &gpu.queue,
            view,
            &mut NullAudio,
            &NullThermal,
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

/// --- Title backdrop scene (render-crate component, no `Game`) -----------------------------------
/// Render the standalone animated parallax title backdrop into a 16:9 offscreen target for a couple
/// of representative `(time, cursor)` states, save PNGs, and assert it draws an atmospheric dark 3D
/// diorama: not a flat single colour, contains warm amber accents (the embers/rim), and the zenith
/// (top) is darker than the horizon band — the basic "sky over a lit horizon" read.
fn backdrop_scene(gpu: &Gpu, failures: &mut u32) {
    use gonedark_render::title_backdrop::TitleBackdrop;
    const TW: u32 = 1024;
    const TH: u32 = 576;

    println!("[title_backdrop] the animated parallax title backdrop draws a dark 3D diorama");
    let (target, view) = make_target_sized(&gpu.device, TW, TH);
    let mut backdrop = TitleBackdrop::new(&gpu.device, FORMAT);

    // Frame A: app start, pointer centred.
    backdrop.render(&gpu.device, &gpu.queue, &view, (TW, TH), 0.0, None);
    let a = read_pixels_sized(&gpu.device, &gpu.queue, &target, TW, TH);
    save_png_sized("target/viz/title_backdrop.png", &a, TW, TH);

    // Frame B: a few seconds in, pointer pushed to the lower-right (drift + parallax engaged).
    backdrop.render(
        &gpu.device,
        &gpu.queue,
        &view,
        (TW, TH),
        3.0,
        Some([0.6, -0.4]),
    );
    let b = read_pixels_sized(&gpu.device, &gpu.queue, &target, TW, TH);
    save_png_sized("target/viz/title_backdrop_parallax.png", &b, TW, TH);

    // (1) Not a flat single colour: there is a real spread between the darkest and brightest pixels.
    let lum = |p: [u8; 4]| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for c in a.chunks_exact(4) {
        let l = lum([c[0], c[1], c[2], c[3]]);
        lo = lo.min(l);
        hi = hi.max(l);
    }
    check(
        failures,
        "title_backdrop_not_flat",
        hi - lo > 30.0,
        format!("luminance spread {:.1} (>30 — a 3D scene, not a flat fill)", hi - lo),
    );

    // (2) Warm amber accents present (the embers + box rim — the signature warm-over-cold motif):
    // pixels where red clearly dominates green dominates blue and the pixel is reasonably warm/bright.
    let is_amber = |p: [u8; 4]| {
        p[0] as i32 > p[1] as i32 + 18 && p[1] as i32 >= p[2] as i32 && p[0] > 90
    };
    let amber = a.chunks_exact(4).filter(|c| is_amber([c[0], c[1], c[2], c[3]])).count();
    check(
        failures,
        "title_backdrop_has_amber_accents",
        amber > 40,
        format!("{amber} warm amber px (>40 — embers/rim drawn over the cold dark)"),
    );

    // (3) The sky reads top-dark → horizon-lighter: average the top 12% of rows vs a band around
    // 55–70% down the frame (the horizon glow). Top must be darker.
    let row_band_avg = |rgba: &[u8], y0: u32, y1: u32| -> f32 {
        let mut sum = 0.0f64;
        let mut n = 0u64;
        for y in y0..y1 {
            for x in 0..TW {
                let i = ((y * TW + x) * 4) as usize;
                sum += lum([rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]) as f64;
                n += 1;
            }
        }
        (sum / n as f64) as f32
    };
    let top = row_band_avg(&a, 0, TH * 12 / 100);
    let horizon = row_band_avg(&a, TH * 55 / 100, TH * 70 / 100);
    check(
        failures,
        "title_backdrop_top_darker_than_horizon",
        top < horizon,
        format!("top luminance {top:.1} < horizon band {horizon:.1} (sky over a lit horizon)"),
    );

    // Sanity: a non-trivial PNG was written.
    let sz = std::fs::metadata("target/viz/title_backdrop.png").map(|m| m.len()).unwrap_or(0);
    check(
        failures,
        "title_backdrop_png_written",
        sz > 2000,
        format!("title_backdrop.png is {sz} bytes (>2000 — a real image was written)"),
    );
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
    // CP-9 (WS-C): a small unit-kind glyph is drawn over each command-view token so the player reads
    // composition at a glance. The glyphs are flat, unshaded, faction-tinted — a signature the lit 3D
    // tokens can't reach (see `is_kind_glyph`). Several player units each get a ~0.05-NDC glyph, so the
    // lit command frame carries a clear band of these bright flat-blue pixels; the embodied dark frame
    // has NONE (the fairness gate `token_icons` returns empty over `world_dark`, re-checked by the
    // Scenario 2 `embodied_strategic_map_dark` player-blue collapse).
    let kind_glyphs = count(&cmd, is_kind_glyph);
    check(
        &mut failures,
        "command_draws_kind_glyphs",
        kind_glyphs > 150,
        format!("{kind_glyphs} flat unit-kind-glyph px (>150 — the CP-9 kind icons render over tokens)"),
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
        &NullThermal,
    );
    g.frame(
        &band_up,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
        &NullThermal,
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
        &NullThermal,
    );
    g.frame(
        &band_up,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
        &NullThermal,
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
        &NullThermal,
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
        &NullThermal,
    );
    g.frame(
        &hold,
        TICK_DT,
        (W, H),
        &gpu.device,
        &gpu.queue,
        &view,
        &mut NullAudio,
        &NullThermal,
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
        &NullThermal,
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
    // the enemy actively firing, the embodied frame stays free of strategic map content (#6).
    //
    // Invariant #5 makes this scenario subtle, and is what the standing FAIL was really about. A
    // possessed avatar that dies AUTO-SURFACES the player back to command — there is NO respawn
    // (engine `should_auto_surface`). The default-scene avatar holds the front line and dies early,
    // so driving a single embody for 220 frames and reading "whatever falls out" asserted the
    // POST-EJECTION command view (grid + control-point rings + `UNITS:` readout) — which of course
    // shows strategic intel, and whose warm HUD text even spuriously satisfied `hud_markers_drawn`.
    // That was a SCENARIO bug, not an invariant-#6 leak (scenario 2 already proves a genuinely
    // embodied frame draws zero strategic blue). The fix is to stay embodied the way a real player
    // does: when the avatar falls and the host ejects us, re-press embody to possess another live
    // player unit (the RTS "pick another unit" loop — engine `embody_target` falls back to a live
    // unit), and assert on the best GENUINELY-embodied combat frame (guarded by `is_embodied()` and
    // no shell overlay) once alerts have accrued.
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
        &NullThermal,
    );
    // Capture a no-alert baseline of the SAME embodied scene first (the world + avatar, before any
    // alerts), so the marker delta isolates the HUD overlay rather than the world itself.
    let pre_alert = read_pixels(&gpu.device, &gpu.queue, &target);
    let pre_marker = count(&pre_alert, is_alert_marker);
    // Drive combat, re-possessing a live unit after every avatar death so the fight is fought (and
    // asserted) in first person, and keep the embodied frame with the most alert markers. Combat
    // raises alerts within the first second or two, while player units (incl. camp reinforcements)
    // are still alive to re-possess.
    let mut hud_marker = 0usize;
    let mut hud_frame: Vec<u8> = Vec::new();
    let mut captured_embodied = false;
    for _ in 0..240 {
        // If a fallen avatar ejected us to command, re-press embody to take another live player
        // unit; otherwise let the frame run. `embody_pressed` is a no-op once already embodied.
        let input = if g.is_embodied() {
            InputFrame::default()
        } else {
            embody.clone()
        };
        g.frame(
            &input,
            TICK_DT,
            (W, H),
            &gpu.device,
            &gpu.queue,
            &view,
            &mut NullAudio,
            &NullThermal,
        );
        // Only a frame that is embodied AND has no shell overlay (pause / post-match summary) up is
        // a valid sample of the dark first-person combat view.
        if g.is_embodied() && !g.shell_overlay_active() {
            let f = read_pixels(&gpu.device, &gpu.queue, &target);
            let m = count(&f, is_alert_marker);
            if !captured_embodied || m >= hud_marker {
                hud_marker = m;
                hud_frame = f;
                captured_embodied = true;
            }
        }
    }
    save_png("target/viz/embodied_hud.png", &hud_frame);
    // Guard: we must have actually held a genuinely embodied combat frame. If we never could (every
    // player unit died before we re-possessed), the scenario proves nothing — fail loudly rather
    // than assert against a stale/command frame (the exact trap the old version fell into).
    check(
        &mut failures,
        "embodied_combat_frame_captured",
        captured_embodied,
        "held a genuinely embodied combat frame (is_embodied + no overlay) to assert against"
            .to_string(),
    );
    // The directional alert HUD draws marker glyphs over the FPS world: more alert-marker-colored
    // px during combat than before any alert fired. (`is_alert_marker` keys on the saturated marker
    // palette — orange/red/teal/pale — which the muted blue-grey sky/ground never produces.)
    check(
        &mut failures,
        "hud_markers_drawn",
        hud_marker > pre_marker + 30,
        format!("{hud_marker} alert-marker px during embodied combat vs {pre_marker} before (directional pings drawn over the world)"),
    );
    // The fairness guarantee holds THROUGH combat: the alert HUD is a DIRECTIONAL PING ring near the
    // screen edge (`render/src/hud.rs`), not a map reveal — it tells you a bearing, never an enemy
    // position. The strategic map stays dark: the player's own off-screen squad + control-point
    // intel never reappear. We count player-blue px that are NOT themselves alert markers (the teal
    // TerritoryLost glyph reads blue-ish but is a directional ping, not ally intel) — that residual
    // must stay near zero even while the enemy is firing AND we keep re-embodying through deaths.
    let hud_map_intel = count(&hud_frame, |p| is_player_blue(p) && !is_alert_marker(p));
    check(
        &mut failures,
        "embodied_combat_strategic_map_stays_dark",
        hud_map_intel < 20 && (hud_map_intel as f32) < (blue as f32) * 0.1,
        format!("{hud_map_intel} non-marker player-blue px during embodied combat (<20 and <10% of command's {blue} — the map stays dark; alerts are pings, not intel)"),
    );
    let _ = dark_nondark; // (the pre-/post-alert dark counts are now both ~0 with a world drawn)

    // --- Scenario 3b: accessibility cues (WS-D, invariant #6) ----------------------------------
    // The going-dark alert is a directional flash + audio; a colourblind or hard-of-hearing player
    // needs an equivalent or the core mechanic is unfair. Two halves are proven here:
    //   (i)  the colourblind-safe faction PALETTE actually swaps the rendered ramp (command view),
    //        while the readout tally still classifies units (interpolate + tally share one palette);
    //   (ii) the embodied ALERT HUD still reads under the CVD cue mode — the directional pings draw
    //        over the FPS world with the palette swapped and CVD labels on, and STILL leak no intel.
    println!("[a11y_palette] the colourblind palette swaps the faction ramp in the command view");
    let mut g = Game::new(&gpu.device, FORMAT, DEFAULT_SEED);
    // Enable all accessibility cues; the palette + toggles persist on the Game across frames (the
    // host would re-push them each frame — a single set is equivalent here). Tritanopia's ramp makes
    // the player faction GREEN (not blue) — a large, lighting-stable recolour the `is_player_blue`
    // predicate can key on, unlike the red-green ramp whose blue/orange sit adjacent to the defaults.
    g.set_accessibility_prefs(true, true, PaletteMode::Tritanopia);
    advance(&mut g, 40, InputFrame::default(), &gpu, &view);
    let cmd_cvd = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/command_cvd.png", &cmd_cvd);
    check(
        &mut failures,
        "cvd_command_not_dark",
        dark_fraction(&cmd_cvd) < 0.5,
        format!("dark fraction {:.3} (<0.5 — the CVD command frame is still a lit field)", dark_fraction(&cmd_cvd)),
    );
    // The palette swap must actually recolour the faction tokens: under tritanopia the player ramp is
    // green, so the strong player-blue signature of the default command frame (`cmd`, same seed + 40
    // frames) collapses. That the same tokens now render green — not blue — is the on-screen proof the
    // renderer baked the alternate ramp (interpolate_instances) into the command view.
    let green_default = count(&cmd, is_faction_green);
    let green_cvd = count(&cmd_cvd, is_faction_green);
    check(
        &mut failures,
        "cvd_palette_recolors_player_tokens",
        green_cvd > green_default + 200,
        format!("{green_cvd} faction-green px under the tritanopia ramp vs {green_default} default (the player tokens render GREEN under the CVD ramp — the alternate ramp is baked into the command view)"),
    );

    println!("[a11y_alert] the embodied alert HUD reads under the CVD cue mode (pings, still no intel)");
    let mut g = Game::new(&gpu.device, FORMAT, DEFAULT_SEED);
    g.set_accessibility_prefs(true, true, PaletteMode::Deuteranopia);
    g.frame(&embody, TICK_DT, (W, H), &gpu.device, &gpu.queue, &view, &mut NullAudio, &NullThermal);
    let a11y_pre = read_pixels(&gpu.device, &gpu.queue, &target);
    let a11y_pre_marker = count(&a11y_pre, is_alert_marker);
    let mut a11y_marker = 0usize;
    let mut a11y_frame: Vec<u8> = Vec::new();
    let mut a11y_captured = false;
    for _ in 0..240 {
        let input = if g.is_embodied() {
            InputFrame::default()
        } else {
            embody.clone()
        };
        g.frame(&input, TICK_DT, (W, H), &gpu.device, &gpu.queue, &view, &mut NullAudio, &NullThermal);
        if g.is_embodied() && !g.shell_overlay_active() {
            let f = read_pixels(&gpu.device, &gpu.queue, &target);
            let m = count(&f, is_alert_marker);
            if !a11y_captured || m >= a11y_marker {
                a11y_marker = m;
                a11y_frame = f;
                a11y_captured = true;
            }
        }
    }
    save_png("target/viz/embodied_a11y.png", &a11y_frame);
    check(
        &mut failures,
        "a11y_embodied_frame_captured",
        a11y_captured,
        "held a genuinely embodied combat frame under the CVD cue mode to assert against".to_string(),
    );
    check(
        &mut failures,
        "a11y_alert_hud_reads",
        a11y_marker > a11y_pre_marker + 30,
        format!("{a11y_marker} alert-marker px under the CVD cue mode vs {a11y_pre_marker} before (the alert still reads)"),
    );
    // (Fairness under the cue mode — no strategic map intel while embodied — is proven by Scenarios 2
    // & 3 above, and by the pure directional-only guard `marker_position_encodes_only_bearing` in
    // `render::hud` tests: the CVD labels/echoes ride the same bearing-only ring, adding no world
    // position. A pixel re-test here would false-positive on the legitimate teal TerritoryLost ping.)

    // --- Scenario 4: command-view muzzle flash (TF-1) ------------------------------------------
    // The debug overlay draws a bright muzzle-flash burst on any unit that fired in the last few
    // ticks (command-view only, invariant #6). Boot the demo skirmish, turn the overlay on (F3),
    // let the squads close and trade fire, and prove the flash actually draws — the visual proof
    // that "units are firing" reads on screen, which the headless sim harnesses cannot see.
    println!("[combat_muzzle] AI units firing draw a muzzle flash in the command-view debug overlay");
    let mut g = Game::new_scene(&gpu.device, FORMAT, DEFAULT_SEED, Scene::Default);
    g.toggle_debug_hitboxes(); // Scene::Default boots the overlay OFF — turn it on (the F3 overlay)
    // Baseline BEFORE anyone is in range: the overlay (range rings / cones / LoS lines) is already
    // drawn, but no shot has fired — so the muzzle-flash count must be ~0 here. This makes the check
    // a real delta and proves the predicate isn't just catching static overlay chrome.
    advance(&mut g, 8, InputFrame::default(), &gpu, &view);
    let pre = read_pixels(&gpu.device, &gpu.queue, &target);
    let pre_muzzle = count(&pre, is_muzzle_flash);
    // A flash lasts MUZZLE_FLASH_TICKS (8) per shot on a ~30-tick cooldown, so any single frame may
    // fall in a cooldown gap. Sample the whole engagement (the squads close, trade, and die out) and
    // keep the frame with the most flash — guarantees catching a unit mid-flash while combat lasts.
    let mut best_muzzle = 0usize;
    let mut muzzle_frame: Vec<u8> = Vec::new();
    for _ in 0..220 {
        advance(&mut g, 1, InputFrame::default(), &gpu, &view);
        let f = read_pixels(&gpu.device, &gpu.queue, &target);
        let m = count(&f, is_muzzle_flash);
        if m >= best_muzzle {
            best_muzzle = m;
            muzzle_frame = f;
        }
    }
    save_png("target/viz/combat_muzzle.png", &muzzle_frame);
    check(
        &mut failures,
        "command_muzzle_flash_drawn",
        pre_muzzle < 8 && best_muzzle > 30,
        format!("muzzle-flash px {pre_muzzle} pre-combat → {best_muzzle} peak during combat (firing reads on screen, not static overlay)"),
    );

    // --- Scenario 5: embodied fire kills the enemy you aim at (TF-1) ---------------------------
    // The infantry sandbox boots embodied, facing +X at a row of dummies (the open one 8 units dead
    // ahead, 12 HP). Holding the trigger must KILL them — the visual proof of the kill path the
    // "impossible to kill" report was about. (Through the full `Game::frame` path the enemy commander
    // also advances the dummies into the line of fire, so several fall, not just the two reachable
    // from a standstill.)
    println!("[embodied_kill] holding fire while embodied kills the enemies under the crosshair");
    let mut g = Game::new_scene(&gpu.device, FORMAT, DEFAULT_SEED, Scene::Infantry);
    advance(&mut g, 4, InputFrame::default(), &gpu, &view); // settle the camera/world
    let before = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/embodied_kill_before.png", &before);
    // WS-4 baseline: BEFORE any shot connects there is no hitmarker, so the center crop holds no
    // bright near-white. (The embodied world center is the muted blue-grey horizon.) This makes the
    // hitmarker check below a real delta — the "X" appears where ~zero was.
    let pre_hit_center = count_center(&before, 48, is_hitmarker);
    // The kill itself is judged on SIM state, not screen pixels: the embodied first-person frame
    // carries a warm muzzle-flash tint + a low-health vignette that defeat an enemy-red pixel count
    // (we render before/after PNGs for eyeballing, but assert on the authoritative entity count).
    let enemies_before = g.alive_unit_count(Faction::Enemy);
    let fire = InputFrame {
        fire: true,
        ..Default::default()
    };
    // Hold the trigger ~2.6 s. A hitmarker flashes for HITMARKER_TICKS (10) per connecting shot and
    // fades, so any single frame may fall in the gap between hits — sample the whole window and keep
    // the frame with the most center hitmarker pixels (a fresh, full-bright "X"), exactly as the
    // muzzle-flash scene samples its peak. That frame is the visual proof a shot CONNECTED.
    let mut best_hit_center = 0usize;
    let mut hit_frame: Vec<u8> = before.clone();
    for _ in 0..160 {
        advance_holding(&mut g, 1, fire.clone(), &gpu, &view);
        let f = read_pixels(&gpu.device, &gpu.queue, &target);
        let c = count_center(&f, 48, is_hitmarker);
        if c >= best_hit_center {
            best_hit_center = c;
            hit_frame = f;
        }
    }
    save_png("target/viz/embodied_kill_after.png", &hit_frame);
    let enemies_after = g.alive_unit_count(Faction::Enemy);
    check(
        &mut failures,
        "embodied_fire_kills_enemy",
        enemies_before > 0 && enemies_after < enemies_before,
        format!("alive enemy units {enemies_before} → {enemies_after} after sustained embodied fire (the targets you aimed at died)"),
    );
    // WS-4: the hitmarker "X" draws over the dark frame when the avatar's OWN shot connects — the
    // "I hit him" signal the game never sent. The center crop goes from ~0 (pre-hit) to a clear
    // count on a connecting frame (presentation feedback on the player's own action; invariant #6).
    check(
        &mut failures,
        "embodied_hitmarker_on_connecting_shot",
        pre_hit_center < 8 && best_hit_center > 20,
        format!("center hitmarker px {pre_hit_center} pre-hit → {best_hit_center} peak while firing (the \"I hit him\" X drew on a connecting shot)"),
    );

    // --- Scenario 5.9: a baked real-world map draws its cover grid under the debug overlay ------
    // Boot the map-inspection scene (Scene::MapInspect) on the baked Pointe du Hoc map. Its F3 cover
    // overlay is ON by default, so `render::debug::covergrid_lines` outlines the sim's actual cover
    // cells (amber Light, steel Heavy) over the command view — the in-engine half of map diagnosis.
    // Toggling the overlay OFF must change a large number of pixels (the thousands of cover-cell
    // edges vanish): the visual proof the baked map's cover is BOTH loaded into the sim AND drawn for
    // inspection. A frame diff (not a color match) keeps this robust to the present-grade transform.
    println!("[map_inspect] the baked Pointe du Hoc map draws its cover grid in the debug overlay");
    let mut g = Game::new_scene(&gpu.device, FORMAT, DEFAULT_SEED, Scene::MapInspect);
    advance(&mut g, 2, InputFrame::default(), &gpu, &view); // settle the camera on the command view
    let overlay_on = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/map_inspect.png", &overlay_on);
    g.toggle_debug_hitboxes(); // F3: cover overlay OFF
    advance(&mut g, 1, InputFrame::default(), &gpu, &view);
    let overlay_off = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/map_inspect_no_overlay.png", &overlay_off);
    let changed = overlay_on
        .chunks_exact(4)
        .zip(overlay_off.chunks_exact(4))
        .filter(|(a, b)| {
            (a[0] as i16 - b[0] as i16).abs() > 12
                || (a[1] as i16 - b[1] as i16).abs() > 12
                || (a[2] as i16 - b[2] as i16).abs() > 12
        })
        .count();
    check(
        &mut failures,
        "baked_map_cover_overlay_draws",
        changed > 3000,
        format!("toggling the cover overlay changed {changed} px on the baked map (the cover grid draws)"),
    );

    // --- Scenario 6: animated parallax title backdrop (render-crate component) -----------------
    backdrop_scene(&gpu, &mut failures);

    println!(
        "\nPNGs: target/viz/{{command,selected,radial,marquee,embodied_dark,embodied_hud,\
         combat_muzzle,embodied_kill_before,embodied_kill_after,title_backdrop,\
         title_backdrop_parallax}}.png"
    );
    if failures == 0 {
        println!("RESULT: all visual assertions passed ✓");
    } else {
        println!("RESULT: {failures} visual assertion(s) FAILED ✗");
        std::process::exit(1);
    }
}
