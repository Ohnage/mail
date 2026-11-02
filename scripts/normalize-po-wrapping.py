#!/usr/bin/env python3
"""Fold PO quoted continuations without changing comments, flags, or order."""
import json
import re
import sys

DIRECTIVE = re.compile(r"^(msgctxt|msgid_plural|msgid|msgstr(?:\[[0-9]+\])?)[ \t]+(.*)$")


def quoted(value, line_number):
    # JSON accepts \/ and \u escapes that GNU PO rejects. Consume escaped
    # backslashes as pairs so a literal backslash followed by u stays valid.
    for escape in re.finditer(r"\\(.)", value):
        if escape.group(1) not in {'"', "\\", "b", "f", "n", "r", "t"}:
            raise ValueError(f"line {line_number}: invalid or unsupported PO escape")
    try:
        decoded = json.loads(value)
    except (ValueError, TypeError) as error:
        raise ValueError(f"line {line_number}: invalid or unsupported PO string: {error}") from error
    if not isinstance(decoded, str):
        raise ValueError(f"line {line_number}: expected a quoted PO string")
    return decoded


def normalize(data):
    # Split on LF only: Unicode line/paragraph separators may occur in strings.
    lines = data.decode("utf-8").replace("\r\n", "\n").split("\n")
    output = []
    index = 0
    while index < len(lines):
        line = lines[index]
        directive = DIRECTIVE.fullmatch(line)
        if directive:
            name, value = directive.groups()
            text = quoted(value, index + 1)
            index += 1
            while index < len(lines) and lines[index].lstrip(" \t").startswith('"'):
                text += quoted(lines[index].lstrip(" \t"), index + 1)
                index += 1
            output.append(name + " " + json.dumps(text, ensure_ascii=False))
            continue
        if line.lstrip(" \t").startswith('"'):
            raise ValueError(f"line {index + 1}: quoted continuation without a directive")
        output.append(line)
        index += 1
    return "\n".join(output).encode("utf-8")


def main():
    try:
        normalized = normalize(sys.stdin.buffer.read())
    except (UnicodeError, ValueError) as error:
        print(f"normalize-po-wrapping: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(normalized)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
