#!/usr/bin/env python3
"""Make the Windows app's Vietnamese text from the Mac app's.

The Windows app is written in English, like the Mac app: `T("Camera")` in the
C# code. This tool gathers every such text, finds its Vietnamese in

  1. apps/windows/app/Texts/windows.json   texts only the Windows app has,
  2. apps/apple/GodsCamera/Localizable.xcstrings   the Mac app's texts,

and writes apps/windows/app/Assets/vi.json with exactly the texts the Windows
app uses. It fails, and names them, if any text has no Vietnamese.

    python tools/sync-windows-strings.py [--check]

--check writes nothing and only reports.

Places for a number or a name are written {0}, {1} in the C# code. The Mac's
file writes them %lld, %@ and so on; they are matched up here.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
APP = REPO / "apps" / "windows" / "app"
MAC = REPO / "apps" / "apple" / "GodsCamera" / "Localizable.xcstrings"
EXTRA = APP / "Texts" / "windows.json"
OUT = APP / "Assets" / "vi.json"

# A printf place such as %lld, %@, %.1f or %2$@.
PLACE = re.compile(r"%(?:(\d+)\$)?(?:\.\d+)?(?:lld|ld|d|@|f|s)")
# T("...") in C#, with the usual escapes inside the quotes.
CALL = re.compile(r'\bT\(\s*"((?:[^"\\]|\\.)*)"')


def braces(text: str) -> str:
    """The Mac's places, %lld and so on, as {0}, {1} in the order they come."""
    count = 0

    def place(match: re.Match) -> str:
        nonlocal count
        index = int(match.group(1)) - 1 if match.group(1) else count
        count += 1
        return "{" + str(index) + "}"

    return PLACE.sub(place, text)


def unescape(literal: str) -> str:
    """A C# string literal's contents as the text it stands for."""
    return json.loads('"' + literal.replace("\\'", "'") + '"')


def texts_in_code() -> dict[str, list[str]]:
    used: dict[str, list[str]] = {}
    for source in sorted(APP.rglob("*.cs")):
        if {"bin", "obj", "Core"} & set(source.relative_to(APP).parts):
            continue
        for match in CALL.finditer(source.read_text(encoding="utf-8")):
            used.setdefault(unescape(match.group(1)), []).append(source.name)
    return used


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="write nothing; only report")
    args = parser.parse_args()

    vietnamese: dict[str, str] = {}
    mac = json.loads(MAC.read_text(encoding="utf-8"))["strings"]
    for english, entry in mac.items():
        unit = entry.get("localizations", {}).get("vi", {}).get("stringUnit", {})
        if unit.get("value"):
            vietnamese[braces(english)] = braces(unit["value"])
    if EXTRA.is_file():
        # The Windows app's own texts win over the Mac's where both have one.
        vietnamese.update(json.loads(EXTRA.read_text(encoding="utf-8")))

    used = texts_in_code()
    missing = sorted(text for text in used if text not in vietnamese)
    for text in missing:
        print(f"no Vietnamese for {text!r}  ({', '.join(sorted(set(used[text])))})")
    for text in sorted(used):
        have = sorted(set(re.findall(r"\{\d+\}", vietnamese.get(text, text))))
        want = sorted(set(re.findall(r"\{\d+\}", text)))
        if text in vietnamese and have != want:
            print(f"the places differ in {text!r}: {vietnamese[text]!r}")
            missing.append(text)
    if missing:
        print(f"{len(missing)} text(s) need attention. Add them to {EXTRA.relative_to(REPO)}.")
        return 1
    print(f"{len(used)} texts, all with Vietnamese")
    if not args.check:
        OUT.parent.mkdir(parents=True, exist_ok=True)
        chosen = {text: vietnamese[text] for text in sorted(used)}
        OUT.write_text(json.dumps(chosen, ensure_ascii=False, indent=1) + "\n", encoding="utf-8", newline="\n")
        print(f"wrote {OUT.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
