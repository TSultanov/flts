//! Builds the `syncthing-core` Go module into a c-archive (c-shared on Android)
//! and emits the link directives for it.

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

    // Android's cgo only supports `-buildmode=c-shared`.
    let android = target_os == "android";
    let lib = out_dir.join(if android {
        "libsyncthing_core.so"
    } else {
        "libsyncthing_core.a"
    });

    println!("cargo:rerun-if-changed={}", go_dir.join("go.mod").display());
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
        println!("cargo:rustc-link-lib=dylib=syncthing_core");
        stage_android_jnilib(&crate_dir, &lib);
    } else {
        println!("cargo:rustc-link-lib=static=syncthing_core");
    }

    link_platform_libs();
}

fn build_archive(go_dir: &Path, lib: &Path, target_os: &str) {
    let android = target_os == "android";
    let go = env::var("FLTS_GO_BIN").unwrap_or_else(|_| "go".to_string());

    // Debug builds embed Syncthing's Web GUI for diagnostics; release skips it.
    let debug = env::var("PROFILE").as_deref() == Ok("debug");
    let assets_modfile = debug.then(|| embed_web_ui_modfile(&go, go_dir));

    let mut cmd = Command::new(&go);
    cmd.current_dir(go_dir).arg("build");
    match &assets_modfile {
        Some(modfile) => {
            cmd.arg("-modfile").arg(modfile);
        }
        // FLTS drives the engine over REST; the `noassets` fallback suffices.
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

    // Bare SONAME so the DT_NEEDED is resolved from the app's lib dir, not OUT_DIR.
    if android {
        cmd.arg("-ldflags=-extldflags=-Wl,-soname,libsyncthing_core.so");
    }
    cmd.arg(".");

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

/// Returns an alternate `go.mod` whose `replace` points at a writable copy of
/// the Syncthing module carrying the generated `lib/api/auto/gui.files.go`.
///
/// `-overlay` can't work here: the generated file is absent from the module
/// cache and Go forbids overlaying anything beneath GOMODCACHE. Host toolchain
/// only — `genassets.go` is a standalone `//go:build ignore` walker.
fn embed_web_ui_modfile(go: &str, go_dir: &Path) -> PathBuf {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

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

    // The cache tree is read-only, hence the rm/cp/chmod dance.
    let st_copy = out_dir.join("syncthing-src");
    run(Command::new("rm").arg("-rf").arg(&st_copy));
    std::fs::create_dir_all(&st_copy)
        .unwrap_or_else(|e| panic!("creating {}: {e}", st_copy.display()));
    run(Command::new("cp").arg("-R").arg(format!("{}/.", st_src.display())).arg(&st_copy));
    run(Command::new("chmod").arg("-R").arg("u+w").arg(&st_copy));

    // Go's zip packaging strips nested `vendor/` dirs, so supply our own copy.
    let vendor_src = go_dir.join("webui-vendor");
    let vendor_dst = st_copy.join("gui/default/vendor");
    std::fs::create_dir_all(&vendor_dst)
        .unwrap_or_else(|e| panic!("creating {}: {e}", vendor_dst.display()));
    run(Command::new("cp").arg("-R").arg(format!("{}/.", vendor_src.display())).arg(&vendor_dst));

    run(Command::new(go)
        .current_dir(go_dir)
        .arg("run")
        .arg(st_copy.join("script/genassets.go"))
        .arg("-o")
        .arg(st_copy.join("lib/api/auto/gui.files.go"))
        .arg(st_copy.join("gui")));

    // `-modfile` derives its sum file by swapping the extension, hence go.webui.sum.
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

/// Points cgo's clang at the SDK matching the active iOS target.
fn apply_ios_cross_env(cmd: &mut Command) {
    let target = env::var("TARGET").unwrap_or_default();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // Simulator triples: `*-apple-ios-sim` and `x86_64-apple-ios`.
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

/// Points cgo's CC at the NDK clang wrapper matching the active Android target.
fn apply_android_cross_env(cmd: &mut Command) {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // Only 32-bit ARM needs GOARM.
    let (goarch, clang_triple, goarm): (&str, &str, Option<&str>) = match arch.as_str() {
        "aarch64" => ("arm64", "aarch64-linux-android", None),
        "arm" => ("arm", "armv7a-linux-androideabi", Some("7")),
        "x86_64" => ("amd64", "x86_64-linux-android", None),
        "x86" => ("386", "i686-linux-android", None),
        other => panic!("unsupported Android arch: {other}"),
    };

    // Must be >= the app's minSdk; 24 matches the linker cargo-tauri uses.
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

/// Locates the NDK's prebuilt LLVM `bin` dir holding the clang wrappers.
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

/// Copies `libsyncthing_core.so` into the Tauri Android project's
/// `jniLibs/<abi>/` so Gradle packages it beside `libapp_lib.so`. Skipped when
/// that tree is absent; linking still resolves against the OUT_DIR copy.
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
            // Go's net/crypto pull in CoreFoundation/Security/resolv;
            // CoreServices provides FSEvents for the file watcher.
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=CoreServices");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=resolv");
        }
        "ios" => {
            // No CoreServices; the watcher falls back to kqueue in libSystem.
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=resolv");
        }
        "linux" => {
            println!("cargo:rustc-link-lib=pthread");
            println!("cargo:rustc-link-lib=dl");
        }
        "android" => {
            // Bionic folds pthread/dl into libc; cgo glue still needs liblog.
            println!("cargo:rustc-link-lib=log");
        }
        _ => {}
    }
}
