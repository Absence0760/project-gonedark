//! A headless screenshot harness for the out-of-match shell screens (dev-only). Renders each screen's
//! egui builder into an offscreen texture and reads it back to a PNG, so the layout can be inspected
//! without opening a window. Mirrors the `viz-runner` readback path. Run explicitly (it needs a GPU
//! and writes files, so it is `#[ignore]`d):
//!
//! ```text
//! cargo test -p gonedark-app shell::shot -- --ignored --nocapture
//! ```
//!
//! Output lands in `target/shell-shots/<screen>.png`.

use super::*;
use gonedark_core::campaign::{Difficulty, NodeId};
use gonedark_render::globe_backdrop::{GlobeBackdrop, GlobePin};

const W: u32 = 1600;
const H: u32 = 1000;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn headless() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .expect("request a headless wgpu adapter");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("shell-shot-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }))
    .expect("request a wgpu device + queue")
}

/// Render one screen builder into a PNG at `path`, clearing to a mid grey so the (dark, translucent)
/// card reads against the background the live 3D backdrop would otherwise provide.
fn shoot(device: &wgpu::Device, queue: &wgpu::Queue, path: &str, build: impl FnMut(&mut egui::Ui)) {
    shoot_impl(device, queue, path, None, build)
}

/// [`shoot`], composited over the LIVE campaign globe backdrop (D103) instead of the grey stand-in —
/// the one shot that exercises the real `render::globe_backdrop` WGSL + pipelines headlessly, so a
/// shader regression fails here instead of at title boot.
fn shoot_over_globe(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &str,
    pins: &[GlobePin],
    build: impl FnMut(&mut egui::Ui),
) {
    shoot_impl(device, queue, path, Some(pins), build)
}

fn shoot_impl(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &str,
    globe_pins: Option<&[GlobePin]>,
    mut build: impl FnMut(&mut egui::Ui),
) {
    let ctx = egui::Context::default();
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| *style = shell_style());
    ctx.set_pixels_per_point(1.0);

    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(W as f32, H as f32),
        )),
        ..Default::default()
    };
    // TWO passes, rendering the second: `over_backdrop_screen` centres its card from the height it
    // remembered on the previous frame, so the first pass lays out at the fallback position and the
    // second is the settled frame the player actually sees. Texture deltas from BOTH passes are
    // accumulated before rendering — egui emits the font-atlas delta only on the first frame, and
    // egui-wgpu samples that atlas for every primitive (solid fills use its white texel).
    let mut deltas = Vec::new();
    let mut full = ctx.run_ui(raw.clone(), |ui| build(ui));
    deltas.append(&mut full.textures_delta.set);
    // Advance the clock a full second on the settled pass so egui's animations (Area fade-in,
    // style eases) complete — back-to-back passes share ~0ms of wall time and would render the
    // title screen mid-fade.
    let raw = egui::RawInput {
        time: Some(1.0),
        ..raw
    };
    let mut full = ctx.run_ui(raw, |ui| build(ui));
    deltas.append(&mut full.textures_delta.set);

    let jobs = ctx.tessellate(full.shapes, full.pixels_per_point);
    let mut renderer =
        egui_wgpu::Renderer::new(device, FORMAT, egui_wgpu::RendererOptions::default());
    for (id, delta) in &deltas {
        renderer.update_texture(device, queue, *id, delta);
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shell-shot.target"),
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

    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [W, H],
        pixels_per_point: full.pixels_per_point,
    };
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("shell-shot.encoder"),
    });
    // The globe variant paints the real backdrop first (it clears the view itself), then the
    // egui pass LOADs over it — the same compositing order as `run_and_paint`.
    if let Some(pins) = globe_pins {
        let mut globe = GlobeBackdrop::new(device, FORMAT);
        globe.render(device, queue, &view, (W, H), 1.0, None, pins);
    }
    let user_cmds = renderer.update_buffers(device, queue, &mut enc, &jobs, &screen);
    {
        let load = if globe_pins.is_some() {
            wgpu::LoadOp::Load
        } else {
            // A mid slate grey stand-in for the live backdrop, so the card's edges are visible.
            wgpu::LoadOp::Clear(wgpu::Color {
                r: 0.22,
                g: 0.25,
                b: 0.30,
                a: 1.0,
            })
        };
        let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shell-shot.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        let mut pass = pass.forget_lifetime();
        renderer.render(&mut pass, &jobs, &screen);
    }
    queue.submit(user_cmds.into_iter().chain(std::iter::once(enc.finish())));

    let rgba = read_pixels(device, queue, &texture);
    save_png(path, &rgba);
    eprintln!("wrote {path}");
}

fn read_pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let bpp = 4u32;
    let unpadded = W * bpp;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shell-shot.readback"),
        size: (padded * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("shell-shot.readback_encoder"),
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

#[test]
#[ignore = "needs a GPU + writes PNGs; run with --ignored"]
fn shell_screens_to_png() {
    let (device, queue) = headless();
    // The workspace target dir (cargo test runs with the crate dir as CWD, so a bare relative
    // "target/…" would create a stray `app/target/` beside the real build tree).
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/shell-shots");
    std::fs::create_dir_all(dir).expect("create shots dir");

    let campaign = gonedark_engine::mission_registry::default_campaign();
    let mut settings = SettingsState::default();
    let mut profile = ProfileState::default();
    let army = ArmySelectState::default();
    let loadout = gonedark_engine::loadout_ui::LoadoutEditor::new();
    let mut rebinding = None;
    let mut conflict = None;

    // The hub with real state: default identity + the NEXT OPERATION card derived from the
    // shipped campaign (fresh progress → first node).
    let next = next_operation(&campaign);
    shoot(&device, &queue, &format!("{dir}/title.png"), |ui| {
        title_ui(ui, "build dev \u{00b7} v0.0.0", &profile, &army, next.as_ref());
    });
    shoot(&device, &queue, &format!("{dir}/pvp.png"), |ui| {
        pvp_ui(ui, gonedark_core::components::Army::Us);
    });
    shoot(&device, &queue, &format!("{dir}/skirmish_setup.png"), |ui| {
        skirmish_setup_ui(ui, &SkirmishSetupState::default());
    });
    shoot(&device, &queue, &format!("{dir}/loadout.png"), |ui| {
        loadout_ui(ui, &loadout);
    });
    shoot(&device, &queue, &format!("{dir}/operations.png"), |ui| {
        mission_select_ui(ui, &campaign);
    });
    // The hub over the LIVE atlas globe (D103) — exercises the real globe WGSL headlessly.
    let pins = atlas_pins(&campaign);
    shoot_over_globe(&device, &queue, &format!("{dir}/operations_globe.png"), &pins, |ui| {
        mission_select_ui(ui, &campaign);
    });
    shoot(&device, &queue, &format!("{dir}/settings.png"), |ui| {
        settings_ui(ui, &mut settings, false, &mut rebinding, &mut conflict);
    });
    shoot(&device, &queue, &format!("{dir}/profile.png"), |ui| {
        profile_ui(ui, &mut profile);
    });
    shoot(&device, &queue, &format!("{dir}/army.png"), |ui| {
        army_select_ui(ui, &army);
    });
    shoot(&device, &queue, &format!("{dir}/about.png"), |ui| {
        about_ui(ui, "build dev \u{00b7} v0.0.0");
    });
    shoot(&device, &queue, &format!("{dir}/briefing.png"), |ui| {
        briefing_ui(ui, &campaign, NodeId(0), Difficulty::Recruit);
    });
}
