#!/usr/bin/env python3
"""Bounded hosted-only build/receipt and separate real decoder qualification.

The output stage additionally checks owned hosted software GL and a virtual audio
monitor. Neither stage establishes television capacity or physical speaker output.
The fixture encoder is not a player dependency.
"""
import argparse
import array
import ctypes
import hashlib
import json
import os
import pathlib
import re
import select
import signal
import shutil
import struct
import subprocess
import sys
import tarfile
import time
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[1]
TEST = "local_multiview::real_qualification::nine_independent_real_decoders_video_audio_seek_withdrawal_and_retirement"
GRID_TEST = "local_multiview::real_qualification::four_live_hls_windows_advance_after_gateway_timeouts"
OUTPUT_TEST = "local_multiview::real_qualification::output::nine_real_yuv_textures_and_virtual_sdl_audio"
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

    def run(self, name, command, seconds, env=None, cwd=ROOT, tick=None):
        remaining = min(seconds, self.end - time.monotonic())
        if remaining <= 0:
            raise RuntimeError("Qualification budget exhausted")
        started = time.monotonic()
        print(f"START {name} (budget {remaining:.0f}s)", flush=True)
        process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            if tick is None:
                output, _ = process.communicate(timeout=remaining)
            else:
                deadline = time.monotonic() + remaining
                while True:
                    tick()
                    budget = deadline - time.monotonic()
                    if budget <= 0:
                        raise subprocess.TimeoutExpired(command, remaining)
                    try:
                        output, _ = process.communicate(timeout=min(0.05, budget))
                        tick()
                        break
                    except subprocess.TimeoutExpired:
                        continue
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
            print(f"END {name}: terminated, {time.monotonic() - started:.3f}s", flush=True)
            raise
        print(f"END {name}: exit {process.returncode}, {time.monotonic() - started:.3f}s", flush=True)
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


class VirtualOutput:
    """Owned per-job servers, never a runner's global audio/display session."""
    def __init__(self, runner, env, profile):
        self.runner = runner
        self.profile = profile
        self.root = runner.out / ("proof-" + profile)
        self.root.mkdir()
        self.processes = []
        self.env = dict(env, LIBGL_ALWAYS_SOFTWARE="1", SDL_AUDIODRIVER="pulseaudio",
                        PLXNATIVE_LOCAL_VIDEO_GL_PROFILE=profile,
                        PLXNATIVE_LOCAL_VIDEO_PROOF_DIR=str(self.root))
        self.recorder = None
        self.phase = None
        self.phase_measurements = {}
        self.phase_order = ("full", "half", "mute", "unmute")

    def background(self, name, command, env, stdout=None, pass_fds=()):
        name = name + "-" + self.profile
        log = (self.runner.out / f"{name}.log").open("wb")
        destination = stdout if stdout is not None else log
        print(f"START owned {name}", flush=True)
        try:
            process = subprocess.Popen(command, cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
                                       stdout=destination, stderr=log, start_new_session=True,
                                       pass_fds=pass_fds)
        finally:
            log.close()
        self.processes.append((name, process))
        return process

    def stop(self, name, process):
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=8)
        # A process can exit while leaving a child in its group; retire that group too.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        else:
            raise RuntimeError(f"Owned {name} left descendants after shutdown")
        self.runner.commands.append({"name": name, "pid": process.pid, "exit": process.returncode,
                                     "owned_cleanup": True})
        print(f"END owned {name}: retired", flush=True)

    def __enter__(self):
        try:
            read_fd, write_fd = os.pipe()
            try:
                display = self.background("xvfb", ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1920x1080x24",
                                                     "+extension", "GLX", "-nolisten", "tcp", "-noreset"],
                                          self.env, pass_fds=(write_fd,))
            finally:
                os.close(write_fd)
            try:
                if not select.select([read_fd], [], [], 15)[0]:
                    raise RuntimeError("Owned Xvfb did not announce a display")
                number = os.read(read_fd, 64).decode().strip()
                if display.poll() is not None or not number.isdigit():
                    raise RuntimeError("Owned Xvfb failed to become ready")
                self.env["DISPLAY"] = ":" + number
            finally:
                os.close(read_fd)
            private = self.runner.out / ("pulse-" + self.profile)
            private.mkdir(mode=0o700)
            xdg = private / "runtime"
            xdg.mkdir(mode=0o700)
            config = private / "config"
            config.mkdir(mode=0o700)
            socket = private / "native"
            pulse_config = private / "owned.pa"
            pulse_config.write_text(".fail\nload-module module-native-protocol-unix socket=" + str(socket)
                                    + " auth-anonymous=1\nload-module module-null-sink sink_name=ottplay_ci format=s16le rate=48000 channels=2"
                                    + " channel_map=front-left,front-right\nset-default-sink ottplay_ci\n"
                                    + "set-default-source ottplay_ci.monitor\n")
            self.env.update(PULSE_SERVER="unix:" + str(socket), PULSE_SINK="ottplay_ci", PULSE_LATENCY_MSEC="60",
                            PULSE_RUNTIME_PATH=str(private), XDG_RUNTIME_DIR=str(xdg), XDG_CONFIG_HOME=str(config),
                            # An explicit unreachable address prevents libdbus from
                            # autolaunching an unowned desktop/session bus via DISPLAY.
                            DBUS_SESSION_BUS_ADDRESS="unix:path=" + str(private / "nonexistent-session-bus"),
                            DBUS_SYSTEM_BUS_ADDRESS="unix:path=" + str(private / "nonexistent-system-bus"))
            pulse = self.background("pulseaudio", ["pulseaudio", "-n", "--daemonize=no", "--use-pid-file=no",
                                    "--exit-idle-time=-1", "--disallow-exit", "--disable-shm=yes", "--system=false",
                                    "--log-level=warning", "--log-target=stderr", "--file=" + str(pulse_config)], self.env)
            until = time.monotonic() + 15
            while not socket.exists():
                if pulse.poll() is not None or time.monotonic() >= until:
                    raise RuntimeError("Owned PulseAudio did not become ready")
                time.sleep(0.05)
            self.runner.run("pulse-ready-" + self.profile, ["pactl", "--server=" + self.env["PULSE_SERVER"], "info"], 10, self.env)
            with (self.root / "monitor.pcm").open("wb") as monitor:
                self.recorder = self.background("parec", ["parec", "--server=" + self.env["PULSE_SERVER"],
                    "--device=ottplay_ci.monitor", "--format=s16le", "--rate=48000", "--channels=2", "--raw",
                    "--latency-msec=20"], self.env, stdout=monitor)
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def __exit__(self, *_):
        errors = []
        for name, process in reversed(self.processes):
            try:
                self.stop(name, process)
            except BaseException as exc:
                errors.append(str(exc))
        self.processes.clear()
        if errors:
            raise RuntimeError("; ".join(errors))

    def phase_tick(self):
        request_path = self.root / "phase-request.json"
        if not request_path.exists():
            return
        request = json.loads(request_path.read_text())
        phase = request.get("phase")
        if self.phase is None:
            index = len(self.phase_measurements)
            if phase in self.phase_measurements:
                return
            if index >= len(self.phase_order) or phase != self.phase_order[index]:
                raise RuntimeError("Unexpected virtual gain measurement phase")
            path = self.root / ("phase-" + phase + ".pcm")
            with path.open("xb") as capture:
                recorder = self.background("parec-" + phase, ["parec", "--server=" + self.env["PULSE_SERVER"],
                    "--device=ottplay_ci.monitor", "--format=s16le", "--rate=48000", "--channels=2", "--raw",
                    "--latency-msec=20"], self.env, stdout=capture)
            self.phase = {"name": phase, "path": path, "process": recorder,
                          "started": time.monotonic(), "go": False}
        state = self.phase
        if phase != state["name"] or state["process"].poll() is not None:
            raise RuntimeError("Gain capture lost its phase or owned monitor")
        if not state["go"]:
            if state["path"].stat().st_size < 3840:
                if time.monotonic() - state["started"] > 2:
                    raise RuntimeError("Gain monitor did not connect before the PCM phase")
                return
            atomic_json(self.root / "phase-go.json", {"phase": phase})
            state["go"] = True
        done_path = self.root / "phase-done.json"
        if not done_path.exists() or json.loads(done_path.read_text()).get("phase") != phase:
            return
        self.stop("parec-" + phase + "-" + self.profile, state["process"])
        self.processes = [(name, p) for name, p in self.processes if p is not state["process"]]
        measured = steady_pcm(state["path"])
        if phase == "full" and measured["rms"] <= 1:
            raise RuntimeError("Full-gain steady monitor captured only silence")
        if phase == "half":
            ratio = measured["rms"] / self.phase_measurements["full"]["rms"]
            measured["ratio_to_full"] = ratio
            if not 0.4 <= ratio <= 0.65:
                raise RuntimeError("Measured half gain does not attenuate the actual sink")
        if phase == "mute" and measured["rms"] > 1:
            raise RuntimeError("Measured steady mute still contains audible PCM")
        if phase == "unmute":
            ratio = measured["rms"] / self.phase_measurements["full"]["rms"]
            measured["ratio_to_full"] = ratio
            if not 0.8 <= ratio <= 1.2:
                raise RuntimeError("Measured unmute did not restore full gain")
        self.phase_measurements[phase] = measured
        atomic_json(self.root / "phase-ack.json", {"phase": phase, "measured": True, **measured})
        self.phase = None

    def proof(self):
        if tuple(self.phase_measurements) != self.phase_order or self.phase is not None:
            raise RuntimeError("Virtual gain and mute phases were not independently measured")
        if self.recorder is None or self.recorder.poll() is not None:
            raise RuntimeError("Owned PCM monitor stopped before output qualification")
        time.sleep(0.1)
        self.stop("parec-" + self.profile, self.recorder)
        self.processes = [(name, p) for name, p in self.processes if p is not self.recorder]
        raw = (self.root / "monitor.pcm").read_bytes()
        if len(raw) < 3840 or len(raw) > 32 * 1024 * 1024 or len(raw) % 4:
            raise RuntimeError("Virtual audio monitor returned missing or unbounded stereo PCM")
        samples = array.array("h")
        samples.frombytes(raw)
        if sys.byteorder != "little":
            samples.byteswap()
        rms = (sum(float(s) * s for s in samples) / len(samples)) ** 0.5
        if rms <= 1.0:
            raise RuntimeError("Virtual audio monitor captured only silence")
        output = json.loads((self.root / "OUTPUT.json").read_text())
        checks = ("nine_readback", "cpu_yuv_reference", "single_region_update", "ordinary_ui_after_video",
                  "stale_audio_owner", "stale_gain_owner", "gain_mute_measured", "clear", "drain", "reopen", "cancel_retired")
        if output.get("schema") != 1 or output.get("width") != 384 or output.get("height") != 216:
            raise RuntimeError("Unexpected virtual graphics proof geometry")
        if output.get("gl_profile") != self.profile:
            raise RuntimeError("Output qualification used the wrong GL profile")
        expected_format = "GL_LUMINANCE" if self.profile == "es2" else "GL_RED"
        if output.get("texture_format") != expected_format:
            raise RuntimeError("Output qualification used the wrong video texture format")
        version = output.get("gl", {}).get("version", "")
        if not version or ("OpenGL ES" in version) != (self.profile == "es2"):
            raise RuntimeError("Actual GL identity does not match the requested profile")
        if output.get("pixel_format") != "RGBA8-top-down" or output.get("sdl_audio_driver") != "pulseaudio":
            raise RuntimeError("Output qualification did not use declared virtual formats")
        if any(output.get("checks", {}).get(name) is not True for name in checks):
            raise RuntimeError("Virtual output proof is incomplete")
        renderer = output.get("gl", {}).get("renderer", "").lower()
        if not any(name in renderer for name in ("llvmpipe", "softpipe", "swrast")):
            raise RuntimeError("Output qualification did not report a software GL renderer")
        images = {}
        for name in ("output", "output-updated", "output-ui"):
            path = self.root / (name + ".rgba")
            pixels = path.read_bytes()
            if len(pixels) != 384 * 216 * 4:
                raise RuntimeError("Invalid virtual framebuffer readback length")
            png = self.root / (name + ".png")
            png_rgba(png, 384, 216, pixels)
            images[name] = {"rgba_sha256": sha(path), "png_sha256": sha(png)}
        receipt = {"scope": "owned hosted Mesa/Xvfb + PulseAudio null sink; no physical device claim",
                   "monitor": {"format": "s16le-stereo-48000", "bytes": len(raw), "samples": len(samples),
                               "rms": rms, "peak": max(abs(s) for s in samples), "sha256": sha(self.root / "monitor.pcm")},
                   "images": images, "gain_phases": self.phase_measurements,
                   "pulse_latency_msec": 60, "output": output}
        write(self.root / "VIRTUAL.json", receipt)
        return receipt


def atomic_json(path, value):
    temporary = path.with_suffix(".tmp")
    write(temporary, value)
    temporary.replace(path)


def steady_pcm(path):
    raw = path.read_bytes()
    rate, channels = 48000, 2
    if len(raw) % 4 or not 1.5 * rate * channels * 2 <= len(raw) <= 32 * 1024 * 1024:
        raise RuntimeError("Gain phase returned a short, unaligned or unbounded capture")
    samples = array.array("h")
    samples.frombytes(raw)
    if sys.byteorder != "little":
        samples.byteswap()
    start, end = int(0.5 * rate * channels), int(0.1 * rate * channels)
    steady = samples[start:len(samples) - end]
    if not steady:
        raise RuntimeError("Gain phase has no steady measurement window")
    return {"format": "s16le-stereo-48000", "rate": rate, "channels": channels,
            "raw_bytes": len(raw), "raw_samples": len(samples), "raw_duration_s": len(samples) / (rate * channels),
            "trim_start_s": 0.5, "trim_end_s": 0.1, "steady_samples": len(steady),
            "steady_duration_s": len(steady) / (rate * channels),
            "rms": (sum(float(s) * s for s in steady) / len(steady)) ** 0.5,
            "peak": max(abs(s) for s in steady), "sha256": sha(path)}


def png_rgba(path, width, height, pixels):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    scanlines = b"".join(b"\0" + pixels[y * width * 4:(y + 1) * width * 4] for y in range(height))
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(scanlines)) + chunk(b"IEND", b""))


def qualify(runner, ident, artifact, virtual=False):
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
        if re.fullmatch(r"slot[2678]-hls/(?:media\.m3u8|segment[0-9]{3}\.ts)", record["name"]) is None:
            raise RuntimeError("Unexpected HLS fixture object")
    for slot in (2, 6, 7, 8):
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
    test = OUTPUT_TEST if virtual else TEST
    listed = runner.run("inventory", [binary, test, "--exact", "--ignored", "--list"], 30, env)
    if [line for line in listed.splitlines() if line.endswith(": test")] != [test + ": test"]:
        raise RuntimeError("Required ignored qualification test is missing or ambiguous")
    if virtual:
        proof = {}
        for profile in ("core", "es2"):
            # A new Rust process, GL context, display and audio server for each branch.
            with VirtualOutput(runner, env, profile) as output:
                result = runner.run("virtual-output-" + profile, [binary, test, "--exact", "--ignored", "--nocapture", "--test-threads=1"], 120, output.env, tick=output.phase_tick)
                if "running 1 test" not in result or "test result: ok. 1 passed; 0 failed; 0 ignored;" not in result:
                    raise RuntimeError("Virtual output did not execute exactly one passing test")
                proof[profile] = output.proof()
    else:
        result = runner.run("nine-sessions", [binary, test, "--exact", "--ignored", "--nocapture", "--test-threads=1"], 120, env)
    if "running 1 test" not in result or "test result: ok. 1 passed; 0 failed; 0 ignored;" not in result:
        raise RuntimeError("Real decoder qualification did not execute exactly one passing test")
    if not virtual:
        grid = runner.run("four-live-hls", [binary, GRID_TEST, "--exact", "--ignored", "--nocapture", "--test-threads=1"], 60, env)
        if "test result: ok. 1 passed; 0 failed; 0 ignored;" not in grid:
            raise RuntimeError("Four live HLS decoders did not pass gateway recovery")
    assert_source(ident)
    write(runner.out / "QUALIFIED.json", {"identity": ident, "test": test, "passed": 1 if virtual else 2, "failed": 0, "ignored": 0,
                                           "fixtures": manifest, "sources": {"held": 3, "http": 3, "hls": 3}, "scope": ("hosted Mesa/Xvfb and virtual PulseAudio null sink; no television or physical speaker claim"
                                                     if virtual else "software decoders only; no GL, audio sink or TV claim"),
                                           "virtual_output": proof if virtual else None,
                                           "four_live_hls_test": None if virtual else GRID_TEST,
                                           "configurations": ["core", "es2"] if virtual else ["default"],
                                           "counts_scope": "per configuration"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("build", "qualify", "output"))
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
            qualify(runner, ident, args.artifact.resolve(), virtual=args.stage == "output")
        success = True
    except BaseException as exc:
        error = str(exc)
        raise
    finally:
        write(runner.out / "RESULT.json", {"success": success, "error": error, "commands": runner.commands})


if __name__ == "__main__":
    main()
