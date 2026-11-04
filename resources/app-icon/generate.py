#!/usr/bin/env python3
"""Generate application artwork from the unmodified user-supplied PNG.

Generation requires Pillow (tested with 12.3.0). --check uses only Python's
standard library so existing offline CI can verify all committed outputs,
the Microsoft Store logo sizes, and the Android and iOS alternate-icon
declarations. See README.md for provenance, sizing, platform exceptions,
and verification.
"""

import argparse
import base64
import hashlib
import io
import json
from pathlib import Path
import re
import struct
import sys
import zlib

ROOT = Path(__file__).resolve().parent
PROJECT = ROOT.parents[1]
SOURCE = ROOT / "source/NewFlectarIcon.png"
SOURCE_SHA256 = "485ca8a7067b496d70e82c3df44b2fe8f59a6dfced8cf8e7ce3ede833df50216"
SOURCE_SIZE = (1448, 1086)
VARIANTS = ("graphite", "light", "blue", "green", "purple")
ICO_SIZES = (16, 20, 24, 32, 40, 48, 64, 96, 128, 256)
MANIFEST = ROOT / "generated.json"
ANDROID_RES = PROJECT / "platform/android/gradle/app/src/main/res"
IOS_ASSETS = PROJECT / "platform/ios/Assets.xcassets"
IOS_PROJECT = PROJECT / "platform/ios/project.yml"
WINDOWS_ASSETS = PROJECT / "platform/windows/msix/Assets"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def source_bytes():
    data = SOURCE.read_bytes()
    if sha256(data) != SOURCE_SHA256:
        raise ValueError("Source artwork does not match the approved original SHA-256")
    if png_header(data) != (*SOURCE_SIZE, 8, 6):
        raise ValueError("Source artwork must remain the original 1448 x 1086 RGBA PNG")
    return data


def png_header(data):
    if data[:16] != b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR":
        raise ValueError("Invalid PNG header")
    return struct.unpack(">IIBB", data[16:26])


def chunk(kind, body):
    return (struct.pack(">I", len(body)) + kind + body
            + struct.pack(">I", zlib.crc32(kind + body)))


def png_bytes(image):
    """Canonical lossless PNG encoding: no metadata, filter 0, fixed zlib level."""
    channels = {"RGB": 3, "RGBA": 4}[image.mode]
    width, height = image.size
    pixels = image.tobytes()
    stride = width * channels
    raw = b"".join(b"\0" + pixels[y:y + stride] for y in range(0, len(pixels), stride))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8,
                                          6 if channels == 4 else 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def contain(source, size, opaque=False, safe_zone=False):
    """Center the entire source canvas; never trim, stretch, mask, or recolor.

    Android adaptive launchers mask the outer canvas. Fit inside their 66/108
    safe zone (including the corners), keeping the entire artwork visible.
    iOS alone requires opaque icons, so composite onto a black canvas there.
    """
    from PIL import Image
    limit = size * 66 / 108 / (2 ** 0.5) if safe_zone else size
    scale = limit / max(source.size)
    dimensions = tuple(max(1, round(value * scale)) for value in source.size)
    resized = source.resize(dimensions, Image.Resampling.LANCZOS)
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    canvas.paste(resized, ((size - dimensions[0]) // 2, (size - dimensions[1]) // 2))
    if opaque:
        background = Image.new("RGBA", canvas.size, (0, 0, 0, 255))
        return Image.alpha_composite(background, canvas).convert("RGB")
    return canvas


def svg_bytes(data):
    encoded = base64.b64encode(data).decode("ascii")
    return (f'<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" '
            f'width="1024" height="1024" viewBox="0 0 1024 1024">\n'
            f'  <image x="0" y="0" width="1024" height="1024" '
            f'preserveAspectRatio="xMidYMid meet" xlink:href="data:image/png;base64,{encoded}"/>\n'
            f'</svg>\n').encode("utf-8")


def ico_bytes(source):
    frames = [png_bytes(contain(source, size)) for size in ICO_SIZES]
    offset = 6 + 16 * len(frames)
    directory = bytearray(struct.pack("<HHH", 0, 1, len(frames)))
    for size, frame in zip(ICO_SIZES, frames):
        dimension = size if size < 256 else 0
        directory.extend(struct.pack("<BBBBHHII", dimension, dimension, 0, 0,
                                     1, 32, len(frame), offset))
        offset += len(frame)
    return bytes(directory) + b"".join(frames)


def expected_outputs():
    from PIL import Image
    data = source_bytes()
    source = Image.open(io.BytesIO(data)).convert("RGBA")
    svg = svg_bytes(data)
    outputs = {}
    for name in ("flectar-mail-masked.svg", "flectar-mail-nonmasked.svg"):
        outputs[ROOT / name] = svg
    for name, size in (("masked", 1024), ("nonmasked", 1024), ("masked-512", 512),
                       ("window", 256), ("tray", 64)):
        outputs[ROOT / f"flectar-mail-{name}.png"] = png_bytes(contain(source, size))
    outputs[ROOT / "flectar-mail.ico"] = ico_bytes(source)
    # Saved appIcon values keep working; every legacy choice is the same art.
    for name in VARIANTS:
        outputs[ROOT / f"variants/{name}.svg"] = svg
        for label, size in (("512", 512), ("window", 256), ("tray", 64)):
            outputs[ROOT / f"variants/{name}-{label}.png"] = png_bytes(contain(source, size))
    for contents in sorted(IOS_ASSETS.glob("AppIcon*.appiconset/Contents.json")):
        for item in json.loads(contents.read_text(encoding="utf-8"))["images"]:
            size = round(float(item["size"].split("x")[0]) * float(item.get("scale", "1x")[:-1]))
            outputs[contents.parent / item["filename"]] = png_bytes(contain(source, size, opaque=True))
    outputs[ANDROID_RES / "drawable-nodpi/ic_launcher_artwork.png"] = png_bytes(
        contain(source, 432, safe_zone=True))
    background = ('<?xml version="1.0" encoding="utf-8"?>\n'
                  '<shape xmlns:android="http://schemas.android.com/apk/res/android" android:shape="rectangle">\n'
                  '    <solid android:color="#00000000" />\n</shape>\n').encode()
    foreground = ('<?xml version="1.0" encoding="utf-8"?>\n'
                  '<bitmap xmlns:android="http://schemas.android.com/apk/res/android"\n'
                  '    android:src="@drawable/ic_launcher_artwork" android:gravity="fill" />\n').encode()
    for path in sorted((ANDROID_RES / "drawable").glob("ic_launcher_background*.xml")):
        outputs[path] = background
    for path in sorted((ANDROID_RES / "drawable").glob("ic_launcher_foreground*.xml")):
        outputs[path] = foreground
    # A themed monochrome mask would recolor this design. Do not offer one.
    for path in sorted(ANDROID_RES.glob("mipmap-anydpi-v*/ic_launcher*.xml")):
        outputs[path] = ('<?xml version="1.0" encoding="utf-8"?>\n'
                         '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n'
                         '    <background android:drawable="@drawable/ic_launcher_background" />\n'
                         '    <foreground android:drawable="@drawable/ic_launcher_foreground" />\n'
                         '</adaptive-icon>\n').encode()
    return outputs


def windows_logos():
    """Store package logo file names, with their pixel size and icon scale.

    These are the sizes Microsoft's app icon guidance lists. Windows uses an
    exact size when it has one and otherwise shrinks the next larger file, so
    every size the taskbar, Start and title bars use at each display scale is
    rendered from the SVG. The altform-unplated (dark theme) and
    altform-lightunplated files stop Windows from drawing a plate behind the
    icon. Start menu tiles keep the icon at half size.
    """
    logos = {}
    for scale in (100, 125, 150, 200, 400):
        for name, base, _icon in (("StoreLogo", 50, 1), ("Square44x44Logo", 44, 1), ("Square150x150Logo", 150, 0.5)):
            logos[f"{name}.scale-{scale}.png"] = (int(base * scale / 100 + 0.5), 1)
    for size in (16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256):
        for form in ("", "_altform-unplated", "_altform-lightunplated"):
            logos[f"Square44x44Logo.targetsize-{size}{form}.png"] = (size, 1)
    return logos


def check_store_logos():
    """The Store package keeps every logo size upstream renders from the master icon."""
    expected = windows_logos()
    actual = {path.name for path in WINDOWS_ASSETS.glob("*.png")}
    if actual != set(expected):
        raise ValueError("Microsoft Store logo inventory does not match the packaged sizes")
    for name, (size, _scale) in expected.items():
        width, height, depth, color = png_header((WINDOWS_ASSETS / name).read_bytes())
        if (width, height, depth, color) != (size, size, 8, 6):
            raise ValueError(f"Store logo {name} is {width}x{height}, expected {size}px RGBA")


def check_alternate_icons():
    """Launcher aliases and the iOS alternate-icon list stay in lockstep with the variants."""
    manifest = (ANDROID_RES.parent / "AndroidManifest.xml").read_text(encoding="utf-8")
    aliases = re.findall(r'<activity-alias\s+android:name="\.Icon(\w+)"', manifest)
    icons = ["Orange", *(name[:1].upper() + name[1:] for name in VARIANTS)]
    if aliases != icons:
        raise ValueError(f"AndroidManifest.xml declares launcher icons {aliases}, expected {icons}")
    project = IOS_PROJECT.read_text(encoding="utf-8")
    declared = re.search(r'ASSETCATALOG_COMPILER_ALTERNATE_APPICON_NAMES: "([^"]*)"', project)
    expected = " ".join(f"AppIcon{name[:1].upper() + name[1:]}" for name in VARIANTS)
    if not declared or declared.group(1) != expected:
        raise ValueError(f'project.yml must list the alternate icons as "{expected}"')


def verify_ico(data, expected_frames):
    reserved, kind, count = struct.unpack_from("<HHH", data)
    if (reserved, kind, count) != (0, 1, len(ICO_SIZES)):
        raise ValueError("Windows ICO header/frame count is invalid")
    expected_offset = 6 + 16 * count
    for index, size in enumerate(ICO_SIZES):
        w, h, colors, reserved, planes, bits, length, offset = struct.unpack_from(
            "<BBBBHHII", data, 6 + 16 * index)
        if (w or 256, h or 256, colors, reserved, planes, bits, offset) != (
                size, size, 0, 0, 1, 32, expected_offset):
            raise ValueError(f"Windows ICO {size}px frame directory is invalid")
        frame = data[offset:offset + length]
        if png_header(frame) != (size, size, 8, 6) or sha256(frame) != expected_frames[str(size)]:
            raise ValueError(f"Windows ICO {size}px frame is stale or invalid")
        expected_offset += length
    if expected_offset != len(data):
        raise ValueError("Windows ICO contains missing or untracked data")


def manifest(outputs):
    ico = outputs[ROOT / "flectar-mail.ico"]
    frames = {}
    for index, size in enumerate(ICO_SIZES):
        length, offset = struct.unpack_from("<II", ico, 6 + index * 16 + 8)
        frames[str(size)] = sha256(ico[offset:offset + length])
    return {"source_sha256": SOURCE_SHA256, "source_size": list(SOURCE_SIZE),
            "generator_sha256": sha256(Path(__file__).read_bytes().replace(b"\r\n", b"\n")),
            "ico_frames": frames,
            "files": {path.relative_to(PROJECT).as_posix(): sha256(data)
                      for path, data in sorted(outputs.items())}}


def check_declarations(recorded):
    """Changes to platform catalogs cannot silently escape the asset manifest."""
    import xml.etree.ElementTree as ET
    required = {ROOT / "flectar-mail.ico"}
    for suffix in ("masked.svg", "nonmasked.svg", "masked.png", "nonmasked.png",
                   "masked-512.png", "window.png", "tray.png"):
        required.add(ROOT / f"flectar-mail-{suffix}")
    variants = {ROOT / f"variants/{name}{suffix}" for name in VARIANTS
                for suffix in (".svg", "-512.png", "-window.png", "-tray.png")}
    if set((ROOT / "variants").glob("*")) != variants:
        raise ValueError("Unexpected or missing legacy compatibility icon files")
    required.update(variants)
    for catalog in IOS_ASSETS.glob("AppIcon*.appiconset/Contents.json"):
        required.update(catalog.parent / item["filename"]
                        for item in json.loads(catalog.read_text(encoding="utf-8"))["images"])
    manifest = ET.parse(ANDROID_RES.parent / "AndroidManifest.xml")
    android = "{http://schemas.android.com/apk/res/android}"
    for node in manifest.iter():
        for attribute in ("icon", "roundIcon"):
            resource = node.get(android + attribute, "")
            if resource.startswith("@mipmap/"):
                for version in ("v26", "v33"):
                    required.add(ANDROID_RES / f"mipmap-anydpi-{version}" / (resource[8:] + ".xml"))
    required.add(ANDROID_RES / "drawable-nodpi/ic_launcher_artwork.png")
    required.update((ANDROID_RES / "drawable").glob("ic_launcher_*.xml"))
    actual = {PROJECT / name for name in recorded["files"]}
    if required != actual:
        raise ValueError("Platform declarations and generated asset inventory differ; regenerate icons")


def check():
    source_bytes()
    recorded = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if (recorded["source_sha256"] != SOURCE_SHA256 or recorded["source_size"] != list(SOURCE_SIZE)
            or recorded["generator_sha256"] != sha256(Path(__file__).read_bytes().replace(b"\r\n", b"\n"))):
        raise ValueError("Generation manifest is stale; run resources/app-icon/generate.py")
    check_declarations(recorded)
    for name, digest in recorded["files"].items():
        path = PROJECT / name
        data = path.read_bytes()
        if path.suffix in (".svg", ".xml"):
            data = data.replace(b"\r\n", b"\n")
        if sha256(data) != digest:
            raise ValueError(f"Stale generated icon: {name}")
        if path.suffix == ".png":
            w, h, depth, color = png_header(data)
            if w != h or depth != 8 or color != (2 if "Assets.xcassets" in name else 6):
                raise ValueError(f"Invalid generated PNG format: {name}")
    verify_ico((ROOT / "flectar-mail.ico").read_bytes(), recorded["ico_frames"])
    check_store_logos()
    check_alternate_icons()
    print(f"Checked original PNG, {len(recorded['files'])} generated files, all {len(ICO_SIZES)} ICO frames, and the Store logos")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Read-only stdlib freshness and frame validation")
    args = parser.parse_args()
    try:
        if args.check:
            check()
        else:
            outputs = expected_outputs()
            for path, data in outputs.items():
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            MANIFEST.write_text(json.dumps(manifest(outputs), indent=2) + "\n", encoding="utf-8")
            check()
        return 0
    except (OSError, ValueError, KeyError, ImportError, struct.error) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
