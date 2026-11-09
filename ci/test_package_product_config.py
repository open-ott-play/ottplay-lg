#!/usr/bin/env python3
"""Exercise the real package CLI on small private repositories; no build or network."""
import ast
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
APP = "com.openottplay.player.native"
VERSION = "0.3.0"
CONFIG = "ci/ottplay-integration/release/product.json"
NOTES = "ci/ottplay-integration/release/notes/v0.3.0.md"
AUDIT = "ci/ottplay-integration/release/audits/v0.3.0.md"
NOTE_TEXT = "# OTTPlay native\n\nVerify the downloaded files with `sha256sum -c ottplay-native-webos-0.3.0.sha256`.\n"
AUDIT_TEXT = "\n\n".join("## " + title + "\n\nNot claimed by this fixture." for title in (
    "Device test evidence", "External compatibility reports known at release time",
    "Compatibility tiers, and what moved", "Known issues at release, with provenance",
    "Release configuration")) + "\n\n<!-- BEGIN GENERATED -->\n<!-- END GENERATED -->\n"


class ProductPackage(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        for name in ("check-package.py", "mkipk.py", "flavor.py", "version_rule.py", "deps.py",
                     "dependencies.json", "install-identities.json"):
            target = self.root / "ci" / name
            target.parent.mkdir(exist_ok=True)
            shutil.copyfile(ROOT / "ci" / name, target)
        self.appinfo = {"id": APP, "version": VERSION, "title": "OTTPlay",
                        "appDescription": "Plex media client", "type": "native"}
        self.write("pkg/appinfo.json", json.dumps(self.appinfo))
        self.write("ipkroot/ctl/control", "Package: " + APP + "\nVersion: " + VERSION + "\n")
        self.write("rust-modules/Cargo.toml", '[package]\nversion = "' + VERSION + '"\n')
        self.write("pkg/resources/fr/appinfo.json", json.dumps({"title": "OTTPlay", "appDescription": "Client de Plex"}))
        self.write(NOTES, NOTE_TEXT)
        self.write(AUDIT, AUDIT_TEXT)
        self.config = {"schema": 1, "product": "ottplay-native", "application_id": APP,
                       "version": VERSION, "notes": NOTES, "audit": AUDIT,
                       "notes_contract": "outer-publisher-verbatim-v1"}
        self.save_config()

    def write(self, rel, data):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(data, encoding="utf-8")

    def save_config(self):
        self.write(CONFIG, json.dumps(self.config))

    def cli(self, *args):
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
        env.pop("REQUIRE_PACKAGE", None)
        return subprocess.run([sys.executable, "ci/check-package.py", *args], cwd=self.root,
                              env=env, capture_output=True, text=True, timeout=10)

    def configured(self, *args):
        return self.cli("--product-config", CONFIG, *args)

    def assert_rejected(self, result, text):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(text, result.stdout + result.stderr)
        self.assertNotIn("PRODUCT_CONFIG_RECEIPT=", result.stdout)

    def stage(self, metadata):
        self.write("ipkroot/data/usr/palm/applications/" + APP + "/appinfo.json", json.dumps(metadata))

    def test_size_diagnostic_packager_keeps_normal_install_identity(self):
        # Optimization is not an install flavor. Exercise the real packager with
        # a synthetic payload; this proves metadata, not an executable package.
        self.appinfo.update(vendor="OTTPlay", requiredPermissions=[
            "database.operation", "securitykey.operation"])
        self.write("pkg/appinfo.json", json.dumps(self.appinfo))
        self.stage(self.appinfo)
        (self.root / "pkg/plxnative-storage").write_bytes(
            b"\x7fELF\x01\x01" + bytes(12) + b"\x28\x00")
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", RELEASE="1",
                   FLAVOR="stable", CARGO_PROFILE_RELEASE_OPT_LEVEL="s")
        result = subprocess.run([sys.executable, "ci/mkipk.py"], cwd=self.root,
                                env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((self.root / "pkg" / (APP + "_" + VERSION + "_arm.ipk")).is_file())
        with tarfile.open(self.root / "ipkroot/data.tar.gz", "r:gz") as archive:
            def metadata(name):
                return json.load(archive.extractfile(name))
            app = "usr/palm/applications/" + APP
            for name in (app + "/appinfo.json", app + "/resources/fr/appinfo.json"):
                self.assertEqual(metadata(name)["title"], "OTTPlay")
            self.assertEqual(metadata(app + "/appinfo.json")["id"], APP)
            package = metadata("usr/palm/packages/" + APP + "/packageinfo.json")
            self.assertEqual((package["id"], package["app"], package["loc_name"]),
                             (APP, APP, "OTTPlay"))
            self.assertEqual(package["services"], [APP + ".storage"])
            self.assertEqual(package["version"], VERSION)
            for kind, expected in (("applications", APP), ("packages", APP),
                                   ("services", APP + ".storage")):
                prefix = "usr/palm/" + kind + "/"
                owners = {name[len(prefix):].split("/", 1)[0]
                          for name in archive.getnames() if name.startswith(prefix)}
                self.assertEqual(owners, {expected})
        # The product checker must use the staged normal identity. As with the
        # existing partial-stage fixture, its routing receipt is not full PASS.
        checked = self.configured()
        receipts = [json.loads(line.split("=", 1)[1]) for line in checked.stdout.splitlines()
                    if line.startswith("PRODUCT_CONFIG_RECEIPT=")]
        self.assertEqual(len(receipts), 1, checked.stdout + checked.stderr)
        self.assertEqual((receipts[0]["application_id"], receipts[0]["metadata_source"]),
                         (APP, "staged"))
        self.assertNotEqual(checked.returncode, 0)

    def test_normal_product_rejects_dev_trigger_bytes_independently_of_stamp(self):
        self.stage(self.appinfo)
        for name in ("rust-modules/src/dev.rs", "rust-modules/base/src/devtrig.rs"):
            self.write(name, (ROOT / name).read_text())
        binary = self.root / "ipkroot/data/usr/palm/applications" / APP / "plxnative"
        gate = "the " + APP + " package carries no dev-trigger surface"
        cases = (("--no-default-features", b"synthetic-payload", "ok"),
                 ("--no-default-features", b"synthetic-payload\0plxnative-noidle\0", "FAIL"),
                 ("--no-default-features --features devtriggers",
                  b"synthetic-payload\0plxnative-noidle\0", "FAIL"))
        for flags, payload, verdict in cases:
            with self.subTest(flags=flags, verdict=verdict):
                self.write("pkg/.build-config", "features:" + flags)
                binary.write_bytes(payload)
                checked = self.configured()
                # The deliberately incomplete package fails other checks in all
                # cases. Assert this exact real byte gate, not just a nonzero exit.
                self.assertNotEqual(checked.returncode, 0)
                lines = [line.strip() for line in checked.stdout.splitlines()
                         if line.strip().startswith(("ok — " + gate, "FAIL — " + gate))]
                self.assertEqual(len(lines), 1, checked.stdout + checked.stderr)
                self.assertTrue(lines[0].startswith(verdict + " — " + gate), lines)
                self.assertNotIn("all assertions passed", checked.stdout)

    def test_control_stamp_enables_real_release_byte_gates(self):
        self.stage(self.appinfo)
        for name in ("rust-modules/src/dev.rs", "rust-modules/base/src/devtrig.rs"):
            self.write(name, (ROOT / name).read_text())
        binary = self.root / "ipkroot/data/usr/palm/applications" / APP / "plxnative"
        binary.write_bytes(b"synthetic-payload\0plxnative-noidle\0plxnative@0.4.0-dev\0")
        for control in ("none", "a" * 64):
            with self.subTest(control=control):
                self.write("pkg/.build-config",
                           "features:--no-default-features+symbols+tel:98c4b7d37a4c+control:" + control)
                result = self.configured()
                # This is an intentionally incomplete synthetic package. These
                # failures must be the actual BUILD-dependent byte gates.
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("FAIL — the packaged binary is a RELEASE build", result.stdout)
                self.assertIn("FAIL — the RELEASE binary reports 0.3.0 exactly", result.stdout)
                self.assertNotIn("neither shipped configuration", result.stdout)

    def test_present_bad_stamp_fails_before_configuration_skip(self):
        self.stage(self.appinfo)
        cases = ("", "garbage", "features:+tel:abc+control:" + "a" * 63,
                 "features:+tel:abc+control:none+control:none",
                 "features:+control:none+tel:abc", "features:+tel:abc+future:1")
        for stamp in cases:
            with self.subTest(stamp=stamp):
                self.write("pkg/.build-config", stamp)
                result = self.configured()
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("FAIL — pkg/.build-config has malformed or unsupported build configuration",
                              result.stdout)
                self.assertNotIn("neither shipped configuration", result.stdout)
                self.assertNotIn("Traceback", result.stderr)

    def test_absent_stamp_and_supported_non_shipped_features_remain_explicit_skips(self):
        self.stage(self.appinfo)
        for name in ("rust-modules/src/dev.rs", "rust-modules/base/src/devtrig.rs"):
            self.write(name, (ROOT / name).read_text())
        binary = self.root / "ipkroot/data/usr/palm/applications" / APP / "plxnative"
        binary.write_bytes(b"synthetic-payload")
        for flags in (None, "--features lab-diagnostics",
                      "--no-default-features --features lab-diagnostics",
                      "--no-default-features --features devtriggers"):
            with self.subTest(flags=flags):
                if flags is not None:
                    self.write("pkg/.build-config", "features:" + flags + "+tel:abc+control:none")
                result = self.configured()
                self.assertIn("SKIP — pkg/.build-config is neither shipped configuration", result.stdout)
                self.assertNotIn("malformed or unsupported build configuration", result.stdout)

    def test_tracked_uses_routed_docs_and_receipt_matches_exact_bytes(self):
        result = self.configured("--tracked")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        lines = [s.split("=", 1)[1] for s in result.stdout.splitlines() if s.startswith("PRODUCT_CONFIG_RECEIPT=")]
        self.assertEqual(len(lines), 1)
        receipt = json.loads(lines[0])
        self.assertEqual((receipt["schema"], receipt["product"]), (1, "ottplay-native"))
        self.assertEqual((receipt["application_id"], receipt["version"]), (APP, VERSION))
        self.assertEqual(receipt["notes_contract"], "outer-publisher-verbatim-v1")
        self.assertEqual(receipt["scope"], "release-document-routing-only")
        self.assertEqual(receipt["config_sha256"], hashlib.sha256((self.root / CONFIG).read_bytes()).hexdigest())
        for key, rel in (("notes", NOTES), ("audit", AUDIT)):
            self.assertEqual(receipt[key]["path"], rel)
            self.assertEqual(receipt[key]["sha256"], hashlib.sha256((self.root / rel).read_bytes()).hexdigest())

    def test_standalone_route_requires_its_own_checksum_filename(self):
        self.config["notes_contract"] = "standalone-native-v1"
        self.save_config()
        self.assert_rejected(self.configured("--tracked"), "release document checks failed")
        self.write(NOTES, NOTE_TEXT.replace("ottplay-native-webos-", "ottplay-lg-")
                   + "\nPackage SHA-256: `__IPK_SHA256__`\n")
        result = self.configured("--tracked")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        receipt = next(json.loads(line.split("=", 1)[1]) for line in result.stdout.splitlines()
                       if line.startswith("PRODUCT_CONFIG_RECEIPT="))
        self.assertEqual(receipt["notes_contract"], "standalone-native-v1")
        self.assertEqual(receipt["scope"], "release-document-routing-only")

    def test_standalone_route_rejects_duplicate_or_unknown_checksum_sentinels(self):
        self.config["notes_contract"] = "standalone-native-v1"
        self.save_config()
        for body in (
            NOTE_TEXT.replace("ottplay-native-webos-", "ottplay-lg-")
            + "\n__IPK_SHA256__ __IPK_SHA256__\n",
            NOTE_TEXT.replace("ottplay-native-webos-", "ottplay-lg-")
            + "\n__UNKNOWN_HASH__\n",
        ):
            with self.subTest(body=body):
                self.write(NOTES, body)
                self.assert_rejected(self.configured("--tracked"),
                                     "exactly one package checksum sentinel")

    def test_default_route_keeps_existing_note_contract(self):
        self.write("docs/release-notes/v0.3.0.md", "# Release\n\nChecksum: __IPK_SHA256__\n")
        self.write("docs/release-audits/v0.3.0.md", AUDIT_TEXT)
        result = self.cli("--tracked")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("PRODUCT_CONFIG_RECEIPT=", result.stdout)
        self.write("docs/release-notes/v0.3.0.md", NOTE_TEXT)
        self.assert_rejected(self.cli("--tracked"), "__IPK_SHA256__")

    def test_tracked_mode_ignores_stale_stage_only_when_explicit(self):
        staged = dict(self.appinfo, version="9.0.0")
        self.stage(staged)
        self.assertEqual(self.configured("--tracked").returncode, 0)
        self.assert_rejected(self.configured(), "product config version does not match package metadata")

    def test_staged_identity_cannot_be_vouched_by_tracked_metadata(self):
        self.stage(dict(self.appinfo, id=APP + ".debug"))
        self.assert_rejected(self.configured(), "product config application_id does not match package metadata")

    def test_staged_and_tracked_apply_same_routed_note_checks(self):
        self.write(NOTES, NOTE_TEXT + "\n__UNRESOLVED__\n")
        self.assert_rejected(self.configured("--tracked"), "unresolved placeholder")
        self.stage(self.appinfo)
        self.assert_rejected(self.configured(), "unresolved placeholder")

    def test_unknown_and_conflicting_options_fail_before_any_mode(self):
        for args in (("--unknown",), ("--selftest", "--unknown"), ("--selftest", "--product-config", CONFIG),
                     ("--tracked", "--lint-note", NOTES), ("--product-config",), ("--track",),
                     ("--lint-note", "", "--product-config", CONFIG)):
            with self.subTest(args=args):
                self.assert_rejected(self.cli(*args), "usage:")

    def test_missing_config_never_falls_back_to_default_docs(self):
        (self.root / CONFIG).unlink()
        self.assert_rejected(self.configured("--tracked"), "product config")

    def test_schema_contract_and_identity_fail_closed(self):
        original = dict(self.config)
        for changes in ({"schema": True}, {"schema": 2}, {"product": "different"},
                        {"application_id": APP + ".debug"}, {"version": "0.3.0-beta.1"},
                        {"version": "0.3.1"}, {"extra": 1}, {"notes_contract": "unverified"}):
            with self.subTest(changes=changes):
                self.config = dict(original, **changes)
                self.save_config()
                self.assert_rejected(self.configured("--tracked"), "product config")

    def test_duplicate_keys_and_invalid_utf8_are_rejected(self):
        self.write(CONFIG, json.dumps(self.config)[:-1] + ', "schema": 1}')
        self.assert_rejected(self.configured("--tracked"), "duplicate")
        (self.root / CONFIG).write_bytes(b'\xff')
        self.assert_rejected(self.configured("--tracked"), "product config")

    def test_relative_path_contract_rejects_traversal_and_ambiguous_names(self):
        for path in (str(self.root / CONFIG), "../product.json", "ci//product.json", "ci/./product.json", "ci/../product.json", "ci\\product.json"):
            with self.subTest(path=path):
                self.assert_rejected(self.cli("--product-config", path, "--tracked"), "product config")
        for path in (str(self.root / NOTES), "../notes.md", "ci/../notes.md", "ci/bad name.md", "ci/note.txt", AUDIT):
            with self.subTest(path=path):
                self.config["notes"] = path
                self.save_config()
                self.assert_rejected(self.configured("--tracked"), "product config")

    def test_config_and_document_symlinks_cannot_escape(self):
        target = Path(self.temp.name) / "outside.json"
        target.write_text(json.dumps(self.config))
        (self.root / CONFIG).unlink()
        (self.root / CONFIG).symlink_to(target)
        self.assert_rejected(self.configured("--tracked"), "product config")
        (self.root / CONFIG).unlink()
        self.save_config()
        (self.root / NOTES).unlink()
        (self.root / NOTES).symlink_to(target)
        self.assert_rejected(self.configured("--tracked"), "product config")

    def test_outer_note_requires_verifiable_downloads_without_invented_values(self):
        for body, message in (("# No verification\n", "ottplay-native-webos-0.3.0.sha256"),
                              (NOTE_TEXT + "\n" + "a" * 64 + "\n", "hand-typed"),
                              (NOTE_TEXT + "\n__IPK_SHA256__\n", "unresolved placeholder")):
            with self.subTest(message=message):
                self.write(NOTES, body)
                self.assert_rejected(self.configured("--tracked"), message)

    def test_tracked_cannot_bypass_require_package(self):
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", REQUIRE_PACKAGE="1")
        result = subprocess.run([sys.executable, "ci/check-package.py", "--tracked",
                                 "--product-config", CONFIG], cwd=self.root, env=env,
                                capture_output=True, text=True, timeout=10)
        self.assert_rejected(result, "--tracked conflicts with REQUIRE_PACKAGE=1")

    def test_outer_note_accepts_both_checksum_tools_and_rejects_another_asset(self):
        for command in ("sha256sum -c", "shasum -a 256 -c"):
            with self.subTest(command=command):
                self.write(NOTES, "Verify with `" + command + " ottplay-native-webos-0.3.0.sha256`.\n")
                result = self.configured("--tracked")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for asset in ("SHA256SUMS", "ottplay-native-webos-0.3.1.sha256",
                      "ottplay-native-webos-0.3.0.sha256.bad", "ottplay-native-webos-0.3.0-beta.1.sha256"):
            with self.subTest(asset=asset):
                self.write(NOTES, "Verify with `sha256sum -c " + asset + "`.\n")
                self.assert_rejected(self.configured("--tracked"), "ottplay-native-webos-0.3.0.sha256")

    def test_parent_symlink_and_nonregular_document_are_rejected(self):
        outside = Path(self.temp.name) / "documents"
        outside.mkdir()
        (outside / "note.md").write_text(NOTE_TEXT)
        (self.root / "linked").symlink_to(outside, target_is_directory=True)
        self.config["notes"] = "linked/note.md"
        self.save_config()
        self.assert_rejected(self.configured("--tracked"), "product config")
        self.config["notes"] = "fifo.md"
        self.save_config()
        os.mkfifo(self.root / "fifo.md")
        self.assert_rejected(self.configured("--tracked"), "not a regular file")

    def test_staged_routing_receipt_does_not_claim_a_complete_package_pass(self):
        self.stage(self.appinfo)
        result = self.configured()
        # This deliberately incomplete package cannot pass the existing payload gates.
        self.assertNotEqual(result.returncode, 0)
        lines = [s.split("=", 1)[1] for s in result.stdout.splitlines()
                 if s.startswith("PRODUCT_CONFIG_RECEIPT=")]
        self.assertEqual(len(lines), 1, result.stdout + result.stderr)
        receipt = json.loads(lines[0])
        self.assertEqual(receipt["metadata_source"], "staged")
        self.assertEqual(receipt["scope"], "release-document-routing-only")
        self.assertEqual(receipt["version"], VERSION)
        self.assertNotIn("all assertions passed", result.stdout)

    def test_active_make_gate_passes_explicit_config_as_one_literal_argument(self):
        make = (ROOT / "Makefile").read_text()
        body = make.split("\nipk-pack: release-guard\n", 1)[1].split("\n# THE STABLE", 1)[0]
        lines = [line for line in body.splitlines() if line.startswith("\tpython3 ci/check-package.py")]
        self.assertEqual(len(lines), 1)
        line = lines[0].replace("python3 ci/check-package.py", "python3 argv.py")
        self.write("argv.py", "import json,sys; print('ARGS=' + json.dumps(sys.argv[1:]))\n")
        self.write("gate.mk", "OTT_PRODUCT_CONFIG ?=\n.PHONY: gate\ngate:\n" + line + "\n")
        for value in ("", CONFIG, "bad'; touch injected; #"):
            result = subprocess.run(["make", "-s", "-f", "gate.mk", "gate", "OTT_PRODUCT_CONFIG=" + value],
                                    cwd=self.root, capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            actual = json.loads(next(s[5:] for s in result.stdout.splitlines() if s.startswith("ARGS=")))
            self.assertEqual(actual, ["--product-config", value] if value else [])
            self.assertFalse((self.root / "injected").exists())


class LibraryPayload(unittest.TestCase):
    """Exercise the actual archive checker without constructing a whole application."""
    libraries = {name: b"\x7fELFsynthetic-arm-library" for name in (
        "libavutil-plx.so.61", "libavcodec-plx.so.63", "libavformat-plx.so.63", "libass-plx.so.0")}
    notices = "\n".join("`" + name + "`" for name in libraries)

    def inspect(self, files=None, build="release", extra_entries=(), root=None,
                include_notices=True, notice_content=None, notice_name="THIRD-PARTY-NOTICES.md",
                path_prefix="", extra_files=()):
        # Load the production helper alone: importing check-package.py runs its CLI.
        source = ast.parse((ROOT / "ci/check-package.py").read_text())
        function = next(node for node in source.body
                        if isinstance(node, ast.FunctionDef) and node.name == "check_library_payload")
        results = []
        namespace = {"Path": Path, "re": re, "ROOT": root,
                     "check": lambda condition, message: results.append((bool(condition), message))}
        exec(compile(ast.Module(body=[function], type_ignores=[]), "check-package.py", "exec"), namespace)
        blob = io.BytesIO()
        files = dict(self.libraries if files is None else files)
        if include_notices:
            files[notice_name] = self.notices.encode() if notice_content is None else notice_content
        with tarfile.open(fileobj=blob, mode="w:gz") as archive:
            for name, content in files.items():
                entry = tarfile.TarInfo(path_prefix + "usr/palm/applications/" + APP + "/" + name)
                entry.size = len(content)
                archive.addfile(entry, io.BytesIO(content))
            for name, content in extra_files:
                entry = tarfile.TarInfo(name)
                entry.size = len(content)
                archive.addfile(entry, io.BytesIO(content))
            for entry in extra_entries:
                archive.addfile(entry)
        blob.seek(0)
        with tarfile.open(fileobj=blob, mode="r:gz") as archive:
            namespace["check_library_payload"](archive, self.notices.encode(), build, APP)
        self.assertTrue(results, "the archive must be graded")
        return [message for condition, message in results if not condition]

    def test_mixed_host_and_dev_outputs_do_not_grade_as_shipped(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "pkg").mkdir()
            cached = {"libass-plx-host.so.0": b"\x7fELFhost",
                      "libavutil-plx.61.dylib": b"Mach-O-host",
                      "libswscale-plx.so.10": b"\x7fELFdev"}
            for name, data in cached.items():
                (root / "pkg" / name).write_bytes(data)
            self.assertEqual(self.inspect(root=root), [])
            self.assertEqual({p.name: p.read_bytes() for p in (root / "pkg").iterdir()}, cached)

    def test_real_linux_host_leak_fails_notices_and_host_checks(self):
        failures = self.inspect({**self.libraries, "libass-plx-host.so.0": b"\x7fELFhost"})
        self.assertEqual(len(failures), 2, failures)
        self.assertTrue(any("shipped-not-named=['libass-plx-host.so.0']" in item for item in failures))
        self.assertTrue(any("payload carries no host libraries" in item for item in failures))

    def test_macho_host_leak_is_rejected_even_without_elf_magic(self):
        failures = self.inspect({**self.libraries, "libavcodec-plx.63.dylib": b"Mach-O-host"})
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("payload carries no host libraries", failures[0])

    def test_host_names_cannot_hide_in_nonregular_archive_members(self):
        for kind in (tarfile.SYMTYPE, tarfile.DIRTYPE):
            for name in ("libass-plx-host.so.0", "libass-plx.0.dylib"):
                with self.subTest(kind=kind, name=name):
                    entry = tarfile.TarInfo("usr/palm/applications/" + APP + "/" + name)
                    entry.type = kind
                    if kind == tarfile.SYMTYPE:
                        entry.linkname = "libass-plx.so.0"
                    failures = self.inspect(extra_entries=(entry,))
                    self.assertEqual(len(failures), 2 if kind == tarfile.SYMTYPE else 1, failures)
                    self.assertIn("payload carries no host libraries", failures[-1])

    def test_unknown_shipped_elf_is_not_filtered_out(self):
        failures = self.inspect({**self.libraries, "libsurprise.so.1": b"\x7fELFunknown"})
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("shipped-not-named=['libsurprise.so.1']", failures[0])

    def test_unversioned_unknown_shipped_elf_is_not_filtered_out(self):
        failures = self.inspect({**self.libraries, "libsurprise.so": b"\x7fELFunknown"})
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("shipped-not-named=['libsurprise.so']", failures[0])

    def test_missing_documented_library_fails(self):
        files = {name: data for name, data in self.libraries.items() if name != "libass-plx.so.0"}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "pkg").mkdir()
            (root / "pkg/libass-plx.so.0").write_bytes(self.libraries["libass-plx.so.0"])
            failures = self.inspect(files, root=root)
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("named-not-shipped=['libass-plx.so.0']", failures[0])

    def test_actual_notice_bytes_and_application_path_are_required(self):
        for options in ({"include_notices": False}, {"notice_content": b"stale notices"},
                        {"notice_content": b""}, {"notice_name": "other/THIRD-PARTY-NOTICES.md"}):
            with self.subTest(options=options):
                failures = self.inspect(**options)
                self.assertEqual(len(failures), 1, failures)
                self.assertIn("one byte-identical THIRD-PARTY-NOTICES.md", failures[0])

    def test_duplicate_or_nonregular_notice_cannot_vouch_for_the_source_text(self):
        for kind, prefix in ((tarfile.REGTYPE, ""), (tarfile.REGTYPE, "./"),
                             (tarfile.SYMTYPE, ""), (tarfile.DIRTYPE, "")):
            with self.subTest(kind=kind, prefix=prefix):
                entry = tarfile.TarInfo(prefix + "usr/palm/applications/" + APP + "/THIRD-PARTY-NOTICES.md")
                entry.type = kind
                if kind == tarfile.SYMTYPE:
                    entry.linkname = "other/THIRD-PARTY-NOTICES.md"
                failures = self.inspect(extra_entries=(entry,), include_notices=(kind == tarfile.REGTYPE))
                self.assertEqual(len(failures), 1 if kind == tarfile.DIRTYPE else 2, failures)
                self.assertIn("one byte-identical THIRD-PARTY-NOTICES.md", failures[-1])

    def test_empty_library_payload_does_not_skip_notices(self):
        failures = self.inspect({"appinfo.json": b"{}"})
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("named-not-shipped=", failures[0])

    def test_non_elf_named_file_cannot_satisfy_notices(self):
        failures = self.inspect({**self.libraries, "libass-plx.so.0": b"not-an-ELF"})
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("named-not-shipped=['libass-plx.so.0']", failures[0])

    def test_single_leading_dot_slash_and_normal_directories_are_allowed(self):
        for prefix in ("", "./"):
            with self.subTest(prefix=prefix):
                directories = []
                for path in (".", "usr", "usr/palm", "usr/palm/applications", "usr/palm/applications/" + APP):
                    entry = tarfile.TarInfo(prefix + path if path != "." else ".")
                    entry.type = tarfile.DIRTYPE
                    directories.append(entry)
                self.assertEqual(self.inspect(path_prefix=prefix, extra_entries=directories), [])

    def test_interior_dot_notice_alias_is_rejected(self):
        alias = "usr/palm/applications/" + APP + "/./THIRD-PARTY-NOTICES.md"
        failures = self.inspect(extra_files=((alias, b"tampered notices"),))
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("IPK payload paths are canonical", failures[0])

    def test_repeated_separator_notice_alias_is_rejected(self):
        alias = "usr/palm/applications/" + APP + "//THIRD-PARTY-NOTICES.md"
        failures = self.inspect(extra_files=((alias, b"tampered notices"),))
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("IPK payload paths are canonical", failures[0])

    def test_traversal_absolute_and_repeated_prefix_aliases_are_rejected(self):
        prefix = "usr/palm/applications/" + APP + "/"
        for alias in (prefix + "x/../THIRD-PARTY-NOTICES.md", "/" + prefix + "THIRD-PARTY-NOTICES.md",
                      "././" + prefix + "THIRD-PARTY-NOTICES.md", prefix + "x\\..\\THIRD-PARTY-NOTICES.md"):
            with self.subTest(alias=alias):
                failures = self.inspect(extra_files=((alias, b"tampered notices"),))
                self.assertEqual(len(failures), 1, failures)
                self.assertIn("IPK payload paths are canonical", failures[0])

    def test_duplicate_library_identity_cannot_overwrite_checked_elf(self):
        for prefix in ("", "./"):
            with self.subTest(prefix=prefix):
                alias = prefix + "usr/palm/applications/" + APP + "/libass-plx.so.0"
                failures = self.inspect(extra_files=((alias, b"not an ELF"),))
                self.assertEqual(len(failures), 1, failures)
                self.assertIn("IPK payload paths are canonical", failures[0])

    def test_links_cannot_redirect_a_canonical_notice_path(self):
        prefix = "usr/palm/applications/" + APP + "/"
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
            with self.subTest(kind=kind):
                entry = tarfile.TarInfo(prefix + "redirect")
                entry.type = kind
                entry.linkname = "."
                failures = self.inspect(extra_entries=(entry,),
                                        extra_files=((prefix + "redirect/THIRD-PARTY-NOTICES.md", b"tampered"),))
                self.assertEqual(len(failures), 1, failures)
                self.assertIn("IPK payload paths are canonical", failures[0])

    def test_dev_scaler_exception_does_not_weaken_release(self):
        files = {**self.libraries, "libswscale-plx.so.10": b"\x7fELFdev"}
        self.assertEqual(self.inspect(files, build="dev"), [])
        failures = self.inspect(files, build="release")
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("a RELEASE build ships none of the dev-only libraries", failures[0])


if __name__ == "__main__":
    unittest.main()
