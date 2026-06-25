//! Desktop audio backend (the [`gonedark_pal::Audio`] seam). The *mix* — which sounds, where,
//! how loud, what's muffled — is computed platform-free in `engine::audio`; this backend only
//! RENDERS that mix. The embodied "strategic sound bleeding into FPS" model (invariant #6) is the
//! same everywhere; only the output path is per-platform.
//!
//! Output is behind the opt-in `audio` feature (default OFF) so a bare build needs no system
//! audio dev headers (invariant #8 — clone-and-run). With the feature on, this opens a `cpal`
//! output stream and synthesizes a short procedural cue per [`SoundId`] (no audio assets yet),
//! pans it by `azimuth`, scales it by `gain`, and low-passes `muffled` (off-map) cues. Audio is
//! never load-bearing for the sim, so any device/stream failure degrades to a silent no-op
//! rather than panicking.

#[cfg(not(feature = "audio"))]
mod backend {
    use gonedark_pal::{Audio, AudioCue};

    /// Silent sink — the default build links no audio system libs.
    #[derive(Default)]
    pub struct DesktopAudio;

    impl DesktopAudio {
        pub fn new() -> Self {
            DesktopAudio
        }
    }

    impl Audio for DesktopAudio {
        fn play_oneshot(&mut self, _sound_id: u32) {}
        fn submit_mix(&mut self, _cues: &[AudioCue]) {}
    }
}

#[cfg(feature = "audio")]
mod backend {
    use std::collections::HashMap;
    use std::f32::consts::PI;
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SizedSample};
    use gonedark_pal::{Audio, AudioCue, SoundId};

    /// Max simultaneous voices; beyond this the oldest finished/started are dropped so a burst of
    /// fire can't grow the mix unbounded.
    const MAX_VOICES: usize = 24;

    /// One playing sound: a shared synthesized buffer plus a cursor, per-ear gain, and a one-pole
    /// low-pass state (`alpha == 1.0` is a pass-through; `< 1.0` muffles the off-map bleed).
    struct Voice {
        samples: Arc<Vec<f32>>,
        pos: usize,
        gain_l: f32,
        gain_r: f32,
        lp_l: f32,
        lp_r: f32,
        alpha: f32,
    }

    /// The shared mix state read by the realtime audio callback and written by the game thread.
    #[derive(Default)]
    struct Mixer {
        voices: Vec<Voice>,
    }

    impl Mixer {
        /// Sum one stereo frame from all live voices, advancing + low-passing each. Finished
        /// voices contribute nothing (pruned lazily on `push`).
        fn next_frame(&mut self) -> (f32, f32) {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for v in &mut self.voices {
                if v.pos >= v.samples.len() {
                    continue;
                }
                let s = v.samples[v.pos];
                v.pos += 1;
                v.lp_l += v.alpha * (s * v.gain_l - v.lp_l);
                v.lp_r += v.alpha * (s * v.gain_r - v.lp_r);
                l += v.lp_l;
                r += v.lp_r;
            }
            // Soft clamp to avoid clipping when several cues stack.
            (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0))
        }

        fn push(&mut self, v: Voice) {
            if self.voices.len() >= MAX_VOICES {
                self.voices.retain(|x| x.pos < x.samples.len());
            }
            if self.voices.len() >= MAX_VOICES {
                self.voices.remove(0); // still full → drop the oldest
            }
            self.voices.push(v);
        }
    }

    /// Active output: the live stream (kept alive by ownership), the shared mixer, the synthesized
    /// cue bank, and the device sample rate (for nothing beyond bookkeeping).
    struct Active {
        _stream: cpal::Stream,
        mixer: Arc<Mutex<Mixer>>,
        bank: HashMap<SoundId, Arc<Vec<f32>>>,
    }

    /// Desktop audio sink. `inner` is `None` when no device/stream could be opened — the sink then
    /// silently drops everything (audio is never load-bearing).
    pub struct DesktopAudio {
        inner: Option<Active>,
    }

    impl Default for DesktopAudio {
        fn default() -> Self {
            Self::new()
        }
    }

    impl DesktopAudio {
        pub fn new() -> Self {
            match Active::open() {
                Ok(active) => DesktopAudio {
                    inner: Some(active),
                },
                Err(e) => {
                    eprintln!("[audio] disabled (silent): {e}");
                    DesktopAudio { inner: None }
                }
            }
        }

        /// Queue one voice for `sound`, panned by `azimuth` (0 = ahead, + = right), scaled by
        /// `gain`, low-passed when `muffled`.
        fn queue(&self, sound: SoundId, azimuth: f32, gain: f32, muffled: bool) {
            let Some(active) = &self.inner else { return };
            let Some(samples) = active.bank.get(&sound) else {
                return;
            };
            // Equal-power pan: map azimuth's lateral component to [0, PI/2].
            let pan = azimuth.sin().clamp(-1.0, 1.0);
            let angle = (pan + 1.0) * 0.25 * PI;
            let g = gain.clamp(0.0, 1.0);
            let voice = Voice {
                samples: Arc::clone(samples),
                pos: 0,
                gain_l: angle.cos() * g,
                gain_r: angle.sin() * g,
                lp_l: 0.0,
                lp_r: 0.0,
                alpha: if muffled { 0.12 } else { 1.0 },
            };
            if let Ok(mut mixer) = active.mixer.lock() {
                mixer.push(voice);
            }
        }
    }

    impl Audio for DesktopAudio {
        fn play_oneshot(&mut self, sound_id: u32) {
            // Legacy fire-and-forget path: map the opaque id onto a cue, centered at full gain.
            let sound = match sound_id {
                1 => SoundId::UnitDown,
                2 => SoundId::BaseHit,
                3 => SoundId::Capture,
                4 => SoundId::ProductionReady,
                _ => SoundId::Gunfire,
            };
            self.queue(sound, 0.0, 0.9, false);
        }

        fn submit_mix(&mut self, cues: &[AudioCue]) {
            for c in cues {
                self.queue(c.sound, c.azimuth, c.gain, c.muffled);
            }
        }
    }

    impl Active {
        fn open() -> Result<Active, String> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or("no default output device")?;
            let supported = device
                .default_output_config()
                .map_err(|e| format!("default_output_config: {e}"))?;
            let sample_format = supported.sample_format();
            let config: cpal::StreamConfig = supported.into();
            let sample_rate = config.sample_rate.0;
            let channels = config.channels as usize;

            let mixer = Arc::new(Mutex::new(Mixer::default()));
            let bank = synth_bank(sample_rate);

            let stream = build_stream(&device, &config, sample_format, Arc::clone(&mixer))?;
            stream.play().map_err(|e| format!("stream.play: {e}"))?;
            let _ = channels; // channel handling lives in the callback
            Ok(Active {
                _stream: stream,
                mixer,
                bank,
            })
        }
    }

    /// Build the output stream for the device's native sample format, writing frames from `mixer`.
    fn build_stream(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        format: cpal::SampleFormat,
        mixer: Arc<Mutex<Mixer>>,
    ) -> Result<cpal::Stream, String> {
        let channels = config.channels as usize;
        match format {
            cpal::SampleFormat::F32 => make::<f32>(device, config, channels, mixer),
            cpal::SampleFormat::I16 => make::<i16>(device, config, channels, mixer),
            cpal::SampleFormat::U16 => make::<u16>(device, config, channels, mixer),
            other => Err(format!("unsupported sample format {other:?}")),
        }
    }

    fn make<T: SizedSample + FromSample<f32>>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        channels: usize,
        mixer: Arc<Mutex<Mixer>>,
    ) -> Result<cpal::Stream, String> {
        device
            .build_output_stream(
                config,
                move |out: &mut [T], _| {
                    // Never block the realtime thread: if the game thread holds the lock, emit a
                    // frame of silence (rare; submit_mix's critical section is tiny).
                    if let Ok(mut m) = mixer.try_lock() {
                        for frame in out.chunks_mut(channels) {
                            let (l, r) = m.next_frame();
                            for (i, s) in frame.iter_mut().enumerate() {
                                let v = match i {
                                    0 => l,
                                    1 => r,
                                    _ => 0.0,
                                };
                                *s = T::from_sample(v);
                            }
                        }
                    } else {
                        for s in out.iter_mut() {
                            *s = T::from_sample(0.0f32);
                        }
                    }
                },
                |e| eprintln!("[audio] stream error: {e}"),
                None,
            )
            .map_err(|e| format!("build_output_stream: {e}"))
    }

    // --- procedural cue synthesis (no audio assets yet) -----------------------------------------

    /// Synthesize a short buffer per sound at the device sample rate. Kept intentionally simple
    /// and recognizable; amplitudes stay ~0.5 so a few stacked cues don't clip.
    fn synth_bank(sr: u32) -> HashMap<SoundId, Arc<Vec<f32>>> {
        let mut bank = HashMap::new();
        bank.insert(SoundId::Gunfire, Arc::new(gunfire(sr)));
        bank.insert(SoundId::UnitDown, Arc::new(unit_down(sr)));
        bank.insert(SoundId::BaseHit, Arc::new(base_hit(sr)));
        bank.insert(SoundId::Capture, Arc::new(capture(sr)));
        bank.insert(SoundId::ProductionReady, Arc::new(production_ready(sr)));
        bank
    }

    fn secs(sr: u32, s: f32) -> usize {
        (sr as f32 * s) as usize
    }
    fn sine(sr: u32, i: usize, freq: f32) -> f32 {
        (2.0 * PI * freq * i as f32 / sr as f32).sin()
    }
    /// Tiny xorshift noise (audio noise need not be deterministic).
    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// A snappy noise burst with a fast exponential decay.
    fn gunfire(sr: u32) -> Vec<f32> {
        let n = secs(sr, 0.09);
        let tau = sr as f32 * 0.02;
        let mut st = 0x1234_5678u32;
        (0..n)
            .map(|i| noise(&mut st) * 0.5 * (-(i as f32) / tau).exp())
            .collect()
    }

    /// A descending tone — a unit falling.
    fn unit_down(sr: u32) -> Vec<f32> {
        let n = secs(sr, 0.28);
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                let freq = 380.0 - 240.0 * t; // 380 → 140 Hz
                sine(sr, i, freq) * 0.45 * (1.0 - t)
            })
            .collect()
    }

    /// A low thud + noise — a building being hit.
    fn base_hit(sr: u32) -> Vec<f32> {
        let n = secs(sr, 0.2);
        let tau = sr as f32 * 0.06;
        let mut st = 0x9E37_79B9u32;
        (0..n)
            .map(|i| {
                let env = (-(i as f32) / tau).exp();
                (sine(sr, i, 90.0) * 0.5 + noise(&mut st) * 0.2) * env
            })
            .collect()
    }

    /// A rising two-tone chime — a control point captured.
    fn capture(sr: u32) -> Vec<f32> {
        let n = secs(sr, 0.22);
        let half = n / 2;
        (0..n)
            .map(|i| {
                let freq = if i < half { 620.0 } else { 930.0 };
                let t = i as f32 / n as f32;
                sine(sr, i, freq) * 0.4 * (1.0 - t * 0.5)
            })
            .collect()
    }

    /// A short high blip — production finished.
    fn production_ready(sr: u32) -> Vec<f32> {
        let n = secs(sr, 0.07);
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                sine(sr, i, 1050.0) * 0.4 * (1.0 - t)
            })
            .collect()
    }
}

pub use backend::DesktopAudio;
