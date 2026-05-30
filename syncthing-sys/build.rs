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
    let archive = out_dir.join("libsyncthing_core.a");

    // Re-run if any Go source or the module manifest changes.
    println!("cargo:rerun-if-changed={}", go_dir.join("go.mod").display());
    for entry in std::fs::read_dir(&go_dir).expect("read syncthing-core dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "go") {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    build_archive(&go_dir, &archive);

    // Link the static archive (`libsyncthing_core.a` -> `-lsyncthing_core`).
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=syncthing_core");

    link_platform_libs();
}

fn build_archive(go_dir: &Path, archive: &Path) {
    let go = env::var("FLTS_GO_BIN").unwrap_or_else(|_| "go".to_string());
    let mut cmd = Command::new(&go);
    cmd.current_dir(go_dir)
        .args([
            "build",
            // `noassets`: skip the generated Web-GUI asset blob. FLTS drives the
            // engine over REST, never serves its GUI, so the minimal fallback in
            // syncthing's `lib/api/auto/noassets.go` is sufficient — and the
            // real assets aren't present in the module cache anyway.
            "-tags",
            "noassets",
            "-buildmode=c-archive",
            "-o",
            archive.to_str().expect("archive path is utf-8"),
            ".",
        ])
        .env("CGO_ENABLED", "1");

    // Cross-compile for iOS when Cargo is targeting it; otherwise the host go
    // toolchain builds for the host. Tauri builds the app for aarch64-apple-ios
    // (device) and the *-ios-sim / x86_64-apple-ios simulator triples.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "ios" {
        apply_ios_cross_env(&mut cmd);
    }

    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn `{go}`: {e}. Is the Go toolchain installed?"));
    assert!(
        status.success(),
        "`go build -buildmode=c-archive` failed (status {status})"
    );
    assert!(archive.exists(), "go build did not produce {}", archive.display());
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
        _ => {}
    }
}
