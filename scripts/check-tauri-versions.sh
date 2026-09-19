#!/bin/sh
# Keep the Tauri Rust crates and their npm counterparts on the same major.minor.
#
# `cargo tauri build` aborts with
#   "Found version mismatched Tauri packages ... tauri (vX) : @tauri-apps/api (vY)"
# whenever the two drift, which turns a routine `pnpm install`/dependency bump
# into a build that only fails on a fresh checkout. This compares the committed
# lockfiles -- what a fresh system actually installs -- so the drift is caught at
# commit time instead of at build time.
#
# Fix a reported mismatch by pinning the npm side down to the crate's minor in
# site/package.json (tilde ranges, plus the pnpm override for @tauri-apps/api),
# never by silently bumping the crate.

set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
cargo_lock="$root/Cargo.lock"
pnpm_lock="$root/site/pnpm-lock.yaml"

for f in "$cargo_lock" "$pnpm_lock"; do
    if [ ! -f "$f" ]; then
        echo "check-tauri-versions: missing $f" >&2
        exit 1
    fi
done

minor() { printf '%s\n' "${1%.*}"; }

# crate name -> version, for `tauri` and every `tauri-plugin-*` in Cargo.lock.
crates="$(awk '
    /^name = /    { gsub(/"/, ""); name = $3 }
    /^version = / {
        gsub(/"/, "")
        if (name ~ /^tauri(-plugin-[a-z0-9-]+)?$/) { print name, $3 }
        name = ""
    }
' "$cargo_lock")"

# npm package -> every distinct version the lockfile resolves.
npm="$(awk -F "'" '
    {
        for (i = 2; i <= NF; i += 2) {
            if ($i ~ /^@tauri-apps\/[a-z0-9-]+@[0-9]/) {
                spec = $i
                version = spec
                sub(/^.*@/, "", version)
                sub(/@[^@]*$/, "", spec)
                print spec, version
            }
        }
    }
' "$pnpm_lock" | sort -u)"
packages="$(printf '%s\n' "$npm" | awk '!seen[$1]++ { print $1 }')"

npm_versions() {
    printf '%s\n' "$npm" | awk -v pkg="$1" '
        $1 == pkg { printf "%s%s", sep, $2; sep = " " }
    '
}

status=0
fail() {
    printf 'check-tauri-versions: %s\n' "$1" >&2
    status=1
}

for pkg in $packages; do
    versions="$(npm_versions "$pkg")"
    short="${pkg#@tauri-apps/}"
    if [ "$short" = "api" ]; then
        crate="tauri"
    else
        crate="tauri-$short"
    fi

    # More than one resolved copy is how @tauri-apps/api crept back to a newer
    # minor through a plugin's own dependency range.
    case "$versions" in
        *" "*)
            fail "$pkg resolves to multiple versions ($versions); pin it with a pnpm override in site/package.json"
            continue
            ;;
    esac

    npm_version="$versions"
    crate_version="$(printf '%s\n' "$crates" | awk -v crate="$crate" '
        $1 == crate { version = $2 }
        END { print version }
    ')"
    if [ -z "$crate_version" ]; then
        continue # npm-only tooling package, nothing to match against
    fi

    if [ "$(minor "$npm_version")" != "$(minor "$crate_version")" ]; then
        fail "$crate (v$crate_version) : $pkg (v$npm_version) -- major/minor must match"
    fi
done

# A stale node_modules breaks the build the same way even when the lockfiles
# agree, so verify what is actually installed when it is present.
modules="$root/site/node_modules"
if [ -d "$modules" ]; then
    for pkg in $packages; do
        manifest="$modules/$pkg/package.json"
        [ -f "$manifest" ] || continue
        installed="$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$manifest" | head -1)"
        expected="$(npm_versions "$pkg")"
        if [ "$installed" != "$expected" ]; then
            fail "$pkg installed as v$installed but the lockfile pins v$expected; run \`pnpm install\` in site/"
        fi
    done
fi

if [ "$status" -ne 0 ]; then
    echo "check-tauri-versions: Tauri crate/npm versions are out of sync." >&2
    exit 1
fi

echo "check-tauri-versions: Tauri crate and npm versions agree."
