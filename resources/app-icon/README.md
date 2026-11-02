# Application artwork

`source/NewFlectarIcon.png` is the exact image supplied by the user as
`C:\Users\tment\Downloads\NewFlectarIcon.png` for this fork on 2026-11-02.
Its SHA-256 is
`485ca8a7067b496d70e82c3df44b2fe8f59a6dfced8cf8e7ce3ede833df50216`.
It is a 1448 x 1086 RGBA PNG. This provenance records the supplied source;
it does not assign a license or replace the project's existing third-party
notices, licenses, or trademark documentation.

Run `python resources/app-icon/generate.py` with Pillow (verified with
12.3.0) to regenerate. Run
`python resources/app-icon/generate.py --check` for read-only verification
using only the Python standard library. For a full independent pixel check,
run `python -m unittest discover -s resources/app-icon -p "test_*.py"`.

All outputs contain the complete original canvas. Resizing uses Lanczos,
rounds dimensions to the nearest pixel, and centers the result on a square
transparent canvas. It does not crop, stretch, redraw, recolor, or remove
transparent margins from the source. Window and tray canvases are 256 and
64 pixels. The ICO contains 16, 20, 24, 32, 40, 48, 64, 96, 128, and 256 pixel
RGBA PNG frames generated independently from the source.

Platform requirements:

- Windows executable and installer share `flectar-mail.ico`.
- macOS packaging uses the 1024 pixel `flectar-mail-masked.png`.
- Linux packages retain the existing SVG and 512 pixel PNG paths. SVGs embed
  the exact source PNG with centered aspect-preserving containment.
- Android adaptive icons fit the full image inside the circular safe zone
  on a transparent canvas. All launcher aliases share it. No monochrome
  layer is supplied because it would recolor the design.
- iOS requires opaque RGB app icons. Only those outputs composite the
  contained image onto black; the source and all other outputs retain alpha.

Legacy color variant filenames and persisted enum values remain for
compatibility. Every variant now uses this same artwork. The application
color picker is disabled. `legacy-managed-sha256.json` records hashes of
the former upstream Linux variants from the pre-replacement checkout.
Runtime recognition of these exact hashes permits upgrading icons previously
written by the application without replacing a user's custom icon.

`generated.json` records source and generator hashes, every generated file
hash, and every embedded ICO frame hash. `--check` verifies the immutable
source, manifest freshness, all outputs, PNG format, and complete ICO
directory and frame payloads. The pixel tests independently reconstruct
the required aspect-preserving canvases from the original source.

Manifest hashing normalizes CRLF to LF for the generator and generated SVG/XML
text. Binary PNG/ICO hashes remain byte-exact. Both LF and CRLF checkout
representations are covered by the portability regression test; no additional
Git line-ending attributes are required.
