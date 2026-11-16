#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Sets the app's version in the four files that carry it.

    python3 -I publik/set-version.py <from> <to>

Used by add-publik.sh (a publik-only version such as 1.0.0-alpha.7.2 on
fjenn/wispr-bro's 1.0.0-alpha.7) and by sync.sh (back to fjenn/wispr-bro's
version before merging a new release of it). Run from the repository root.
"""
import sys

FILES = [
    ("package.json", '"version": "{}"'),
    ("src-tauri/tauri.fork.conf.json", '"version": "{}"'),
    ("src-tauri/Cargo.toml", 'version = "{}"'),
    ("src-tauri/Cargo.lock", 'name = "handy"\nversion = "{}"'),
]


def main() -> None:
    old, new = sys.argv[1], sys.argv[2]
    for path, pattern in FILES:
        with open(path, encoding="utf-8") as f:
            text = f.read()
        before, after = pattern.format(old), pattern.format(new)
        if before not in text:
            sys.exit(f"set-version: {path} has no {before!r}")
        with open(path, "w", encoding="utf-8") as f:
            f.write(text.replace(before, after, 1))


if __name__ == "__main__":
    main()
