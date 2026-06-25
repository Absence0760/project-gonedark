// Link the Android C++ shared runtime.
//
// `oboe` (our low-latency audio sink, D29) pulls in `oboe-sys`, which compiles Oboe's C++
// via `cc`. That leaves C++ ABI symbols undefined in this cdylib — operator new/delete
// (`_Znwm`/`_ZdlPv`), the Itanium C++ ABI helpers (`__cxa_pure_virtual`, `__cxa_throw`,
// the guard/exception entry points). Nothing else in the link references libc++, so without
// the directive below the cdylib carries NO `DT_NEEDED` for `libc++_shared.so` and those
// symbols stay unresolved. At app launch `NativeActivity` then dies in `dlopen` with:
//
//     java.lang.UnsatisfiedLinkError: dlopen failed:
//         cannot locate symbol "__cxa_pure_virtual" referenced by libgonedark_pal_android.so
//
// which ejects the player straight back to the title screen — i.e. "Start does nothing".
//
// Emitting the link here adds the `DT_NEEDED`, so the loader pulls in libc++_shared.so and
// resolves the symbols. The matching libc++_shared.so itself is copied into the APK from the
// NDK by the Gradle `copyCxxShared` task (the apk/install/run path) and by
// scripts/android.sh (the direct `cargo ndk build` path) — it must be PRESENT in the APK, or
// the new DT_NEEDED just changes the error to "library libc++_shared.so not found".
//
// Gated on the android target: on a desktop/CI host this crate is `#![cfg(target_os =
// "android")]`-empty and drags in none of the audio deps, so it must not link libc++ there.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-lib=dylib=c++_shared");
    }
}
