//! Android PAL backend — `cargo-ndk` + `android-activity` + JNI shim (the ship target,
//! platforms.md §8).
//!
//! ## Crate shape (Phase 4 WS-C split)
//! Historically the *entire* crate was `#![cfg(target_os = "android")]`, compiling to an empty
//! lib on a desktop/CI host. That kept the Android deps off the host build — but it also meant
//! **nothing** in this crate could be unit-tested on the host. WS-C needs a host-tested seam: the
//! thermal/battery integer → [`gonedark_pal::ThermalState`]/[`gonedark_pal::PowerState`] mapping.
//! So the gate moved one level down, mirroring how the audio render math lives in the host-tested
//! `gonedark_pal::mix` seam while only the oboe stream glue is android-gated:
//!
//!   * [`thermal`] — pure, **host-compiled** mapping logic (no Android deps) + its exhaustive
//!     unit tests, plus the `#[cfg(target_os = "android")]` JNI sensor that *consumes* the
//!     mapping (`PowerManager.getThermalStatus()` + `BatteryManager` over JNI).
//!   * `android_backend` — the rest of the backend (entry point, lifecycle, window/input/RHI/
//!     audio/storage glue) stays `#[cfg(target_os = "android")]`, so the host build still drags
//!     in none of the `android-activity`/`ndk`/`wgpu`/`oboe`/`jni` deps.
//!
//! On a host target this crate now compiles just the host-safe parts of `thermal`; on
//! `aarch64-linux-android` it additionally compiles `android_backend` + the JNI thermal reader.

// Pure thermal/battery mapping seam + the real Android sensor (Phase 4 WS-C). The mapping fns are
// host-compiled and unit-tested in the module; the JNI reader inside it is android-gated.
pub mod thermal;

// Pure launch-config codec (Compose shell parity, Tier 0). Host-compiled + unit-tested here; the
// JNI reader that feeds it the live `Intent` extra is android-gated in `android_backend`.
pub mod launch;

// The Android backend proper — entry point, lifecycle, surface, input, audio, storage. Android-
// target-only; on a host it is absent and the crate is just the `thermal` mapping seam. The
// re-export keeps the integrator's `pub use gonedark_pal_android::*;` (see the note at the foot of
// `android_backend.rs`) and the `#[no_mangle] android_main` symbol reachable as before.
#[cfg(target_os = "android")]
mod android_backend;
#[cfg(target_os = "android")]
pub use android_backend::*;
