//! The **storage path seam** — the pure, host-testable half of [`AndroidStorage`]
//! (`android_backend`).
//!
//! ## What this module is — and is NOT
//!
//! Android persistence has two backends: **user data** (settings, resume snapshots, replays) lives
//! under the app's private *internal files dir* and is read/written with plain `std::fs`; **bundled
//! read-only assets** (baked maps, etc.) live in the APK and are read through the NDK
//! `AAssetManager`. Both are keyed by the same opaque `&str` the engine/game hands
//! [`gonedark_pal::Storage`]. This module owns the pure part of turning that key into a safe on-disk
//! path / asset path — split along the CLAUDE.md *"extract the pure logic to a testable seam"* rule,
//! exactly the way [`crate::launch`] keeps its wire codec host-compiled while only the JNI reader is
//! android-gated, and [`crate::thermal`] keeps its integer→enum mapping host-compiled while only the
//! JNI sensor is:
//!
//!   * **pure path logic** (this module) — **no Android deps**, std-only. It compiles and is
//!     unit-tested on every host target (the `tests` module below), so the key-sanitization contract
//!     (the load-bearing path-traversal guard, invariant #8) is covered without a device.
//!   * **fs / AAssetManager glue** (`AndroidStorage` in `android_backend`,
//!     `#[cfg(target_os = "android")]`) — the thin part that reads
//!     `AndroidApp::internal_data_path` and opens `AndroidApp::asset_manager` handles; it calls
//!     straight into the functions here for every path it touches, so the two never disagree.
//!
//! ## The safe-key contract (invariant #8: internal-storage paths only)
//!
//! A storage key is produced by trusted game/engine code, never a network peer — but a leak (a
//! `..` traversal, an absolute path, a Windows drive/backslash form) would let a write escape the
//! app's private sandbox. [`safe_relative_key`] is the single guard that refuses all of those, so
//! every path this backend ever forms is provably contained under the app's own dirs.

use std::path::{Path, PathBuf};

/// Subdirectory under the app's internal files dir where our **user data** (settings, resume
/// snapshots, replays) is written. Namespacing our keys under one folder keeps them clear of
/// anything else the process might drop in the files dir, and makes a wipe-our-data path trivial.
pub const USER_DATA_SUBDIR: &str = "gonedark";

/// The longest key we accept (bytes). A key is a short, code-chosen identifier
/// (`"settings"`, `"resume/skirmish"`); anything longer is a bug or an attack, not a real key.
pub const MAX_KEY_LEN: usize = 255;

/// Sanitize a storage `key` into a SAFE relative path (forward-slash separated), or `None` if the
/// key can't be trusted. This is the load-bearing guard behind both the user-data path and the
/// asset path: it refuses everything that could escape the base dir —
///
///   * an empty key, or one longer than [`MAX_KEY_LEN`];
///   * an absolute path (a leading `/`) or a `\`-form (rejected as a non-allowed char);
///   * any `.` or `..` path component (traversal), and any empty component (a leading / trailing /
///     doubled `/`);
///   * any component with a character outside `[A-Za-z0-9._-]` (so no NUL, no drive colon, no
///     separators other than `/`, no whitespace).
///
/// The returned string re-joins the validated components with `/`, so it is a clean relative path
/// safe to `Path::join` under a base dir **and** to hand the `AAssetManager` (which wants a
/// forward-slash relative path with no leading slash).
pub fn safe_relative_key(key: &str) -> Option<String> {
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        return None;
    }
    let mut components: Vec<&str> = Vec::new();
    for component in key.split('/') {
        // Reject empty (leading/trailing/doubled slash), `.` and `..` (traversal).
        if component.is_empty() || component == "." || component == ".." {
            return None;
        }
        // Reject anything but a conservative filename alphabet. This is what refuses NUL bytes,
        // backslashes, drive colons, and whitespace in one shot.
        if !component
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            return None;
        }
        components.push(component);
    }
    Some(components.join("/"))
}

/// The absolute on-disk path a **user-data** `key` maps to, under the app's internal files dir
/// (`files_dir` = `AndroidApp::internal_data_path`). `None` when the key is unsafe (see
/// [`safe_relative_key`]). The caller creates the parent dirs before a write.
///
/// Every user write/read goes through here, so a key can never resolve outside
/// `files_dir/`[`USER_DATA_SUBDIR`] (invariant #8).
pub fn user_data_path(files_dir: &Path, key: &str) -> Option<PathBuf> {
    let rel = safe_relative_key(key)?;
    Some(files_dir.join(USER_DATA_SUBDIR).join(rel))
}

/// The `AAssetManager`-relative path a `key` resolves to for a **bundled read-only asset**. Same
/// safe-key rule as [`user_data_path`]; returned as a plain forward-slash relative string the
/// android glue wraps in a `CString` and hands `AssetManager::open`. `None` for an unsafe key.
///
/// Kept a distinct named seam (rather than an inline `safe_relative_key` call) so the two storage
/// backends' resolution is independently documented and tested — and so a future asset-root prefix
/// (should the bake layout gain one) has one obvious place to live.
pub fn asset_relative_path(key: &str) -> Option<String> {
    safe_relative_key(key)
}

/// The temp path a write stages into before an atomic rename onto [`user_data_path`]'s target, so a
/// crash mid-write can't leave a torn (unparseable) settings / resume file in place. Appends a fixed
/// suffix to the full path, so it is always a sibling of the target within the same (already-safe)
/// directory.
pub fn write_temp_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_os_string();
    s.push(".tmp.new");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_plain_identifier_key_is_accepted_unchanged() {
        assert_eq!(safe_relative_key("settings").as_deref(), Some("settings"));
        assert_eq!(
            safe_relative_key("resume.snapshot").as_deref(),
            Some("resume.snapshot")
        );
        // A nested key keeps its (validated) slash structure.
        assert_eq!(
            safe_relative_key("replays/skirmish-01").as_deref(),
            Some("replays/skirmish-01")
        );
    }

    #[test]
    fn traversal_and_absolute_and_drive_forms_are_refused() {
        // Path traversal in any position.
        assert_eq!(safe_relative_key(".."), None);
        assert_eq!(safe_relative_key("../secrets"), None);
        assert_eq!(safe_relative_key("a/../../etc/passwd"), None);
        assert_eq!(safe_relative_key("saves/.."), None);
        // A single `.` component is also refused (no reason for it; keeps output canonical).
        assert_eq!(safe_relative_key("."), None);
        assert_eq!(safe_relative_key("a/./b"), None);
        // Absolute paths (leading slash → empty first component).
        assert_eq!(safe_relative_key("/etc/passwd"), None);
        // Windows drive + backslash forms (colon and backslash are outside the alphabet).
        assert_eq!(safe_relative_key("C:\\Windows"), None);
        assert_eq!(safe_relative_key("a\\b"), None);
    }

    #[test]
    fn empty_doubled_and_trailing_slashes_are_refused() {
        assert_eq!(safe_relative_key(""), None);
        assert_eq!(safe_relative_key("/"), None);
        assert_eq!(safe_relative_key("a//b"), None); // doubled → empty middle component
        assert_eq!(safe_relative_key("a/"), None); // trailing → empty last component
        assert_eq!(safe_relative_key("/a"), None); // leading → empty first component
    }

    #[test]
    fn nul_bytes_whitespace_and_odd_chars_are_refused() {
        assert_eq!(safe_relative_key("a\0b"), None);
        assert_eq!(safe_relative_key("with space"), None);
        assert_eq!(safe_relative_key("tab\tkey"), None);
        assert_eq!(safe_relative_key("weird*glob?"), None);
        assert_eq!(safe_relative_key("percent%20"), None);
    }

    #[test]
    fn over_long_keys_are_refused() {
        let ok = "a".repeat(MAX_KEY_LEN);
        assert!(safe_relative_key(&ok).is_some());
        let too_long = "a".repeat(MAX_KEY_LEN + 1);
        assert_eq!(safe_relative_key(&too_long), None);
    }

    #[test]
    fn user_data_path_stays_under_the_files_dir_subdir() {
        let base = Path::new("/data/data/com.jaredhoward.goingdark/files");
        let p = user_data_path(base, "settings").expect("safe key resolves");
        assert_eq!(
            p,
            base.join(USER_DATA_SUBDIR).join("settings"),
            "resolves under files_dir/{USER_DATA_SUBDIR}"
        );
        // The resolved path is always contained by the namespaced subdir (invariant #8).
        assert!(p.starts_with(base.join(USER_DATA_SUBDIR)));
        // A nested key stays contained too.
        let nested = user_data_path(base, "replays/s01").unwrap();
        assert!(nested.starts_with(base.join(USER_DATA_SUBDIR)));
    }

    #[test]
    fn user_data_path_refuses_an_unsafe_key() {
        let base = Path::new("/data/data/com.jaredhoward.goingdark/files");
        assert_eq!(user_data_path(base, "../../etc/passwd"), None);
        assert_eq!(user_data_path(base, "/etc/passwd"), None);
        assert_eq!(user_data_path(base, ""), None);
    }

    #[test]
    fn asset_relative_path_matches_the_safe_key_and_refuses_traversal() {
        assert_eq!(
            asset_relative_path("maps/crossroads.map.ron").as_deref(),
            Some("maps/crossroads.map.ron")
        );
        assert_eq!(asset_relative_path("../secret"), None);
        // No leading slash survives (AAssetManager wants a relative path).
        assert_eq!(asset_relative_path("/maps/x"), None);
    }

    #[test]
    fn write_temp_path_is_a_sibling_of_the_target() {
        let base = Path::new("/data/data/com.jaredhoward.goingdark/files");
        let target = user_data_path(base, "settings").unwrap();
        let tmp = write_temp_path(&target);
        assert_eq!(tmp.parent(), target.parent(), "temp is a sibling");
        assert_ne!(tmp, target, "temp differs from the target");
        assert!(
            tmp.to_string_lossy().ends_with(".tmp.new"),
            "temp carries the staging suffix"
        );
    }
}
