"""Focused checks for the supplied artwork and all packaged icon paths."""

import base64
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import subprocess
import sys
import unittest
from unittest import mock
import xml.etree.ElementTree as ET

from PIL import Image, ImageOps

ROOT = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("app_icon_generate", ROOT / "generate.py")
generate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(generate)


class ArtworkTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = Image.open(ROOT / "source/NewFlectarIcon.png").convert("RGBA")
        cls.recorded = json.loads((ROOT / "generated.json").read_text())

    def expected(self, size, ios=False, android=False):
        limit = round(size * 66 / 108 / (2 ** 0.5)) if android else size
        # Independent Pillow contain/pad operations, rather than the generator.
        inner = ImageOps.contain(self.source, (limit, limit), Image.Resampling.LANCZOS)
        expected = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        expected.paste(inner, ((size - inner.width) // 2, (size - inner.height) // 2))
        if ios:
            background = Image.new("RGBA", expected.size, (0, 0, 0, 255))
            return Image.alpha_composite(background, expected).convert("RGB")
        return expected

    def test_original_source_is_unmodified(self):
        self.assertEqual(self.source.size, (1448, 1086))
        self.assertEqual(hashlib.sha256((ROOT / "source/NewFlectarIcon.png").read_bytes()).hexdigest(),
                         "485ca8a7067b496d70e82c3df44b2fe8f59a6dfced8cf8e7ce3ede833df50216")
        self.assertEqual(self.source.getchannel("A").getextrema(), (0, 255))

    def test_every_raster_preserves_complete_source_and_proportions(self):
        paths = [generate.PROJECT / name for name in self.recorded["files"] if name.endswith(".png")]
        self.assertGreater(len(paths), 40)
        for path in paths:
            with self.subTest(path=path.relative_to(generate.PROJECT)):
                actual = Image.open(path)
                size = actual.width
                ios = "Assets.xcassets" in path.parts
                android = "drawable-nodpi" in path.parts
                expected = self.expected(size, ios=ios, android=android)
                self.assertEqual(actual.size, (size, size))
                self.assertEqual(actual.mode, "RGB" if ios else "RGBA")
                self.assertEqual(actual.tobytes(), expected.tobytes())
                if not ios:
                    self.assertEqual(actual.getpixel((0, 0))[3], 0)

    def test_every_ico_frame_matches_source_containment(self):
        data = (ROOT / "flectar-mail.ico").read_bytes()
        self.assertEqual(struct.unpack_from("<HHH", data), (0, 1, 10))
        for index, size in enumerate((16, 20, 24, 32, 40, 48, 64, 96, 128, 256)):
            with self.subTest(size=size):
                w, h, colors, reserved, planes, bits, length, offset = struct.unpack_from(
                    "<BBBBHHII", data, 6 + index * 16)
                self.assertEqual((w or 256, h or 256, colors, reserved, planes, bits),
                                 (size, size, 0, 0, 1, 32))
                image = Image.open(io.BytesIO(data[offset:offset + length]))
                self.assertEqual(image.tobytes(), self.expected(size).tobytes())
        with Image.open(ROOT / "flectar-mail.ico") as ico:
            self.assertEqual(ico.ico.sizes(), {(size, size) for size in generate.ICO_SIZES})

    def test_modified_or_truncated_ico_frames_are_rejected(self):
        data = (ROOT / "flectar-mail.ico").read_bytes()
        for broken in (data[:-1], data[:-1] + bytes([data[-1] ^ 1])):
            with self.assertRaises(ValueError):
                generate.verify_ico(broken, self.recorded["ico_frames"])

    def test_legacy_linux_migration_hashes_are_registered(self):
        recorded = json.loads((ROOT / "legacy-managed-sha256.json").read_text())
        runtime = (generate.PROJECT / "src/app_icon.rs").read_text()
        self.assertEqual(set(recorded), {"svg", "512.png"})
        for digests in recorded.values():
            self.assertEqual(len(digests), 5)
            for digest in digests:
                self.assertIn(f'"{digest}"', runtime)

    def test_svg_embeds_exact_original_with_containment(self):
        original = (ROOT / "source/NewFlectarIcon.png").read_bytes()
        for path in [ROOT / "flectar-mail-masked.svg", ROOT / "flectar-mail-nonmasked.svg",
                     *sorted((ROOT / "variants").glob("*.svg"))]:
            with self.subTest(path=path):
                image = ET.fromstring(path.read_bytes()).find("{http://www.w3.org/2000/svg}image")
                self.assertEqual(image.attrib["preserveAspectRatio"], "xMidYMid meet")
                self.assertEqual((image.attrib["width"], image.attrib["height"]), ("1024", "1024"))
                encoded = image.attrib["{http://www.w3.org/1999/xlink}href"].split(",", 1)[1]
                self.assertEqual(base64.b64decode(encoded), original)

    def test_legacy_variants_cannot_resurrect_old_artwork(self):
        for name in ("graphite", "light", "blue", "green", "purple"):
            for suffix, default in (("512", "masked-512"), ("window", "window"), ("tray", "tray")):
                self.assertEqual((ROOT / f"variants/{name}-{suffix}.png").read_bytes(),
                                 (ROOT / f"flectar-mail-{default}.png").read_bytes())
        for path in generate.ANDROID_RES.glob("mipmap-anydpi-v*/ic_launcher*.xml"):
            tree = ET.fromstring(path.read_bytes())
            self.assertIsNone(tree.find("monochrome"))
            self.assertEqual(tree.find("foreground").attrib[
                "{http://schemas.android.com/apk/res/android}drawable"], "@drawable/ic_launcher_foreground")

    def test_generation_is_reproducible(self):
        for path, expected in generate.expected_outputs().items():
            actual = path.read_bytes()
            if path.suffix in (".svg", ".xml"):
                actual = actual.replace(b"\r\n", b"\n")
            self.assertEqual(actual, expected, str(path.relative_to(generate.PROJECT)))

    def test_manifest_check_accepts_lf_and_crlf_text(self):
        original_read = Path.read_bytes
        for newline in (b"\n", b"\r\n"):
            with self.subTest(newline=newline):
                checked_suffixes = set()

                def read_with_newlines(path):
                    data = original_read(path)
                    if path.suffix in (".py", ".svg", ".xml"):
                        checked_suffixes.add(path.suffix)
                        return data.replace(b"\r\n", b"\n").replace(b"\n", newline)
                    return data

                with mock.patch.object(Path, "read_bytes", read_with_newlines):
                    with contextlib.redirect_stdout(io.StringIO()):
                        generate.check()
                self.assertEqual(checked_suffixes, {".py", ".svg", ".xml"})

    def test_ci_check_needs_no_site_packages(self):
        result = subprocess.run([sys.executable, "-S", str(ROOT / "generate.py"), "--check"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
