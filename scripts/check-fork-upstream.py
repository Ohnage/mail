#!/usr/bin/env python3
"""Refuse a fork build whose source does not contain current upstream/main.

This is a read-only check. It never merges, writes refs, or updates the app.
"""
import argparse
import subprocess
import sys

UPSTREAM = "https://github.com/flectar/mail.git"

def git(*args):
    return subprocess.run(["git", *args], text=True, capture_output=True, check=False)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream-sha", help="Verify a captured upstream commit without a network lookup")
    args = parser.parse_args()
    upstream = args.upstream_sha
    if upstream is None:
        lookup = git("ls-remote", "--exit-code", UPSTREAM, "refs/heads/main")
        if lookup.returncode:
            print("Could not verify upstream/main: " + lookup.stderr.strip(), file=sys.stderr)
            return 1
        upstream = lookup.stdout.split()[0]
    if len(upstream) != 40 or any(c not in "0123456789abcdefABCDEF" for c in upstream):
        parser.error("upstream commit must be a full hexadecimal SHA")
    included = git("merge-base", "--is-ancestor", upstream, "HEAD")
    if included.returncode:
        print(f"Upstream/main {upstream} is not included in this checkout. Fetch and integrate upstream, then verify again.", file=sys.stderr)
        return 1
    head = git("rev-parse", "HEAD")
    if head.returncode:
        print(head.stderr.strip(), file=sys.stderr)
        return 1
    print(f"Verified upstream/main {upstream} is included in fork {head.stdout.strip()}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
