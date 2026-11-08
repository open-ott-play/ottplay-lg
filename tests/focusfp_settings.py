#!/usr/bin/env python3
"""Record flow 6 against the synthetic PMS, waiting for actual input and page transitions.

Fixed sleeps on a 120 Hz host retained enough idle frames to exceed the recorder's decoded
memory budget. This uses the shared SimRun driver, waits for completed recorded frames, and
keeps the Settings -> Privacy -> Settings -> Legal -> Settings -> Home characterization.
The disabled-reporting build opens Policy in Privacy; it never selects Delete or opts in.
Invoke through tests/focusfp.sh --rec --only 6, or with --sim and a fresh --out directory.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time

from focusfp_check import check
from mock_fps import MockPms
from paging_link import KEY_APPLY_S, SimRun, wire

ROOT = Path(__file__).resolve().parent.parent


def completed_frame(runtime):
    """Read the writer's latest committed state row, tolerating its incomplete final line."""
    segments = sorted((runtime / "plxnative-recordings/latest").glob("rec-*.jsonl"))
    if not segments:
        return -1
    with segments[-1].open("rb") as stream:
        stream.seek(max(0, segments[-1].stat().st_size - 16384))
        rows = stream.read().splitlines()
    for row in reversed(rows):
        try:
            value = json.loads(row)
        except (ValueError, UnicodeDecodeError):
            continue
        if value.get("t") == "st":
            return value["f"]
    return -1


def record(binary, output, port, seed, existing_mock=False):
    output.mkdir(parents=True, exist_ok=True)
    runtime = output / "root-6"
    runtime.mkdir(exist_ok=True)
    if any(runtime.iterdir()):
        raise ValueError(f"refusing nonempty simulator root: {runtime}")
    mock = None
    sim = None
    try:
        if existing_mock:
            # The enclosing focusfp harness owns this mock; a real PMS has no such endpoint.
            if not isinstance(wire(f"http://127.0.0.1:{port}"), list):
                raise ValueError("the supplied server is not the synthetic PMS")
        else:
            mock = MockPms("127.0.0.1", port, ("--seed", str(seed)), output / "mock_pms.log").start()
        subprocess.run([str(binary), "--write-synthetic-initial",
                        str(runtime / "plxnative-app-init"), str(seed), str(port), "settings=root"], check=True)
        # SimRun inherits this drawable size; the authored UI remains 1920x1080.
        os.environ.setdefault("PLXNATIVE_WIN", "1920x1080")
        sim = SimRun(binary, port, {"plxnative-focus": "", "plxnative-noidle": "",
                                   "plxnative-rec": "", "plxnative-settings": "root"}, runtime, token=None)

        def wait(pattern, after=0):
            stamp = sim.wait_for(pattern, 20, after=after)
            if stamp is None or sim.proc.poll() is not None:
                raise RuntimeError(f"did not reach {pattern!r}; evidence: {runtime}")
            return stamp

        def stage(name, after=0):
            suffix = "fps=" if name == "home" else "overlay=" + name + " "
            wait(r"loop=.*route=home " + suffix, after)

        def key(name):
            stamp = sim.key(name)
            wait(r"key type=0x301", stamp)
            before = completed_frame(runtime)
            deadline = time.monotonic() + 20
            # Two committed frames keep consecutive Back actions out of the same dispatch
            # even when the host stalled while opening a document. No tape is rewritten.
            while completed_frame(runtime) < before + 2:
                if sim.proc.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError(f"no completed frame after {name}; evidence: {runtime}")
                time.sleep(0.01)
            time.sleep(KEY_APPLY_S)
            return stamp

        stage("settings")
        key("down")
        key("down")
        stage("privacy", key("ok"))
        key("ok")                       # The safe first row is the complete Policy.
        stage("privacy", key("back"))  # Policy -> Privacy.
        stage("settings", key("back"))
        key("down")
        stage("legal", key("ok"))
        key("ok")                       # Legal -> Policy.
        key("back")
        stage("settings", key("back"))
        stage("home", key("back"))
    finally:
        if sim is not None:
            sim.close()
        if mock is not None:
            mock.stop()

    if sim is None or sim.proc.returncode != 0:
        raise AssertionError(f"no successful diagnostic close; log assertions refused: {runtime}")

    lines = (runtime / "plxnative-events.log").read_text().splitlines()
    fingerprint = []
    previous = None
    for line in lines:
        if line.startswith("focus "):
            fingerprint.append(line)
            continue
        match = re.search(r"route=[a-z]+(?: overlay=[a-z]+)?", line)
        if match and match.group() != previous:
            previous = match.group()
            fingerprint.append("hb " + previous)
    path = output / "6-settings-family.fp"
    path.write_text("\n".join(fingerprint) + "\n")
    error = check(6, fingerprint)
    if error:
        raise AssertionError(f"{error}; evidence: {path}")
    if any(line.startswith(("rec: REFUSED", "replay: REFUSED")) for line in lines):
        raise AssertionError(f"the recorder refused this run; evidence: {runtime}")
    recording = runtime / "plxnative-recordings/latest"
    subprocess.run([str(ROOT / "tools/plxnative-rec"), "check", str(recording)], check=True)
    print(f"  [PASS] 6 settings-family: settled input and page transitions -> {path}")
    print(f"         recording: {recording}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sim", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--port", type=int, default=32498)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--existing-mock", action="store_true")
    args = parser.parse_args()
    record(args.sim.resolve(), args.out.resolve(), args.port, args.seed, args.existing_mock)


if __name__ == "__main__":
    main()
