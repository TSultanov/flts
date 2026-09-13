#!/usr/bin/env bash
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

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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
declare -A crates=()
while read -r name version; do
    crates["$name"]="$version"
done < <(awk '
    /^name = /    { gsub(/"/, ""); name = $3 }
    /^version = / { gsub(/"/, ""); if (name != "") { print name, $3; name = "" } }
' "$cargo_lock" | grep -E '^tauri(-plugin-[a-z0-9-]+)? ')

# npm package -> every version the lockfile resolves (duplicates included).
declare -A npm=()
while read -r spec; do
    pkg="${spec%@*}"
    version="${spec##*@}"
    npm["$pkg"]="${npm[$pkg]:-} $version"
done < <(grep -oE "'@tauri-apps/[a-z0-9-]+@[0-9][^']*'" "$pnpm_lock" |
    tr -d "'" | sort -u)

status=0
fail() {
    printf 'check-tauri-versions: %s\n' "$1" >&2
    status=1
}

for pkg in $(printf '%s\n' "${!npm[@]}" | sort); do
    versions=(${npm[$pkg]})
    short="${pkg#@tauri-apps/}"
    if [ "$short" = "api" ]; then
        crate="tauri"
    else
        crate="tauri-$short"
    fi

    # More than one resolved copy is how @tauri-apps/api crept back to a newer
    # minor through a plugin's own dependency range.
    mapfile -t distinct < <(printf '%s\n' "${versions[@]}" | sort -u)
    if [ "${#distinct[@]}" -gt 1 ]; then
        fail "$pkg resolves to multiple versions (${distinct[*]}); pin it with a pnpm override in site/package.json"
        continue
    fi

    npm_version="${distinct[0]}"
    crate_version="${crates[$crate]:-}"
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
    for pkg in $(printf '%s\n' "${!npm[@]}" | sort); do
        manifest="$modules/$pkg/package.json"
        [ -f "$manifest" ] || continue
        installed="$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' "$manifest" | head -1)"
        expected="${npm[$pkg]## }"
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
