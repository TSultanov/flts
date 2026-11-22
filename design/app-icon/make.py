#!/usr/bin/env python3
"""Draws the app icon and renders every platform's icon set from it.

Needs rsvg-convert (brew install librsvg). Run from anywhere:
    python3 design/app-icon/make.py
"""

import json
import pathlib
import shutil
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[2]
HERE = ROOT / "design/app-icon"

PAPER = "#F5F7FB"
INK = "#14213D"
WORD = "#8AB4FF"

DEFS = """
<linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1">
  <stop offset="0" stop-color="#fff" stop-opacity=".35"/>
  <stop offset=".5" stop-color="#fff" stop-opacity="0"/>
  <stop offset="1" stop-color="#000" stop-opacity=".14"/>
</linearGradient>
<filter id="lift" x="-30%" y="-30%" width="160%" height="180%">
  <feGaussianBlur in="SourceAlpha" stdDeviation="3"/>
  <feOffset dy="3"/>
  <feComponentTransfer><feFuncA type="linear" slope=".28"/></feComponentTransfer>
  <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
<filter id="tile" x="-20%" y="-20%" width="140%" height="150%">
  <feGaussianBlur in="SourceAlpha" stdDeviation="1.4"/>
  <feOffset dy="1.2"/>
  <feComponentTransfer><feFuncA type="linear" slope=".3"/></feComponentTransfer>
  <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
"""

# A paragraph whose highlighted word shows its translation in a popover above it,
# drawn on a 100-unit square.
ART = f"""
<g transform="translate(0 -2)">
  <rect x="16" y="20" width="22" height="9" rx="4.5" fill="{INK}" opacity=".25"/>
  <rect x="16" y="35" width="36" height="9" rx="4.5" fill="{INK}" opacity=".25"/>
  <path d="M52 20H78Q84 20 84 26V36Q84 42 78 42H70L65 48L60 42H52Q46 42 46 36V26Q46 20 52 20Z" fill="{INK}" filter="url(#lift)"/>
  <rect x="53" y="28.75" width="24" height="4.5" rx="2.25" fill="{PAPER}"/>
  <rect x="16" y="57" width="24" height="10" rx="5" fill="{INK}" opacity=".8"/>
  <rect x="46" y="52" width="38" height="20" rx="10" fill="{WORD}" filter="url(#lift)"/>
  <rect x="46" y="52" width="38" height="20" rx="10" fill="none" stroke="{INK}" stroke-opacity=".2" stroke-width="1"/>
  <rect x="48" y="53.5" width="34" height="7" rx="3.5" fill="#fff" opacity=".35"/>
  <rect x="55" y="59.75" width="20" height="4.5" rx="2.25" fill="{INK}"/>
  <rect x="16" y="78" width="50" height="9" rx="4.5" fill="{INK}" opacity=".25"/>
</g>
"""


def svg(body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><defs>{DEFS}</defs>{body}</svg>\n'


def paper(x, y, size, rx):
    return (
        f'<rect x="{x}" y="{y}" width="{size}" height="{size}" rx="{rx}" fill="{PAPER}"/>'
        f'<rect x="{x}" y="{y}" width="{size}" height="{size}" rx="{rx}" fill="url(#sheen)"/>'
    )


def scaled(art, scale):
    return (
        f'<g transform="translate(50 50) scale({scale}) translate(-50 -50)">{art}</g>'
    )


# iOS masks the corners itself, so its icon is a full-bleed opaque square.
SQUARE = svg(paper(0, 0, 100, 0) + ART)

# macOS draws the icon as is: Apple's grid puts an 824/1024 tile, corner 185, on a
# transparent canvas, with a drop shadow below.
T, S = 100 / 1024 * 100, 824 / 1024 * 100
MACOS = svg(
    f'<g filter="url(#tile)">{paper(T, T, S, 185 / 1024 * 100)}</g>'
    f'<rect x="{T}" y="{T}" width="{S}" height="{S}" rx="{185 / 1024 * 100}" fill="none" '
    f'stroke="#000" stroke-opacity=".08" stroke-width=".4"/>' + scaled(ART, S / 100)
)

# Android's adaptive foreground is 108dp, of which launchers may mask all but the
# central 66dp circle.
ANDROID_FG = svg(scaled(ART, 0.62))
ANDROID_LEGACY = svg(scaled(paper(4, 4, 92, 18) + scaled(ART, 0.92), 1))
ANDROID_ROUND = svg(
    f'<circle cx="50" cy="50" r="46" fill="{PAPER}"/><circle cx="50" cy="50" r="46" fill="url(#sheen)"/>'
    + scaled(ART, 0.8)
)


def write(name, text):
    path = HERE / name
    path.write_text(text)
    return path


def render(src, out, px):
    out.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["rsvg-convert", "-w", str(px), "-h", str(px), "-o", str(out), str(src)],
        check=True,
    )


def flatten(path):
    # The App Store rejects iOS icons with an alpha channel.
    subprocess.run(
        ["sips", "-s", "format", "jpeg", str(path), "--out", str(path) + ".jpg"],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["sips", "-s", "format", "png", str(path) + ".jpg", "--out", str(path)],
        check=True,
        capture_output=True,
    )
    pathlib.Path(str(path) + ".jpg").unlink()


def appiconset(directory, square_src):
    contents = json.loads((directory / "Contents.json").read_text())
    for image in contents["images"]:
        if "filename" not in image:
            continue
        points = float(image["size"].split("x")[0])
        px = round(points * int(image["scale"][0]))
        out = directory / image["filename"]
        render(square_src, out, px)
        if image["idiom"] != "mac":
            flatten(out)


ANDROID_DENSITIES = {"mdpi": 1, "hdpi": 1.5, "xhdpi": 2, "xxhdpi": 3, "xxxhdpi": 4}
ADAPTIVE = f"""<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
</adaptive-icon>
"""
BACKGROUND = f"""<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="ic_launcher_background">{PAPER}</color>
</resources>
"""


def android(res, fg, legacy, rnd):
    for density, scale in ANDROID_DENSITIES.items():
        d = res / f"mipmap-{density}"
        render(fg, d / "ic_launcher_foreground.png", round(108 * scale))
        render(legacy, d / "ic_launcher.png", round(48 * scale))
        render(rnd, d / "ic_launcher_round.png", round(48 * scale))
    anydpi = res / "mipmap-anydpi-v26"
    anydpi.mkdir(exist_ok=True)
    (anydpi / "ic_launcher.xml").write_text(ADAPTIVE)
    (anydpi / "ic_launcher_round.xml").write_text(ADAPTIVE)
    (res / "values/ic_launcher_background.xml").write_text(BACKGROUND)


def main():
    square = write("icon-ios.svg", SQUARE)
    macos = write("icon-macos.svg", MACOS)
    fg = write("android-foreground.svg", ANDROID_FG)
    legacy = write("android-legacy.svg", ANDROID_LEGACY)
    rnd = write("android-round.svg", ANDROID_ROUND)

    appiconset(ROOT / "native/Resources/Assets.xcassets/AppIcon.appiconset", macos)
    appiconset(ROOT / "native/iOS/Assets.xcassets/AppIcon.appiconset", square)
    android(ROOT / "android/app/src/main/res", fg, legacy, rnd)

    # Tauri derives its desktop icons from one source; its iOS and Android sets
    # then get the same treatment as the native apps'.
    tauri = ROOT / "site/src-tauri"
    master = HERE / "build/icon-macos-1024.png"
    render(macos, master, 1024)
    subprocess.run(["cargo", "tauri", "icon", str(master)], cwd=tauri, check=True)
    appiconset(tauri / "gen/apple/Assets.xcassets/AppIcon.appiconset", square)
    android(tauri / "gen/android/app/src/main/res", fg, legacy, rnd)
    shutil.rmtree(HERE / "build")


if __name__ == "__main__":
    main()
