#!/usr/bin/env python3
"""Check the committed public source boundary without reading local credentials."""
import json
from pathlib import Path
import re
import runpy
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def is_recording(path, data):
    if path.name == "manifest.json":
        doc = json.loads(data)
        return isinstance(doc, dict) and {"state_fp", "schema"} <= doc.keys()
    if path.match("rec-*.jsonl"):
        return True
    # A renamed JSONL segment retains the recorder's frame and event-kind fields.
    for line in data.splitlines():
        if line.strip():
            row = json.loads(line)
            if isinstance(row, dict) and "f" in row and "t" in row:
                return True
    return False


def check_replays(files):
    fixtures = Path("tests/fixtures/replay")
    recorder = runpy.run_path(str(ROOT / "tools/plxnative-rec"))
    alphabet_name = (fixtures / "ALPHABET.json").as_posix()
    if files.get(alphabet_name) not in ("100644", "100755"):
        raise SystemExit("Public replay alphabet must be a tracked regular file")
    data = subprocess.check_output(["git", "show", ":" + alphabet_name], cwd=ROOT)
    try:
        alphabet = recorder["parse_alphabet"](data.decode("utf-8"))
    except ValueError:
        raise SystemExit("Malformed tracked replay alphabet") from None
    recordings = {}
    for name, mode in files.items():
        path = Path(name)
        if "plxnative-recordings" in path.parts:
            raise SystemExit("Private recording directory is tracked")
        if path.name != "manifest.json" and path.suffix != ".jsonl":
            continue
        # Inspect the Git index. A symlink blob contains the target path, never its contents.
        data = subprocess.check_output(["git", "show", ":" + name], cwd=ROOT)
        try:
            if path.parent.parent != fixtures:
                if is_recording(path, data):
                    raise SystemExit("Recording data must be in a public replay fixture")
                continue
            if mode not in ("100644", "100755"):
                raise SystemExit("Public replay fixtures must contain regular files")
            if path.name != "manifest.json" and not re.fullmatch(r"rec-[0-9]{4}\.jsonl", path.name):
                raise SystemExit("Unexpected replay segment name")
            if recorder["check_file"](path.name, data.decode("utf-8"), alphabet):
                raise SystemExit("Public replay fixture failed synthetic validation")
        except ValueError:
            raise SystemExit("Malformed tracked recording data") from None
        recordings.setdefault(path.parent, set()).add(path.name)
    for files in recordings.values():
        if "manifest.json" not in files or len(files) < 2:
            raise SystemExit("Public replay fixture requires a tracked manifest and segment")


def main():
    index = subprocess.check_output(["git", "ls-files", "--stage", "-z"], cwd=ROOT)
    files = {}
    for record in index.decode().split("\0"):
        if record:
            metadata, name = record.split("\t", 1)
            files[name] = metadata.split()[0]
    if not files:
        raise SystemExit("No tracked source; stage the reviewed bootstrap first")
    forbidden = {
        ".tv-host", "local.env", "src/config.local.h", "tests/manifest.local.json",
        "pkg/auth.json", "pkg/lab.json", "pkg/telemetry.local.json",
        "pkg/ottplay-profile.json", "pkg/ottplay-control.json", "pkg/ott-state", "pkg/ott-state.json",
    }
    for name in files:
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
    check_replays(files)
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
