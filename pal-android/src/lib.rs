//! Android PAL backend — cargo-ndk + JNI shim (the ship target, platforms.md §8).
//!
//! The entire crate is gated to `target_os = "android"`, so it compiles to an empty lib on
//! desktop/CI hosts and never drags Android deps into the host build. Build-order step 6
//! wires the real `android-activity` entry point, the JNI surface/touch/lifecycle bridge,
//! and the wgpu→Vulkan 1.1 backend here, implementing the `gonedark-pal` traits.
#![cfg(target_os = "android")]

// TODO(phase1-step6):
//   - #[no_mangle] android_main(app: AndroidApp) via the `android-activity` crate
//   - JNI shim: surface lifecycle (create/resize/lost on resume), touch + gyro events
//   - implement gonedark_pal::{Window, Input, Rhi, Audio, Storage} backed by
//     AAudio + wgpu(Vulkan) + AAssetManager
//   - frame pacing via Swappy
