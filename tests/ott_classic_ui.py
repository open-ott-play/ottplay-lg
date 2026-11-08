#!/usr/bin/env python3
"""Capture ordinary Classic UI and actual category writes on a hosted Linux simulator.

No replay, seeded Saved image, test-only UI action, or production account is used.
The fixed 1920x1080 pointer locations come from the reviewed Classic fixture; state
writes are checked independently. Images still REQUIRE VISUAL REVIEW. In particular,
a focus element is not a page ID, and a valid PNG does not prove its contents.
"""
from __future__ import annotations

import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import threading
import time
from xml.sax.saxutils import escape

from ott_fixture_read import exact, inspect_record, json_value, private_read
from paging_link import SimRun

ROOT = Path(__file__).resolve().parents[1]
REQUIRED = ("category-all", "category-favorites", "category-nine")
SUPPLEMENTARY = ("providers-classic", "profiles-classic", "channel-list-classic",
                 "channel-browse", "category-picker", "search-editor", "search-results",
                 "search-cancel", "profiles-return", "providers-return", "exit-prompt", "exit-cancel")
ASSETS = ("appfont.ttf", "appfont-bold.ttf", "appfont-cjk.ttf", "appfont-emoji.ttf",
          "appfont-arabic.ttf", "appfont-arabic-bold.ttf", "appfont-hebrew.ttf",
          "appfont-hebrew-bold.ttf", "icon.png", "largeIcon.png", "splash.png",
          "OFL.txt", "le-roots.pem", "libass-plx-host.so.0")
FIRST_GROUP = "<News> / {category} & Music"
# The flat M3U category identity from ott_catalog_organization::provider_id.
# Display labels are deliberately not persisted as category IDs.
NINTH_CATEGORY = "organization:provider:" + hashlib.sha256(b"Group 07").hexdigest()


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def png_reader():
    # Reuse the existing CRC/chunk validator; no second PNG parser.
    spec = importlib.util.spec_from_file_location("sim_smoke", ROOT / "tools/sim-smoke.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.png_size


def git_value(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args], text=True, timeout=5).strip()


class Playlist(ThreadingHTTPServer):
    # server_close joins active requests; each socket also has a finite read timeout.
    daemon_threads = False

    def __init__(self):
        self.hits = Counter()
        self.lock = threading.Lock()
        self.started_at = int(time.time())
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def setup(self):
                super().setup()
                self.connection.settimeout(3)

            def log_message(self, *_):
                pass

            def do_GET(self):
                with owner.lock:
                    owner.hits[self.path] += 1
                if self.path == "/fixture.m3u":
                    body, kind = owner.playlist(), "audio/x-mpegurl"
                elif self.path == "/fixture.xml":
                    body, kind = owner.guide(), "application/xml"
                else:
                    # UI-only fixture. Never imply a decoder or playback pass.
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Type", kind)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        super().__init__(("127.0.0.1", 0), Handler)
        self.origin = f"http://127.0.0.1:{self.server_port}"
        self.url = self.origin + "/fixture.m3u"
        self.thread = threading.Thread(target=self.serve_forever, kwargs={"poll_interval": .1}, daemon=True)

    def playlist(self):
        lines = [f'#EXTM3U url-tvg="{self.origin}/fixture.xml"']
        for number in range(1, 9):
            group = FIRST_GROUP if number <= 2 else f"Group {number - 1:02}"
            epg = "" if number in (2, 8) else f'tvg-id="visual-{number:02}" '
            duration = 120 if number == 2 else -1
            lines += [f'#EXTINF:{duration} {epg}group-title="{group}",Fixture Channel {number:02}',
                      f"{self.origin}/unavailable/visual-{number:02}.ts"]
        return ("\n".join(lines) + "\n").encode()

    def guide(self):
        def stamp(value):
            return datetime.fromtimestamp(value, timezone.utc).strftime("%Y%m%d%H%M%S +0000")
        start = self.started_at - 600
        lines = ['<?xml version="1.0" encoding="UTF-8"?><tv>']
        for number in (1, 3, 4, 5, 6, 7):
            lines.append(f'<channel id="visual-{number:02}"><display-name>Fixture Channel {number:02}</display-name></channel>')
        for number in (1, 3, 4, 5, 6, 7):
            for offset, title in ((0, "Current"), (3600, "Next")):
                lines.append(f'<programme channel="visual-{number:02}" start="{stamp(start + offset)}" stop="{stamp(start + offset + 3600)}">'
                             f'<title>{title} Programme {number:02}</title><desc>{escape(f"Synthetic channel {number:02}. Current programme description for visual acceptance.")}</desc></programme>')
        return ("\n".join(lines) + "\n</tv>\n").encode()

    def counts(self):
        with self.lock:
            return dict(self.hits)


class Scenario:
    def __init__(self, sim, output, server, deadline, receipt):
        self.sim, self.output, self.server = sim, output, server
        self.deadline, self.receipt = deadline, receipt
        self.png_size = png_reader()
        self.nshot = 0

    def remaining(self):
        if self.sim.proc.poll() is not None:
            raise RuntimeError(f"simulator exited: {self.sim.proc.returncode}")
        left = self.deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError("Classic UI scenario deadline exceeded")
        return left

    def wait(self, predicate, label, timeout=15):
        until = time.monotonic() + min(timeout, self.remaining())
        while time.monotonic() < until:
            self.remaining()
            value = predicate()
            if value:
                return value
            time.sleep(.05)
        raise TimeoutError(label)

    def pause(self, seconds=.3):
        time.sleep(min(seconds, self.remaining()))
        self.remaining()

    def focus(self):
        latest = self.sim.latest(lambda _, line: line.startswith("focus route="))
        if not latest:
            return None
        at, line = latest
        # Do not search backwards for an older OTT focus when another route owns input.
        return line if (line.startswith("focus route=ott ") and " press=0" in line
                        and time.monotonic() - at >= .25) else None

    def key(self, token):
        self.remaining()
        self.receipt["inputs"].append(token)
        self.sim.key(token)
        self.pause()

    def expect_focus(self, elem, label):
        expected = f"focus route=ott elem={elem} press=0"
        self.wait(lambda: self.focus() == expected, f"{label}: expected {expected}")
        self.receipt.setdefault("navigation", []).append({"label": label, "focus": expected})

    def saved(self):
        try:
            record = inspect_record(self.output / "state", "ott-state")
        except FileNotFoundError:
            return None
        value = record["value"]
        record["value"] = json_value(value) if isinstance(value, str) else value
        return record

    def saved_when(self, condition, label, after_revision=None):
        def accepted():
            record = self.saved()
            return record if (record and (after_revision is None or record["record_revision"] > after_revision)
                              and condition(record["value"])) else None
        record = self.wait(accepted, label)
        self.receipt["writes"].append({"label": label, "revision": record["record_revision"],
                                      "sha256": record["record_sha256"]})
        return record

    def mode(self):
        path = self.output / "state/records/ott-preferences.json"
        try:
            raw = private_read(path, 2 * 1024 * 1024)
        except FileNotFoundError:
            return None
        record = json_value(raw)
        exact(record, ("format", "domain", "key", "version", "revision", "state"))
        if (record.get("format"), record.get("domain"), record.get("key"), record.get("version")) != (
                "plxnative-record", "ott-preferences", "ott-preferences", 1):
            raise ValueError("unexpected public preferences record")
        if type(record["version"]) is not int or type(record["revision"]) is not int or record["revision"] <= 0:
            raise ValueError("invalid public preference revision")
        exact(record["state"], ("Data",))
        exact(record["state"]["Data"], ("payload",))
        value = json_value(record["state"]["Data"]["payload"])
        exact(value, ("value", "revision", "epoch"))
        if (type(value["revision"]) is not int or value["revision"] != record["revision"]
                or type(value["epoch"]) is not int or not 0 < value["epoch"] < 2**64):
            raise ValueError("public preference envelope and payload disagree")
        preferences = value["value"]
        if (not isinstance(preferences, dict) or set(preferences) - {"version", "device_id", "interface_mode"}
                or type(preferences.get("version")) is not int or preferences["version"] != 1):
            raise ValueError("unexpected preferences in fresh UI fixture")
        mode = preferences.get("interface_mode")
        if mode is None:
            return None
        if type(mode) is not int or mode not in (0, 1):
            raise ValueError("invalid interface mode")
        if mode != 0:
            return None
        return {"revision": record["revision"], "sha256": hashlib.sha256(raw).hexdigest(),
                "interface_mode": 0, "scope": "ordinary public preference write, not sealed-state proof"}

    def capture(self, name):
        before = self.wait(self.focus, f"settled OTT focus before {name}")
        self.nshot += 1
        path = self.sim.dir / f"capture-{self.nshot}.png"
        if path.exists():
            raise ValueError("capture sequence was reused")
        sent = self.sim.key("shot")
        def written():
            marker = self.sim.latest(lambda at, line: at >= sent and line == f"shot: wrote 1920x1080 to {path}")
            return marker and path.is_file()
        self.wait(written, f"fresh complete PNG {name}")
        if self.png_size(path) != (1920, 1080):
            raise ValueError("wrong capture geometry")
        after = self.wait(self.focus, f"settled OTT focus after {name}")
        if before != after:
            raise ValueError(f"focus changed during {name}")
        target = self.output / (name + ".png")
        shutil.copyfile(path, target)
        record = self.saved()
        value = record["value"] if record else {}
        self.receipt["captures"].append({"id": name, "path": target.name, "sha256": sha(target),
            "focus_before": before, "focus_after": after, "visual_verdict": "NOT_REVIEWED",
            "saved_selection": {"category": value.get("category"), "favorites_selected": value.get("favorites_selected"),
                                "active_profile": value.get("profiles", {}).get("active_profile")},
            "saved_revision": record["record_revision"] if record else None})
        print(json.dumps({"captured": name, "visual_verdict": "NOT_REVIEWED"}), flush=True)

    def select_category(self, digit, category, favorites, name):
        self.key("k:0,489")  # existing Blue remote handler
        self.capture(name + "-picker")
        revision = self.saved()["record_revision"]
        self.key(f"k:{48 + digit},{29 + digit}")
        self.saved_when(lambda v: v.get("category") == category and v.get("favorites_selected") is favorites,
                        f"category {digit} Saved acknowledgement", revision)
        self.capture(name)

    def run(self):
        fifo = self.sim.dir / "plxnative-remote"
        self.wait(lambda: fifo.exists() and stat.S_ISFIFO(fifo.lstat().st_mode), "remote FIFO ready")
        self.capture("boot")
        for token in ("ok", "ok", "ok", "txt:" + self.server.url, "down", "down", "ok"):
            self.key(token)
        def profile_matches(v):
            profiles = v.get("profiles", {})
            return profiles.get("active_profile") == "m3u-1" and any(
                p.get("id") == "m3u-1" and p.get("source") == self.server.url and p.get("kind") == "m3u"
                for p in profiles.get("profiles", []))
        self.saved_when(profile_matches, "ordinary M3U profile Saved acknowledgement")
        self.wait(lambda: self.server.counts().get("/fixture.m3u", 0) and self.server.counts().get("/fixture.xml", 0), "playlist and EPG requests")
        self.capture("modern-channel-list")
        # These two opening clicks are unchanged in the captured Modern fixture.
        for token in ("back", "back", "ck:158,177", "ck:1120,606"):
            self.key(token)
        # In this fresh eight-channel fixture, Settings allocates keys 43..58:
        # seven groups (including Interface colors), Profiles/Parental/Remote,
        # then Interface mode at 53; the final two rows are Torrent and Stremio.
        # The old row-coordinate now opens Remote.
        # Walk real Down input and fail at the first changed/missed focus instead
        # of continuing blind clicks into another page. Keys 59/60 are the
        # Classic/Modern picker rows; Modern is the initially selected value.
        for elem in range(43, 54):
            if elem != 43:
                self.key("down")
            self.expect_focus(elem, "Settings row")
        self.key("ok")
        self.expect_focus(60, "Modern mode picker")
        self.key("up")
        self.expect_focus(59, "Classic mode picker")
        self.key("ok")
        self.receipt["classic_preference"] = self.wait(self.mode, "ordinary Classic mode write")
        self.capture("classic-selected")
        self.key("back")
        self.capture("settings-classic")
        self.key("back")
        self.capture("providers-classic")
        self.key("ok")
        self.capture("profiles-classic")
        self.key("down")  # Classic's separate Load playlist row
        self.key("ok")
        self.capture("channel-list-classic")
        self.key("ck:811,1023")
        self.capture("channel-browse")
        revision = self.saved()["record_revision"]
        self.key("ck:1066,865")  # Add to Favorites in this eight-row fixture
        def has_first(v):
            entries = v.get("favorites", {}).get("entries", [])
            return len(entries) == 1 and entries[0].get("locator") == {"kind": "channel", "channelId": "m3u-epg:visual-01"}
        self.saved_when(has_first, "first channel favorite Saved acknowledgement", revision)
        self.capture("favorite-added")
        self.key("k:0,489")
        self.capture("category-picker")
        # Selecting a different category first makes even All require a new real write.
        self.key("k:57,38")
        self.saved_when(lambda v: v.get("category") == NINTH_CATEGORY and v.get("favorites_selected") is False,
                        "ninth category setup")
        self.select_category(1, "", False, "category-all")
        self.select_category(2, "", True, "category-favorites")
        self.select_category(9, NINTH_CATEGORY, False, "category-nine")
        # Search is supplementary visual evidence; query one row then cancel it.
        self.select_category(1, "", False, "all-before-search")
        self.key("ck:811,1023")
        self.key("ck:1100,505")
        self.capture("search-editor")
        self.key("ok")
        self.key("txt:Fixture+Channel+08")
        self.key("down")
        self.key("down")
        self.key("ok")
        self.capture("search-results")
        self.key("back")
        self.capture("search-cancel")
        self.key("back")
        self.capture("profiles-return")
        self.key("back")
        self.capture("providers-return")
        self.key("back")
        self.capture("exit-prompt")
        self.key("back")
        self.capture("exit-cancel")
        final = self.saved()
        if not has_first(final["value"]):
            raise ValueError("Favorites changed while navigating")
        self.receipt["final_favorite"] = {"channelId": "m3u-epg:visual-01", "revision": final["record_revision"], "sha256": final["record_sha256"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sim", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args()
    if sys.platform != "linux" or os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        parser.error("run only on a GitHub-hosted Linux runner")
    if not 30 <= args.timeout <= 300:
        parser.error("timeout must be between 30 and 300 seconds")
    binary, source_assets = args.sim.resolve(strict=True), args.assets.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    runtime, assets, state = (output / name for name in ("runtime", "assets", "state"))
    for directory in (runtime, assets, state):
        directory.mkdir(mode=0o700)
    receipt = {"schema": 1, "status": "INCOMPLETE", "visual_verdict": "NOT_REVIEWED",
        "scope": "normal OTT Classic UI with synthetic M3U; not TV, decoder, or full parity acceptance",
        "source_commit": git_value("rev-parse", "HEAD"), "source_tree": git_value("rev-parse", "HEAD^{tree}"),
        "source_diff_sha256": hashlib.sha256(subprocess.check_output(["git", "-C", str(ROOT), "diff", "HEAD", "--"], timeout=5)).hexdigest(),
        "driver_sources": {name: sha(Path(__file__).parent / name) for name in (
            "ott_classic_ui.py", "ott_fixture_read.py", "paging_link.py")},
        "binary_sha256": sha(binary), "required_captures": list(REQUIRED), "supplementary_captures": list(SUPPLEMENTARY),
        "inputs": [], "writes": [], "captures": [], "cleanup": {}}
    server = sim = None
    started = time.monotonic()
    try:
        for name in ASSETS:
            path = source_assets / name
            if path.is_file():
                shutil.copyfile(path, assets / name)
        for required in ("appfont.ttf", "appfont-bold.ttf", "libass-plx-host.so.0"):
            if not (assets / required).is_file():
                raise ValueError(f"missing renderer asset {required}")
        receipt["assets"] = {p.name: sha(p) for p in sorted(assets.iterdir())}
        server = Playlist()
        server.thread.start()
        receipt["fixture"] = {"origin": server.origin, "channels": 8, "categories_with_all_and_favorites": 9,
                              "playlist_sha256": hashlib.sha256(server.playlist()).hexdigest(), "programme_origin": server.started_at}
        env = {k: v for k, v in os.environ.items() if not k.startswith(("PLXNATIVE_", "OTTPLAY_", "OTTP_", "PLX_"))}
        env.update(PLXNATIVE_RUNTIME_DIR=str(runtime), PLXNATIVE_APP_DIR=str(assets),
                   PLXNATIVE_WIN="1920x1080", PLXNATIVE_SHOT=str(runtime / "capture.png"),
                   LIBGL_ALWAYS_SOFTWARE="1", LC_ALL="C.UTF-8", LANG="C.UTF-8", LC_MESSAGES="C.UTF-8")
        argv = [str(binary), "--fixture-state", str(state), "--fixture-create", "127.0.0.1", str(server.server_port)]
        sim = SimRun(binary, server.server_port, {"plxnative-focus": ""}, runtime, token=None,
                     argv=argv, environment=env, log_path=output / "process.log")
        Scenario(sim, output, server, started + args.timeout - 15, receipt).run()
        captured = {item["id"] for item in receipt["captures"]}
        if not set(REQUIRED + SUPPLEMENTARY) <= captured:
            raise ValueError("missing required capture")
        receipt["status"] = "CAPTURED_REQUIRES_VISUAL_REVIEW"
    except Exception as error:
        receipt["error"] = f"{type(error).__name__}: {error}"
    finally:
        if sim is not None:
            try:
                sim.close()
                receipt["cleanup"]["simulator_exited"] = sim.proc.poll() is not None
                receipt["cleanup"]["log_reader_exited"] = not sim.thread.is_alive()
            except Exception as error:
                receipt["cleanup"]["error"] = str(error)
        if server is not None:
            receipt["http_requests"] = server.counts()
            if server.thread.is_alive():
                server.shutdown()
                server.thread.join(3)
            server.server_close()
            receipt["cleanup"]["listener_exited"] = not server.thread.is_alive()
        if not all(receipt["cleanup"].get(key) is True for key in ("simulator_exited", "log_reader_exited", "listener_exited")):
            receipt["status"] = "INCOMPLETE"
        receipt["elapsed_s"] = round(time.monotonic() - started, 3)
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({"status": receipt["status"], "receipt": str(output / "receipt.json"), "error": receipt.get("error")}), flush=True)
    return 0 if receipt["status"] == "CAPTURED_REQUIRES_VISUAL_REVIEW" else 1


if __name__ == "__main__":
    raise SystemExit(main())
