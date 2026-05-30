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
    let status = Command::new(&go)
        .current_dir(go_dir)
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
        // Keep cgo on (default) and let Go pick the host toolchain.
        .env("CGO_ENABLED", "1")
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn `{go}`: {e}. Is the Go toolchain installed?"));
    assert!(
        status.success(),
        "`go build -buildmode=c-archive` failed (status {status})"
    );
    assert!(archive.exists(), "go build did not produce {}", archive.display());
}

/// System libraries the Go runtime + crypto/net stack require at link time.
fn link_platform_libs() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "macos" | "ios" => {
            // Go's net/crypto on Darwin pull in these frameworks + resolv;
            // CoreServices is needed for the FSEvents-based file watcher.
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=CoreServices");
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
