#!/usr/bin/env python3
"""Check the committed public source boundary without reading local credentials."""
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    names = subprocess.check_output(
        ["git", "ls-files", "-z"], cwd=ROOT
    ).decode().split("\0")
    names = [name for name in names if name]
    if not names:
        raise SystemExit("No tracked source; stage the reviewed bootstrap first")
    forbidden = {
        ".tv-host", "local.env", "src/config.local.h", "tests/manifest.local.json",
        "pkg/auth.json", "pkg/lab.json", "pkg/telemetry.local.json",
        "pkg/ottplay-profile.json", "pkg/ottplay-control.json", "pkg/ott-state", "pkg/ott-state.json",
    }
    for name in names:
        if name in forbidden or name.startswith((".webos-ndk/", "build/", "vendor/build-cache/", "pkg/ott-state/", ".codex/", ".claude/", ".agents/")):
            raise SystemExit("Private/generated path is tracked: " + name)
        path = ROOT / name
        if path.is_symlink():
            resolved = path.resolve()
            if not resolved.is_relative_to(ROOT):
                raise SystemExit("Source symlink escapes repository: " + name)
        if path.is_file() and path.suffix in {".md", ".yml", ".yaml"}:
            text = path.read_text(encoding="utf-8")
            if re.search(r"/Users/(?!runner(?:/|\b)|YOUR_USER(?:/|\b)|username(?:/|\b))[^/\s]+/", text):
                raise SystemExit("Machine-specific home path in public documentation: " + name)
    evidence = json.loads((ROOT / "SOURCE-PROVENANCE.json").read_text())
    if evidence.get("prepared_tree") != "23c05264240c3ec4dea9465abfa6c67bf8e6e532":
        raise SystemExit("Unexpected import provenance")
    if len(evidence.get("patches", {})) != 38:
        raise SystemExit("Incomplete integration provenance")
    for name in ("LICENSE", "LICENSING.md", "THIRD-PARTY-NOTICES.md", "licenses/PlxNative-historical-MIT.txt"):
        if not (ROOT / name).is_file():
            raise SystemExit("Missing license or notice: " + name)
    descriptor = json.loads((ROOT / "pkg/appinfo.json").read_text())
    if descriptor.get("id") != "com.openottplay.player.native":
        raise SystemExit("Native installation identity changed")
    print("Public source/provenance/identity checks passed")


if __name__ == "__main__":
    main()
