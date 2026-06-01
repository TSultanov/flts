//! Builds the `syncthing-core` Go module into a static c-archive and links it
//! into this crate.
//!
//! Phase 0: the archive currently exposes only `flts_st_ping`. The build logic
//! here (locate the Go module, run `go build -buildmode=c-archive`, emit the
//! link directives + platform system libraries) is the part that has to be
//! right for the real engine to link later, so it is built and exercised now.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // `syncthing-core` is a sibling of `syncthing-sys` at the workspace root.
    let go_dir = crate_dir
        .parent()
        .expect("syncthing-sys has a parent dir")
        .join("syncthing-core");
    assert!(
        go_dir.join("go.mod").exists(),
        "expected Go module at {}",
        go_dir.display()
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Android's cgo only supports `-buildmode=c-shared`, so there we build a
    // shared object and link it dynamically; every other target links a static
    // c-archive (no extra runtime artifact to ship).
    let android = target_os == "android";
    let lib = out_dir.join(if android {
        "libsyncthing_core.so"
    } else {
        "libsyncthing_core.a"
    });

    // Re-run if any Go source or the module manifest changes.
    println!("cargo:rerun-if-changed={}", go_dir.join("go.mod").display());
    // ...or the committed Web-GUI vendor assets (used only by debug builds).
    println!("cargo:rerun-if-changed={}", go_dir.join("webui-vendor").display());
    for entry in std::fs::read_dir(&go_dir).expect("read syncthing-core dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "go") {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    build_archive(&go_dir, &lib, &target_os);

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    if android {
        // Link the shared object (`libsyncthing_core.so` -> `-lsyncthing_core`),
        // giving `libapp_lib.so` a DT_NEEDED on it, then stage it into the Tauri
        // Android project's jniLibs so Gradle packages the two side by side.
        println!("cargo:rustc-link-lib=dylib=syncthing_core");
        stage_android_jnilib(&crate_dir, &lib);
    } else {
        // Link the static archive (`libsyncthing_core.a` -> `-lsyncthing_core`).
        println!("cargo:rustc-link-lib=static=syncthing_core");
    }

    link_platform_libs();
}

fn build_archive(go_dir: &Path, lib: &Path, target_os: &str) {
    let android = target_os == "android";
    let go = env::var("FLTS_GO_BIN").unwrap_or_else(|_| "go".to_string());

    // Debug builds embed Syncthing's real Web GUI so the app can open the
    // dashboard for diagnostics; release builds skip it (see `embed_web_ui`).
    let debug = env::var("PROFILE").as_deref() == Ok("debug");
    let assets_modfile = debug.then(|| embed_web_ui_modfile(&go, go_dir));

    let mut cmd = Command::new(&go);
    cmd.current_dir(go_dir).arg("build");
    match &assets_modfile {
        // Point the build at an alternate go.mod whose `replace` swaps the
        // Syncthing module for a writable copy carrying the generated
        // `lib/api/auto/gui.files.go`; without `noassets`, `auto.Assets()` then
        // serves the full dashboard. (An `-overlay` can't do this: Go forbids
        // overlaying files beneath GOMODCACHE.)
        Some(modfile) => {
            cmd.arg("-modfile").arg(modfile);
        }
        // `noassets`: skip the Web-GUI asset blob. FLTS drives the engine over
        // REST, so the minimal fallback in `lib/api/auto/noassets.go` suffices.
        None => {
            cmd.args(["-tags", "noassets"]);
        }
    }
    cmd.arg(if android {
        "-buildmode=c-shared"
    } else {
        "-buildmode=c-archive"
    })
        .arg("-o")
        .arg(lib)
        .env("CGO_ENABLED", "1");

    // Pin the shared object's SONAME to its bare name so the consumer records a
    // plain `libsyncthing_core.so` DT_NEEDED (not the absolute OUT_DIR path),
    // which Android resolves from the app's own lib directory at load time.
    if android {
        cmd.arg("-ldflags=-extldflags=-Wl,-soname,libsyncthing_core.so");
    }
    cmd.arg(".");

    // Cross-compile for iOS/Android when Cargo is targeting them; otherwise the
    // host go toolchain builds for the host. Tauri builds the app for
    // aarch64-apple-ios (device) and the *-ios-sim / x86_64-apple-ios simulator
    // triples, and for the four Android ABIs (arm64-v8a, armeabi-v7a, x86_64,
    // x86).
    if target_os == "ios" {
        apply_ios_cross_env(&mut cmd);
    } else if android {
        apply_android_cross_env(&mut cmd);
    }

    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn `{go}`: {e}. Is the Go toolchain installed?"));
    assert!(status.success(), "`go build` failed (status {status})");
    assert!(lib.exists(), "go build did not produce {}", lib.display());
}

/// Prepares a writable copy of the Syncthing module carrying the generated
/// Web-GUI asset file, and returns the path to an alternate `go.mod` whose
/// `replace` points the build at that copy. Built without the `noassets` tag,
/// `auto.Assets()` then serves the full dashboard.
///
/// Why a copy + replace rather than an `-overlay`: the upstream
/// `lib/api/auto/gui.files.go` is generated (gitignored) and so absent from the
/// module cache, and Go refuses to `-overlay` any file beneath GOMODCACHE. A
/// local-directory `replace` sidesteps both — it bypasses go.sum for that module
/// and gives us a writable tree to generate into. Host toolchain only (no cross
/// env): `genassets.go` is a standalone `//go:build ignore` file walker.
fn embed_web_ui_modfile(go: &str, go_dir: &Path) -> PathBuf {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Resolve the Syncthing module's source dir in the (immutable) module cache.
    let out = Command::new(go)
        .current_dir(go_dir)
        .args(["list", "-m", "-f", "{{.Dir}}", "github.com/syncthing/syncthing"])
        .output()
        .expect("failed to run `go list -m`");
    assert!(
        out.status.success(),
        "`go list -m github.com/syncthing/syncthing` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let st_src = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());

    // Refresh a writable copy of the module under OUT_DIR. The cache tree is
    // read-only, so `rm -rf` clears any prior (possibly read-only) copy and
    // `cp -R …/.` copies its contents; then make it writable so we can drop the
    // generated file in.
    let st_copy = out_dir.join("syncthing-src");
    run(Command::new("rm").arg("-rf").arg(&st_copy));
    std::fs::create_dir_all(&st_copy)
        .unwrap_or_else(|e| panic!("creating {}: {e}", st_copy.display()));
    run(Command::new("cp").arg("-R").arg(format!("{}/.", st_src.display())).arg(&st_copy));
    run(Command::new("chmod").arg("-R").arg("u+w").arg(&st_copy));

    // The Go module ships the GUI source but not the third-party `vendor/` libs
    // (Go's zip packaging strips nested `vendor/` dirs). Drop our committed copy
    // into the default theme so genassets embeds a fully working dashboard.
    let vendor_src = go_dir.join("webui-vendor");
    let vendor_dst = st_copy.join("gui/default/vendor");
    std::fs::create_dir_all(&vendor_dst)
        .unwrap_or_else(|e| panic!("creating {}: {e}", vendor_dst.display()));
    run(Command::new("cp").arg("-R").arg(format!("{}/.", vendor_src.display())).arg(&vendor_dst));

    // Generate the assets into the copy's `auto` package from its own `gui/`.
    run(Command::new(go)
        .current_dir(go_dir)
        .arg("run")
        .arg(st_copy.join("script/genassets.go"))
        .arg("-o")
        .arg(st_copy.join("lib/api/auto/gui.files.go"))
        .arg(st_copy.join("gui")));

    // Alternate go.mod = the real one plus a replace onto the copy. `-modfile`
    // derives the matching sum file by swapping the extension, so copy go.sum
    // alongside (the replaced module no longer needs a checksum, but the rest do).
    let go_mod = std::fs::read_to_string(go_dir.join("go.mod"))
        .expect("reading syncthing-core/go.mod");
    let alt_mod = out_dir.join("go.webui.mod");
    std::fs::write(
        &alt_mod,
        format!(
            "{go_mod}\nreplace github.com/syncthing/syncthing => {}\n",
            st_copy.display()
        ),
    )
    .unwrap_or_else(|e| panic!("writing {}: {e}", alt_mod.display()));
    std::fs::copy(go_dir.join("go.sum"), out_dir.join("go.webui.sum"))
        .expect("copying go.sum alongside the alternate go.mod");

    alt_mod
}

/// Runs a command, panicking with its stderr on failure.
fn run(cmd: &mut Command) {
    let out = cmd.output().unwrap_or_else(|e| panic!("failed to spawn {cmd:?}: {e}"));
    assert!(
        out.status.success(),
        "{cmd:?} failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Configures the Go build to cross-compile a c-archive for the active iOS
/// target, pointing cgo's clang at the right SDK (device vs simulator).
fn apply_ios_cross_env(cmd: &mut Command) {
    let target = env::var("TARGET").unwrap_or_default();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // Simulator triples are `*-apple-ios-sim` (Apple Silicon) and
    // `x86_64-apple-ios` (Intel); `aarch64-apple-ios` is the device.
    let is_simulator = target.ends_with("-sim") || arch == "x86_64";
    let (sdk, min_flag) = if is_simulator {
        ("iphonesimulator", "-mios-simulator-version-min=13.0")
    } else {
        ("iphoneos", "-miphoneos-version-min=13.0")
    };

    let goarch = match arch.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => panic!("unsupported iOS arch: {other}"),
    };
    let clang_arch = if arch == "aarch64" { "arm64" } else { "x86_64" };

    let sdk_path = xcrun(&["--sdk", sdk, "--show-sdk-path"]);
    let clang = xcrun(&["--sdk", sdk, "--find", "clang"]);
    let cc = format!("{clang} -arch {clang_arch} -isysroot {sdk_path} {min_flag}");

    cmd.env("GOOS", "ios").env("GOARCH", goarch).env("CC", cc);
}

/// Configures the Go build to cross-compile a c-archive for the active Android
/// target, pointing cgo's CC at the matching NDK clang wrapper. Mirrors
/// `apply_ios_cross_env` but for the four Android ABIs.
fn apply_android_cross_env(cmd: &mut Command) {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // Map the Rust target arch to (GOARCH, NDK clang triple, GOARM). The NDK
    // ships per-API clang wrappers named `<triple><api>-clang`; only 32-bit ARM
    // needs GOARM. ABIs: arm64-v8a, armeabi-v7a, x86_64, x86.
    let (goarch, clang_triple, goarm): (&str, &str, Option<&str>) = match arch.as_str() {
        "aarch64" => ("arm64", "aarch64-linux-android", None),
        "arm" => ("arm", "armv7a-linux-androideabi", Some("7")),
        "x86_64" => ("amd64", "x86_64-linux-android", None),
        "x86" => ("386", "i686-linux-android", None),
        other => panic!("unsupported Android arch: {other}"),
    };

    // The cgo target API level must be >= the app's minSdk (see
    // gen/android/app/build.gradle.kts; tauri.conf.json android.minSdkVersion).
    // Overridable for forward-compat, but defaults to match the Rust linker
    // (cargo-tauri links the lib with `<triple>24-clang`).
    println!("cargo:rerun-if-env-changed=FLTS_ANDROID_API");
    let api = env::var("FLTS_ANDROID_API").unwrap_or_else(|_| "24".to_string());

    let bin = ndk_llvm_bin();
    let clang = bin.join(format!("{clang_triple}{api}-clang"));
    assert!(
        clang.exists(),
        "NDK clang not found at {} — is the NDK r23+ and the API level valid?",
        clang.display()
    );

    cmd.env("GOOS", "android")
        .env("GOARCH", goarch)
        .env("CC", &clang);
    if let Some(v) = goarm {
        cmd.env("GOARM", v);
    }
}

/// Locates the NDK's prebuilt LLVM `bin` dir (which holds the clang wrappers),
/// resolving the NDK root from the standard env vars and globbing the single
/// host-tagged prebuilt directory (e.g. `darwin-x86_64`).
fn ndk_llvm_bin() -> PathBuf {
    let ndk = ["NDK_HOME", "ANDROID_NDK_HOME", "ANDROID_NDK_ROOT"]
        .into_iter()
        .find_map(|var| {
            println!("cargo:rerun-if-env-changed={var}");
            env::var(var).ok().filter(|p| !p.is_empty())
        })
        .map(PathBuf::from)
        .expect(
            "Android build needs the NDK: set NDK_HOME (or ANDROID_NDK_HOME) to an NDK r23+",
        );

    let prebuilt = ndk.join("toolchains/llvm/prebuilt");
    let host_tag = std::fs::read_dir(&prebuilt)
        .unwrap_or_else(|e| panic!("reading {}: {e}", prebuilt.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| !n.starts_with('.'))
        .unwrap_or_else(|| panic!("no prebuilt toolchain under {}", prebuilt.display()));

    prebuilt.join(host_tag).join("bin")
}

/// Stages the freshly built `libsyncthing_core.so` into the Tauri Android
/// project's `jniLibs/<abi>/` so Gradle bundles it into the APK next to
/// `libapp_lib.so` (which links against it). `syncthing-sys` lives at the
/// workspace root, so the app's generated Android tree is a fixed sibling path;
/// if it isn't present (e.g. a standalone crate build), staging is skipped and
/// linking still succeeds against the copy in OUT_DIR.
fn stage_android_jnilib(crate_dir: &Path, lib: &Path) {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let abi = match arch.as_str() {
        "aarch64" => "arm64-v8a",
        "arm" => "armeabi-v7a",
        "x86_64" => "x86_64",
        "x86" => "x86",
        other => panic!("unsupported Android arch: {other}"),
    };

    let workspace = crate_dir.parent().expect("syncthing-sys has a parent dir");
    let gen_android = workspace.join("site/src-tauri/gen/android");
    if !gen_android.exists() {
        println!(
            "cargo:warning=syncthing-sys: {} absent; skipping jniLibs staging",
            gen_android.display()
        );
        return;
    }

    let jnilibs = gen_android.join("app/src/main/jniLibs").join(abi);
    std::fs::create_dir_all(&jnilibs)
        .unwrap_or_else(|e| panic!("creating {}: {e}", jnilibs.display()));
    let dest = jnilibs.join("libsyncthing_core.so");
    std::fs::copy(lib, &dest)
        .unwrap_or_else(|e| panic!("staging {} -> {}: {e}", lib.display(), dest.display()));
}

/// Runs `xcrun` and returns its trimmed stdout.
fn xcrun(args: &[&str]) -> String {
    let out = Command::new("xcrun")
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run xcrun {args:?}: {e}"));
    assert!(out.status.success(), "xcrun {args:?} failed");
    String::from_utf8(out.stdout)
        .expect("xcrun output is utf-8")
        .trim()
        .to_string()
}

/// System libraries the Go runtime + crypto/net stack require at link time.
fn link_platform_libs() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "macos" => {
            // Go's net/crypto on Darwin pull in these frameworks + resolv;
            // CoreServices provides FSEvents for the file watcher (macOS only).
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=CoreServices");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=resolv");
        }
        "ios" => {
            // iOS has no CoreServices/FSEvents (Syncthing's watcher falls back
            // to kqueue, which is in libSystem — no extra framework needed).
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=resolv");
        }
        "linux" => {
            println!("cargo:rustc-link-lib=pthread");
            println!("cargo:rustc-link-lib=dl");
        }
        "android" => {
            // Bionic folds pthread/dl into libc, but the Go runtime's cgo glue
            // logs through liblog, which the final (Rust) link must resolve.
            println!("cargo:rustc-link-lib=log");
        }
        _ => {}
    }
}
