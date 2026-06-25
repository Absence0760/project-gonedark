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

    // --- Scenario 2: embodied — world goes dark ------------------------------------------------
    println!("[embodied_dark] possessing a unit collapses vision to the avatar (invariant #6)");
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
    check(
        &mut failures,
        "embodied_world_went_dark",
        dark_dk > 0.95,
        format!("dark fraction {dark_dk:.4} (>0.95 — the strategic map is gone)"),
    );

    // --- Scenario 3: embodied + combat → alert HUD ---------------------------------------------
    println!("[embodied_hud] after combat, the directional alert HUD draws markers over the dark");
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
    advance(&mut g, 220, InputFrame::default(), &gpu, &view); // allies take fire → alerts accrue
    let hud_frame = read_pixels(&gpu.device, &gpu.queue, &target);
    save_png("target/viz/embodied_hud.png", &hud_frame);
    let hud_dk = dark_fraction(&hud_frame);
    let hud_nondark = (hud_frame.len() / 4) - count(&hud_frame, is_dark);
    check(
        &mut failures,
        "hud_still_mostly_dark",
        hud_dk > 0.80,
        format!(
            "dark fraction {hud_dk:.4} (>0.80 — markers are a thin overlay, the world stays dark)"
        ),
    );
    check(
        &mut failures,
        "hud_markers_drawn",
        hud_nondark > dark_nondark + 50,
        format!("{hud_nondark} non-dark px vs {dark_nondark} with no alerts (alert markers added)"),
    );

    println!("\nPNGs: target/viz/{{command,selected,embodied_dark,embodied_hud}}.png");
    if failures == 0 {
        println!("RESULT: all visual assertions passed ✓");
    } else {
        println!("RESULT: {failures} visual assertion(s) FAILED ✗");
        std::process::exit(1);
    }
}
