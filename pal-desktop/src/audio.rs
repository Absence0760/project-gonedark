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
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SizedSample};
    use gonedark_pal::mix::{oneshot_sound, synth_bank, voice_from_cue, Mixer};
    use gonedark_pal::{Audio, AudioCue, SoundId};

    /// Active output: the live stream (kept alive by ownership), the shared mixer, and the
    /// synthesized cue bank. The per-voice render math (pan/gain/muffle/sum/eviction) lives in the
    /// shared, host-tested `gonedark_pal::mix` seam — this backend only owns the cpal stream glue.
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
        /// `gain`, low-passed when `muffled`. The pan/gain/muffle derivation is the shared
        /// `gonedark_pal::mix::voice_from_cue`; this only looks up the synthesized buffer and
        /// pushes the voice (never blocking the realtime thread holds elsewhere).
        fn queue(&self, sound: SoundId, azimuth: f32, gain: f32, muffled: bool) {
            let Some(active) = &self.inner else { return };
            let Some(samples) = active.bank.get(&sound) else {
                return;
            };
            let voice = voice_from_cue(Arc::clone(samples), azimuth, gain, muffled);
            if let Ok(mut mixer) = active.mixer.lock() {
                mixer.push(voice);
            }
        }
    }

    impl Audio for DesktopAudio {
        fn play_oneshot(&mut self, sound_id: u32) {
            // Legacy fire-and-forget path: map the opaque id onto a cue, centered at full gain.
            self.queue(oneshot_sound(sound_id), 0.0, 0.9, false);
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
}

pub use backend::DesktopAudio;
