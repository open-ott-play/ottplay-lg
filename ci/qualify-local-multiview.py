#!/usr/bin/env python3
"""Bounded hosted-only build/receipt and separate real decoder qualification.

This checks software decoder behavior, not GL, an SDL audio device, television
performance or nine hardware pipelines. The fixture encoder is not a player dependency.
"""
import argparse
import ctypes
import hashlib
import json
import os
import pathlib
import re
import signal
import shutil
import subprocess
import sys
import tarfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
TEST = "local_multiview::real_qualification::nine_independent_real_decoders_video_audio_seek_withdrawal_and_retirement"
DECODERS = ("h264", "hevc", "mpeg2video", "mpeg4", "vp8", "vp9", "mjpeg", "vc1", "wmv3", "aac")
INPUTS = ("ci/build-ffmpeg.sh", "ci/dependencies.json", "ci/ffabi-assert.c", "ci/sanitize-ffmpeg-config.py",
          "ci/qualify-local-multiview.py", "ci/generate-local-multiview-fixtures.py",
          "rust-modules/media/src/ff.rs", "rust-modules/media/src/ff_local_video.rs",
          "rust-modules/media/src/local_multiview.rs", "rust-modules/media/src/local_multiview_real_tests.rs")


def sha(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def identity():
    if os.environ.get("GITHUB_ACTIONS") != "true" or sys.platform != "linux":
        raise RuntimeError("This qualifier is restricted to hosted Linux CI")
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    if commit != os.environ["GITHUB_SHA"]:
        raise RuntimeError("Checkout and workflow source differ")
    return {"commit": commit, "tree": subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=ROOT, text=True).strip(),
            "run_id": os.environ["GITHUB_RUN_ID"], "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
            "inputs": {name: sha(ROOT / name) for name in INPUTS}}


def assert_source(ident):
    # HEAD/tree alone cannot detect a generator or build script mutating tracked inputs.
    if identity() != ident:
        raise RuntimeError("Qualification source identity changed during execution")
    changed = subprocess.run(["git", "diff", "--quiet", "HEAD", "--"], cwd=ROOT, timeout=30)
    if changed.returncode != 0:
        raise RuntimeError("Qualification modified the tracked source tree")


def inventory(root):
    result = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            target = os.readlink(path)
            if pathlib.Path(target).is_absolute() or not path.resolve(strict=True).is_relative_to(root.resolve()):
                raise RuntimeError("Decoder inventory contains an escaping or absolute symlink")
            result[str(path.relative_to(root))] = {"symlink": target}
        elif path.is_file():
            result[str(path.relative_to(root))] = {"sha256": sha(path), "bytes": path.stat().st_size}
    return result


class Runner:
    def __init__(self, out, seconds):
        self.out, self.end = out, time.monotonic() + seconds
        self.commands = []

    def run(self, name, command, seconds, env=None, cwd=ROOT):
        remaining = min(seconds, self.end - time.monotonic())
        if remaining <= 0:
            raise RuntimeError("Qualification budget exhausted")
        started = time.monotonic()
        process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            output, _ = process.communicate(timeout=remaining)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                output, _ = process.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                output, _ = process.communicate(timeout=8)
            (self.out / f"{name}.log").write_bytes(output)
            self.commands.append({"name": name, "command": command, "exit": process.returncode,
                                  "terminated": True, "seconds": round(time.monotonic() - started, 3)})
            raise
        (self.out / f"{name}.log").write_bytes(output)
        self.commands.append({"name": name, "command": command, "exit": process.returncode,
                              "seconds": round(time.monotonic() - started, 3)})
        if process.returncode:
            print(output.decode(errors="replace")[-12000:], file=sys.stderr)
            raise RuntimeError(f"{name} exited {process.returncode}")
        return output.decode(errors="strict")


def cabi(runner, prefix):
    runner.run("cabi", ["cc", "-std=c11", "-I", str(prefix / "include"), "-c",
                        "ci/ffabi-assert.c", "-o", str(runner.out / "ffabi.o")], 60)


def registry(prefix):
    lib = prefix / "lib"
    handles = []
    for stem, major in (("avutil", 61), ("avcodec", 63), ("avformat", 63)):
        path = lib / f"lib{stem}-plx.so.{major}"
        if not path.is_file() or not path.resolve().is_relative_to(prefix.resolve()):
            raise RuntimeError(f"Missing bundled library: {path.name}")
        handles.append(ctypes.CDLL(str(path), mode=ctypes.RTLD_GLOBAL))
    codec = handles[1]
    codec.avcodec_configuration.restype = ctypes.c_char_p
    config = codec.avcodec_configuration().decode()
    if "--disable-network" not in config or "--disable-everything" not in config:
        raise RuntimeError("Bundled configuration lost its closed component registry")
    codec.avcodec_find_decoder_by_name.argtypes = [ctypes.c_char_p]
    codec.avcodec_find_decoder_by_name.restype = ctypes.c_void_p
    found = {name: bool(codec.avcodec_find_decoder_by_name(name.encode())) for name in DECODERS}
    if not all(found.values()):
        raise RuntimeError(f"Required bundled decoder missing: {found}")
    return {"configuration": config, "decoders": found}


def build(runner, ident):
    env = dict(os.environ, HOST="1", JOBS="1", RELEASE="1", PLX_BUILD_CACHE=str(runner.out / "cache"))
    try:
        runner.run("ffmpeg-build", ["sh", "ci/build-ffmpeg.sh"], 1800, env)
    finally:
        for name in ("configure.log", "build.log"):
            for log in (runner.out / "cache" / "ffmpeg").glob("*/" + name):
                shutil.copyfile(log, runner.out / ("ffmpeg-" + name))
    prefix = ROOT / "vendor/ffmpeg-prefix-host"
    cabi(runner, prefix)
    components = registry(prefix)
    dep = next(d for d in json.loads((ROOT / "ci/dependencies.json").read_text()) if d["id"] == "ffmpeg")
    tar = pathlib.Path(subprocess.check_output(
        [sys.executable, "ci/deps.py", "cache-dir"], cwd=ROOT, text=True).strip()) / dep["archive"]
    if not tar.is_file() or sha(tar) != dep["sha256"]:
        raise RuntimeError("Corresponding source tarball does not match the dependency pin")
    assert_source(ident)
    receipt = {"schema": 1, "identity": ident, "ffmpeg": dep, "registry": components,
               "prefix": inventory(prefix), "cabi": {"passed": True, "source_sha256": sha(ROOT / "ci/ffabi-assert.c")}}
    write(runner.out / "BUILD.json", receipt)
    payload = runner.out / "decoder-bundle.tar.gz"
    with tarfile.open(payload, "w:gz") as archive:
        archive.add(prefix, arcname="prefix")
        archive.add(tar, arcname="source/" + dep["archive"])
        archive.add(runner.out / "BUILD.json", arcname="BUILD.json")
    (runner.out / "decoder-bundle.sha256").write_text(sha(payload) + "\n")


def verify(runner, ident, artifact):
    payload = artifact / "decoder-bundle.tar.gz"
    expected = (artifact / "decoder-bundle.sha256").read_text().strip()
    if len(expected) != 64 or sha(payload) != expected:
        raise RuntimeError("Decoder artifact hash mismatch")
    destination = runner.out / "verified"
    destination.mkdir()
    with tarfile.open(payload, "r:gz") as archive:
        members = archive.getmembers()
        if len(members) > 10000 or sum(m.size for m in members) > 512 * 1024 * 1024:
            raise RuntimeError("Decoder artifact exceeds extraction budget")
        archive.extractall(destination, filter="data")
    receipt = json.loads((destination / "BUILD.json").read_text())
    if receipt.get("schema") != 1 or receipt.get("identity") != ident:
        raise RuntimeError("Decoder artifact comes from another run, attempt or source tree")
    dep = next(d for d in json.loads((ROOT / "ci/dependencies.json").read_text()) if d["id"] == "ffmpeg")
    if receipt["ffmpeg"] != dep or sha(destination / "source" / dep["archive"]) != dep["sha256"]:
        raise RuntimeError("Decoder corresponding source identity changed")
    prefix = destination / "prefix"
    if inventory(prefix) != receipt["prefix"]:
        raise RuntimeError("Decoder library/header inventory changed")
    if receipt["cabi"] != {"passed": True, "source_sha256": sha(ROOT / "ci/ffabi-assert.c")}:
        raise RuntimeError("Missing C ABI receipt")
    cabi(runner, prefix)
    if registry(prefix) != receipt["registry"]:
        raise RuntimeError("Decoder configuration or registry changed")
    write(runner.out / "VERIFIED.json", {"identity": ident, "bundle_sha256": expected, "cabi": True,
                                          "registry": receipt["registry"]})
    return prefix


def qualify(runner, ident, artifact):
    prefix = verify(runner, ident, artifact)
    fixtures = runner.out / "fixtures"
    runner.run("fixtures", [sys.executable, "ci/generate-local-multiview-fixtures.py", str(fixtures)], 600)
    manifest = json.loads((fixtures / "MANIFEST.json").read_text())
    records, hls = manifest["videos"], manifest["hls_objects"]
    if manifest.get("schema") != 1 or manifest.get("video_count") != 9 or len(records) != 9 or {r["name"] for r in records} != {f"slot{i}.mp4" for i in range(9)}:
        raise RuntimeError("Fixture video inventory is incomplete")
    if not 6 <= len(hls) <= 100:
        raise RuntimeError("HLS fixture inventory is incomplete or unbounded")
    for record in hls:
        if re.fullmatch(r"slot[267]-hls/(?:media\.m3u8|segment[0-9]{3}\.ts)", record["name"]) is None:
            raise RuntimeError("Unexpected HLS fixture object")
    for slot in (2, 6, 7):
        names = {record["name"] for record in hls}
        if f"slot{slot}-hls/media.m3u8" not in names or f"slot{slot}-hls/segment000.ts" not in names:
            raise RuntimeError("HLS fixture playlist or segment is missing")
    expected_files = {record["name"] for record in records + hls}
    if len(expected_files) != len(records) + len(hls):
        raise RuntimeError("Duplicate fixture objects")
    actual_files = {str(path.relative_to(fixtures)) for path in fixtures.rglob("*") if path.is_file()}
    if actual_files != expected_files | {"MANIFEST.json"}:
        raise RuntimeError("Fixture directory has unrelated files")
    for record in records + hls:
        path = fixtures / record["name"]
        if path.is_symlink() or not path.resolve().is_relative_to(fixtures.resolve()) or not path.is_file():
            raise RuntimeError("Fixture object escaped its source directory")
        if path.stat().st_size != record["bytes"] or not 0 < record["bytes"] <= 16 * 1024 * 1024 or sha(path) != record["sha256"]:
            raise RuntimeError("Fixture hash or bounded size mismatch")
    env = dict(os.environ, CARGO_BUILD_JOBS="1", CARGO_INCREMENTAL="0", PLXNATIVE_APP_DIR=str(prefix / "lib"),
               PLXNATIVE_LOCAL_VIDEO_FIXTURES=str(fixtures), PLXNATIVE_RUNTIME_DIR=str(runner.out / "runtime"))
    env.pop("RUST_MIN_STACK", None)
    (runner.out / "runtime").mkdir()
    compiled = runner.run("media-compile", ["cargo", "test", "--locked", "--lib", "-p", "plx_media", "--features", "hostsim",
                                           "--no-run", "--message-format=json"], 1200, env, ROOT / "rust-modules")
    binaries = set()
    for line in compiled.splitlines():
        # Cargo/rustup status uses stderr; retained logs combine it with JSON stdout.
        if not line.startswith("{"):
            continue
        item = json.loads(line)
        if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "plx_media" and item.get("profile", {}).get("test") and item.get("executable"):
            binaries.add(item["executable"])
    if len(binaries) != 1:
        raise RuntimeError("Expected exactly one media unit-test executable")
    binary = binaries.pop()
    listed = runner.run("inventory", [binary, TEST, "--exact", "--ignored", "--list"], 30, env)
    if [line for line in listed.splitlines() if line.endswith(": test")] != [TEST + ": test"]:
        raise RuntimeError("Required ignored qualification test is missing or ambiguous")
    result = runner.run("nine-sessions", [binary, TEST, "--exact", "--ignored", "--nocapture", "--test-threads=1"], 120, env)
    if "running 1 test" not in result or "test result: ok. 1 passed; 0 failed; 0 ignored;" not in result:
        raise RuntimeError("Real decoder qualification did not execute exactly one passing test")
    assert_source(ident)
    write(runner.out / "QUALIFIED.json", {"identity": ident, "test": TEST, "passed": 1, "failed": 0, "ignored": 0,
                                           "fixtures": manifest, "sources": {"held": 3, "http": 3, "hls": 3}, "scope": "software decoders only; no GL, audio sink or TV claim"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("build", "qualify"))
    parser.add_argument("--out", required=True, type=pathlib.Path)
    parser.add_argument("--artifact", type=pathlib.Path)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    runner = Runner(args.out.resolve(), 1950 if args.stage == "build" else 2100)
    success, error = False, None
    try:
        ident = identity()
        assert_source(ident)
        if args.stage == "build":
            build(runner, ident)
        else:
            if args.artifact is None:
                parser.error("--artifact is required for qualify")
            qualify(runner, ident, args.artifact.resolve())
        success = True
    except BaseException as exc:
        error = str(exc)
        raise
    finally:
        write(runner.out / "RESULT.json", {"success": success, "error": error, "commands": runner.commands})


if __name__ == "__main__":
    main()
