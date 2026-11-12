#!/usr/bin/env python3
"""Fill the model store on this machine from the manifests in packs/.

The model files are not in git. Each machine keeps them in a folder beside the
repository, ../packs, and this script brings that folder up to date:

- it downloads what is missing from the address in each manifest,
- it checks every file against the size and SHA-256 in its manifest,
- it makes the files that are derived from a downloaded one.

A file that is already there and matches is left alone, so the script can be
run again at any time. A download that was cut off carries on where it stopped.

Usage:
    python tools/fetch-packs.py                  everything this machine needs
    python tools/fetch-packs.py --only xseg_3    one pack
    python tools/fetch-packs.py --check          download nothing, only report

It needs Python 3.9 or later and nothing else for downloading. The derived
files are made with the scripts in tools/modelkit, run through uv.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
CHUNK = 1 << 20

# How each derived file is made: the modelkit script and the packages it needs.
# The half-precision model is for the Mac's Neural Engine only.
DERIVED = {
    "inswapper_128.emap.bin": {"script": "extract_emap.py", "with": ["onnx", "numpy<3"], "platforms": None},
    "inswapper_128.reflectfree.fp16.onnx": {"script": None, "with": [], "platforms": ["darwin"]},
}


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(CHUNK):
            digest.update(chunk)
    return digest.hexdigest()


def matches(path: Path, entry: dict) -> bool:
    return path.is_file() and path.stat().st_size == entry["bytes"] and sha256_of(path) == entry["sha256"]


def download(url: str, target: Path) -> None:
    """Download `url` to `target`, carrying on from a partial file if there is one."""
    target.parent.mkdir(parents=True, exist_ok=True)
    part = target.with_name(target.name + ".part")
    have = part.stat().st_size if part.exists() else 0
    request = urllib.request.Request(url, headers={"User-Agent": "fetch-packs"})
    if have:
        request.add_header("Range", f"bytes={have}-")
    try:
        response = urllib.request.urlopen(request, timeout=60)
    except urllib.error.HTTPError as error:
        if error.code == 416 and have:  # the partial file is already the whole file
            part.replace(target)
            return
        raise
    with response:
        resumed = response.status == 206
        if not resumed:
            have = 0
        total = have + int(response.headers.get("Content-Length") or 0)
        shown = -1
        with part.open("ab" if resumed else "wb") as out:
            while chunk := response.read(CHUNK):
                out.write(chunk)
                have += len(chunk)
                tenth = have * 10 // total if total else 0
                if tenth != shown:
                    shown = tenth
                    print(f"    {have / 1e6:8.0f} MB of {total / 1e6:.0f}", flush=True)
    part.replace(target)


def from_archive(archive: Path, member_name: str, target: Path) -> None:
    """Copy the archive member whose file name is `member_name` to `target`."""
    with zipfile.ZipFile(archive) as bundle:
        names = [name for name in bundle.namelist() if Path(name).name == member_name and not name.endswith("/")]
        if len(names) != 1:
            raise SystemExit(f"{archive.name}: expected one {member_name}, found {len(names)}")
        target.parent.mkdir(parents=True, exist_ok=True)
        with bundle.open(names[0]) as source, target.open("wb") as out:
            shutil.copyfileobj(source, out, CHUNK)


def derive(name: str, store: Path) -> str | None:
    """Make a derived file. Returns a reason if it was not made."""
    recipe = DERIVED.get(name)
    if recipe is None:
        return "no recipe in this script"
    if recipe["platforms"] is not None and sys.platform not in recipe["platforms"]:
        return "not needed on this system"
    if recipe["script"] is None:
        return "made by hand on the Mac; see its manifest entry"
    if shutil.which("uv") is None:
        return "uv is not installed"
    command = ["uv", "run", "--no-project"]
    for package in recipe["with"]:
        command += ["--with", package]
    command += ["python", str(REPO / "tools" / "modelkit" / recipe["script"]), "--packs", str(store)]
    done = subprocess.run(command, cwd=store)
    return None if done.returncode == 0 else f"{recipe['script']} failed"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--store", type=Path, default=Path(os.environ.get("GC_PACK_STORE") or REPO.parent / "packs"))
    parser.add_argument("--only", nargs="*", default=None, help="pack names; the default is every pack")
    parser.add_argument("--check", action="store_true", help="report only; change nothing")
    args = parser.parse_args()
    store: Path = args.store.resolve()
    print(f"model store: {store}")

    problems = 0
    waiting = []  # derived files, made after everything they come from is there
    for manifest in sorted((REPO / "packs").glob("*.json")):
        pack = json.loads(manifest.read_text(encoding="utf-8"))
        if args.only is not None and pack["name"] not in args.only:
            continue
        for entry in pack["files"]:
            target = store / entry["path"]
            if matches(target, entry):
                print(f"  ok       {entry['path']}")
                continue
            url = entry["source_url"]
            if not url.startswith("https://"):
                waiting.append(entry)
                continue
            if args.check:
                print(f"  MISSING  {entry['path']}")
                problems += 1
                continue
            print(f"  fetching {entry['path']}  ({entry['bytes'] / 1e6:.0f} MB)", flush=True)
            if url.endswith(".zip"):
                archive = store / url.rsplit("/", 1)[1]
                try:
                    from_archive(archive, target.name, target)
                except (FileNotFoundError, zipfile.BadZipFile, SystemExit):
                    download(url, archive)
                    from_archive(archive, target.name, target)
            else:
                download(url, target)
            if matches(target, entry):
                print(f"  ok       {entry['path']}")
            else:
                print(f"  WRONG    {entry['path']} does not match its manifest; it was removed")
                target.unlink(missing_ok=True)
                problems += 1

    for entry in waiting:
        target = store / entry["path"]
        if args.check:
            print(f"  MISSING  {entry['path']} (derived)")
            problems += 1
            continue
        print(f"  making   {entry['path']}", flush=True)
        reason = derive(target.name, store)
        if reason is not None:
            print(f"  skipped  {entry['path']}: {reason}")
        elif matches(target, entry):
            print(f"  ok       {entry['path']}")
        else:
            print(f"  WRONG    {entry['path']} does not match its manifest")
            problems += 1

    print("everything needed is in place" if problems == 0 else f"{problems} file(s) need attention")
    return 0 if problems == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
