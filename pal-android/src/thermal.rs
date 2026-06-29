//! Real Android thermal/battery sensor (Phase 4 WS-C) + its pure, host-testable mapping seam.
//!
//! This is the `pal-android` realization of the **OWED** WS-C reader (phase-4-plan §C step 3):
//! `PowerManager.getThermalStatus()` → [`ThermalState`] and `BatteryManager` → [`PowerState`],
//! read over JNI — the source of the on-device numbers that may reopen the D21 dual-rate question.
//!
//! It is split along the CLAUDE.md *"extract the pure logic to a testable seam"* rule, exactly the
//! way `gonedark_pal::mix` is host-tested while only the oboe stream glue in this crate is android-
//! gated:
//!
//!   * **Pure mapping** ([`thermal_state_from_status`], [`power_state_from_battery`]) — plain
//!     integer/field → enum logic with **no Android deps**. It compiles and is unit-tested on every
//!     host target (see the `tests` module at the foot of this file), exhaustively over every
//!     in-range constant *and* out-of-range inputs (each falls back to a safe default).
//!   * **JNI glue** ([`AndroidThermalSensor`], `#[cfg(target_os = "android")]`) — the thin part
//!     that fetches those raw integers from the OS via a live `JNIEnv` + the Android `Context`.
//!     This is **genuinely un-constructible without a device/emulator** (there is no way to mint a
//!     real `JNIEnv` / `Activity` jobject on a host), so — like the oboe `on_audio_ready` callback
//!     and the wgpu surface bridge in `android_backend.rs` — it is **exempt from unit coverage**.
//!     It is written against the pinned `jni` 0.21 / `android-activity` 0.6 APIs and, like the rest
//!     of this crate, is **NOT device-verified**; it fails safe (any JNI error → the same defaults
//!     the pure mapper returns for an unreadable sensor), never panicking.

use gonedark_pal::{PowerState, ThermalState};

// ---------------------------------------------------------------------------------------
// Android constant domains (mirrored so the pure mapper needn't do a JNI field lookup).
// ---------------------------------------------------------------------------------------

/// `PowerManager.THERMAL_STATUS_*` constants (Android API 29+). The argument domain of
/// [`thermal_state_from_status`]. Mirrored as plain consts so the pure mapping fn is host-testable
/// without reaching through JNI to read the static fields.
pub mod thermal_status {
    /// No thermal pressure.
    pub const NONE: i32 = 0;
    /// Light throttling — generally imperceptible.
    pub const LIGHT: i32 = 1;
    /// Moderate throttling — may become user-visible.
    pub const MODERATE: i32 = 2;
    /// Severe throttling — device is very warm, heavy backoff.
    pub const SEVERE: i32 = 3;
    /// Platform has done all it can; load-shedding in progress.
    pub const CRITICAL: i32 = 4;
    /// Battery/temperature emergency — shutdown imminent to protect the device.
    pub const EMERGENCY: i32 = 5;
    /// Device is shutting down to protect itself.
    pub const SHUTDOWN: i32 = 6;
}

/// `BatteryManager.BATTERY_STATUS_*` values (the `BATTERY_PROPERTY_STATUS` int property, API 26+).
/// The status domain of [`power_state_from_battery`].
pub mod battery_status {
    /// Status could not be determined.
    pub const UNKNOWN: i32 = 1;
    /// Charging from an external source.
    pub const CHARGING: i32 = 2;
    /// On battery (discharging).
    pub const DISCHARGING: i32 = 3;
    /// Plugged but not charging (e.g. full-but-not-"FULL", or charge-limited).
    pub const NOT_CHARGING: i32 = 4;
    /// Battery full (on external power).
    pub const FULL: i32 = 5;
}

/// `BatteryManager.BATTERY_PROPERTY_*` ids passed to `getIntProperty(int)`.
pub mod battery_property {
    /// Remaining battery capacity, **percent** in `[0,100]` (or negative if not supported).
    pub const CAPACITY: i32 = 4;
    /// Charging status — one of the [`super::battery_status`] values.
    pub const STATUS: i32 = 6;
}

// ---------------------------------------------------------------------------------------
// Pure mapping (host-compiled + unit-tested) — the testable seam.
// ---------------------------------------------------------------------------------------

/// Collapse an Android `PowerManager.getThermalStatus()` int onto the PAL's coarse four-level
/// [`ThermalState`] (which mirrors the iOS `ProcessInfo.thermalState` shape). Monotonic: hotter
/// Android status never maps to a cooler PAL bucket.
///
/// | Android status                | [`ThermalState`] | rationale                                  |
/// |-------------------------------|------------------|--------------------------------------------|
/// | `NONE` (0)                    | `Nominal`        | full render freedom                        |
/// | `LIGHT` (1)                   | `Fair`           | trim obvious waste (don't outrun the tick) |
/// | `MODERATE` (2)                | `Serious`        | user-visible throttling — shed render cost |
/// | `SEVERE` (3)                  | `Critical`       | heavy active throttling — survival mode     |
/// | `CRITICAL`/`EMERGENCY`/`SHUTDOWN` (4/5/6) | `Critical` | platform load-shedding / shutdown      |
/// | anything else (incl. `-1`)    | `Nominal`        | **fail-safe**: an unreadable/"not supported" sensor must never throttle rendering on its own |
pub fn thermal_state_from_status(status: i32) -> ThermalState {
    use thermal_status as ts;
    match status {
        ts::NONE => ThermalState::Nominal,
        ts::LIGHT => ThermalState::Fair,
        ts::MODERATE => ThermalState::Serious,
        ts::SEVERE | ts::CRITICAL | ts::EMERGENCY | ts::SHUTDOWN => ThermalState::Critical,
        // Unknown / out-of-range, including the -1 some devices return for an unsupported HAL:
        // fail safe to Nominal so a sensor we can't read never costs the player frame rate.
        _ => ThermalState::Nominal,
    }
}

/// Build a [`PowerState`] from `BatteryManager` integer properties:
///   * `status` — `BATTERY_PROPERTY_STATUS` (a [`battery_status`] value). `CHARGING`/`FULL` ⇒ on
///     external power; everything else (incl. `UNKNOWN`/out-of-range) ⇒ on battery.
///   * `capacity_percent` — `BATTERY_PROPERTY_CAPACITY` in `[0,100]`. In range ⇒ `Some(p/100)`
///     in `[0,1]`; out of range (incl. the `-1` Android reports when unsupported) ⇒ `None`, the
///     same "charge unknown" hint the trait's default returns.
pub fn power_state_from_battery(status: i32, capacity_percent: i32) -> PowerState {
    let on_external_power = matches!(status, battery_status::CHARGING | battery_status::FULL);
    let charge = if (0..=100).contains(&capacity_percent) {
        Some(capacity_percent as f32 / 100.0)
    } else {
        None
    };
    PowerState {
        on_external_power,
        charge,
    }
}

// ---------------------------------------------------------------------------------------
// JNI reader (android-only, un-constructible on a host → exempt from unit coverage).
// ---------------------------------------------------------------------------------------

/// Real Android [`ThermalSensor`](gonedark_pal::ThermalSensor) backed by `PowerManager` +
/// `BatteryManager`, read over JNI (Phase 4 WS-C). Holds the [`AndroidApp`](android_activity::AndroidApp)
/// so it can reach the running `Context`/`JavaVM` each poll.
///
/// All the decision logic lives in the host-tested pure mappers above; this struct is only the
/// un-testable JNI fetch. It **fails safe** — any attach/lookup/exception yields
/// [`ThermalState::Nominal`] / the default [`PowerState`] (charge unknown, on external power),
/// matching what the pure mappers return for an unreadable sensor — and it **never panics**
/// (invariant #8 robustness, same posture as `AndroidAudio`).
///
/// # NOT device-verified
/// Written against the pinned `jni` 0.21 + `android-activity` 0.6 APIs; the JNI method
/// signatures and the `vm_as_ptr`/`activity_as_ptr` handles need an on-device/emulator shakeout.
/// `getThermalStatus()` needs API 29+; `getIntProperty(STATUS)` needs API 26+.
#[cfg(target_os = "android")]
pub struct AndroidThermalSensor {
    app: android_activity::AndroidApp,
}

#[cfg(target_os = "android")]
impl AndroidThermalSensor {
    /// Build the sensor from the live app handle (cheap; the JNI calls happen per poll).
    pub fn new(app: android_activity::AndroidApp) -> Self {
        AndroidThermalSensor { app }
    }

    /// Fetch the raw `(thermal_status, battery_status, capacity_percent)` ints over JNI.
    /// Returns `Err` on any JNI failure so the public trait methods can fall back to safe
    /// defaults. Kept private + `Result`-typed so the only un-testable surface is this fetch.
    fn read_raw(&self) -> Result<(i32, i32, i32), jni::errors::Error> {
        use jni::objects::{JObject, JValue};
        use jni::JavaVM;

        // SAFETY: the pointers come from `android-activity`'s live `AndroidApp`, valid for the
        // process lifetime while the activity is running.
        let vm = unsafe { JavaVM::from_raw(self.app.vm_as_ptr() as *mut jni::sys::JavaVM)? };
        let mut env = vm.attach_current_thread()?;
        let activity =
            unsafe { JObject::from_raw(self.app.activity_as_ptr() as jni::sys::jobject) };

        // context.getSystemService("power") -> PowerManager; .getThermalStatus() : int
        let power_name = env.new_string("power")?;
        let power_mgr = env
            .call_method(
                &activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[(&power_name).into()],
            )?
            .l()?;
        let thermal_status = env
            .call_method(&power_mgr, "getThermalStatus", "()I", &[])?
            .i()?;

        // context.getSystemService("batterymanager") -> BatteryManager; .getIntProperty(int):int
        let battery_name = env.new_string("batterymanager")?;
        let battery_mgr = env
            .call_method(
                &activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[(&battery_name).into()],
            )?
            .l()?;
        let battery_status = env
            .call_method(
                &battery_mgr,
                "getIntProperty",
                "(I)I",
                &[JValue::Int(battery_property::STATUS)],
            )?
            .i()?;
        let capacity = env
            .call_method(
                &battery_mgr,
                "getIntProperty",
                "(I)I",
                &[JValue::Int(battery_property::CAPACITY)],
            )?
            .i()?;

        Ok((thermal_status, battery_status, capacity))
    }
}

#[cfg(target_os = "android")]
impl gonedark_pal::ThermalSensor for AndroidThermalSensor {
    fn thermal_state(&self) -> ThermalState {
        match self.read_raw() {
            Ok((thermal_status, _, _)) => thermal_state_from_status(thermal_status),
            Err(e) => {
                // Fail safe: an unreadable sensor must not throttle rendering on its own.
                log::warn!("[thermal] read failed ({e:?}); defaulting to Nominal");
                ThermalState::Nominal
            }
        }
    }

    fn power_state(&self) -> PowerState {
        match self.read_raw() {
            Ok((_, battery_status, capacity)) => power_state_from_battery(battery_status, capacity),
            Err(e) => {
                log::warn!("[thermal] battery read failed ({e:?}); defaulting to on-external");
                PowerState {
                    on_external_power: true,
                    charge: None,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Tests — the pure mappers, host-compiled. (The JNI fetch above is exempt: a real `JNIEnv` /
// `Activity` jobject cannot be constructed off a device, exactly like the winit `KeyEvent` /
// android `MotionEvent` seams the CLAUDE.md testing rule carves out.)
// ---------------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thermal_maps_every_in_range_status() {
        use thermal_status as ts;
        assert_eq!(thermal_state_from_status(ts::NONE), ThermalState::Nominal);
        assert_eq!(thermal_state_from_status(ts::LIGHT), ThermalState::Fair);
        assert_eq!(thermal_state_from_status(ts::MODERATE), ThermalState::Serious);
        assert_eq!(thermal_state_from_status(ts::SEVERE), ThermalState::Critical);
        assert_eq!(thermal_state_from_status(ts::CRITICAL), ThermalState::Critical);
        assert_eq!(thermal_state_from_status(ts::EMERGENCY), ThermalState::Critical);
        assert_eq!(thermal_state_from_status(ts::SHUTDOWN), ThermalState::Critical);
    }

    #[test]
    fn thermal_is_monotonic_non_decreasing() {
        // Hotter Android status never maps to a cooler PAL bucket (ordering by enum severity).
        fn rank(s: ThermalState) -> u8 {
            match s {
                ThermalState::Nominal => 0,
                ThermalState::Fair => 1,
                ThermalState::Serious => 2,
                ThermalState::Critical => 3,
            }
        }
        let mut prev = 0u8;
        for status in 0..=6 {
            let r = rank(thermal_state_from_status(status));
            assert!(r >= prev, "status {status} regressed severity {r} < {prev}");
            prev = r;
        }
    }

    #[test]
    fn thermal_out_of_range_fails_safe_to_nominal() {
        for status in [-1, -100, 7, 8, 42, 1000, i32::MIN, i32::MAX] {
            assert_eq!(
                thermal_state_from_status(status),
                ThermalState::Nominal,
                "out-of-range status {status} must fail safe to Nominal"
            );
        }
    }

    #[test]
    fn power_external_power_only_for_charging_or_full() {
        use battery_status as bs;
        assert!(power_state_from_battery(bs::CHARGING, 50).on_external_power);
        assert!(power_state_from_battery(bs::FULL, 100).on_external_power);
        assert!(!power_state_from_battery(bs::DISCHARGING, 50).on_external_power);
        assert!(!power_state_from_battery(bs::NOT_CHARGING, 50).on_external_power);
        assert!(!power_state_from_battery(bs::UNKNOWN, 50).on_external_power);
        // Out-of-range status is treated as "on battery" (not external).
        for bogus in [-1, 0, 6, 99, i32::MIN, i32::MAX] {
            assert!(!power_state_from_battery(bogus, 50).on_external_power);
        }
    }

    #[test]
    fn power_capacity_maps_to_unit_fraction_in_range() {
        assert_eq!(
            power_state_from_battery(battery_status::DISCHARGING, 0).charge,
            Some(0.0)
        );
        assert_eq!(
            power_state_from_battery(battery_status::DISCHARGING, 50).charge,
            Some(0.5)
        );
        assert_eq!(
            power_state_from_battery(battery_status::DISCHARGING, 100).charge,
            Some(1.0)
        );
        assert_eq!(
            power_state_from_battery(battery_status::CHARGING, 73).charge,
            Some(0.73)
        );
    }

    #[test]
    fn power_capacity_out_of_range_is_none() {
        for capacity in [-1, -100, 101, 200, i32::MIN, i32::MAX] {
            assert_eq!(
                power_state_from_battery(battery_status::DISCHARGING, capacity).charge,
                None,
                "out-of-range capacity {capacity} must report None (charge unknown)"
            );
        }
    }
}
