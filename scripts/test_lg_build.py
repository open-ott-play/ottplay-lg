#!/usr/bin/env python3
"""Exercise source/build admission and candidate packaging without compilation."""
import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
LOADER = importlib.util.spec_from_file_location("lg_build", ROOT / "scripts/lg-build.py")
build = importlib.util.module_from_spec(LOADER)
LOADER.loader.exec_module(build)


class SourceAdmission(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve() / "repository"
        self.root.mkdir()
        self.args = argparse.Namespace(work_dir=Path(self.temporary.name) / "output")
        for relative, content in {
            "rust-modules/Cargo.toml": "[workspace]\n",
            ".gitignore": "local.env\nbuild/\n",
            build.PRODUCT_CONFIG: json.dumps({"notes": "notes.md", "audit": "audit.md"}),
            "notes.md": "Candidate only.\n", "audit.md": "Not executed.\n",
        }.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        self.git("init", "-q")
        self.git("add", ".")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "fixture")
        self.patch = mock.patch.object(build, "ROOT", self.root)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, text=True).strip()

    def test_receipt_names_new_repository_and_exact_commit(self):
        record = build.prepare(self.args, {})
        self.assertEqual(record["repository"], "https://github.com/open-ott-play/ottplay-lg")
        self.assertEqual(record["revision"], self.git("rev-parse", "HEAD"))
        self.assertEqual(record["tree"], self.git("rev-parse", "HEAD^{tree}"))
        self.assertEqual(build.check_source(self.args, {}), self.root)
        self.assertFalse((self.args.work_dir / "source").exists())

    def test_changed_tracked_and_untracked_inputs_are_rejected(self):
        build.prepare(self.args, {})
        for relative in ("notes.md", "untracked-build-input"):
            path = self.root / relative
            original = path.read_bytes() if path.exists() else None
            path.write_text("changed")
            with self.assertRaisesRegex(ValueError, "committed clean"):
                build.check_source(self.args, {})
            if original is None:
                path.unlink()
            else:
                path.write_bytes(original)

    def test_ignored_private_configuration_is_rejected(self):
        (self.root / "local.env").write_text("synthetic configuration")
        self.assertEqual(self.git("status", "--porcelain"), "")
        with self.assertRaisesRegex(ValueError, "private or excluded"):
            build.prepare(self.args, {})

    def test_build_directory_cannot_silently_change_revision(self):
        build.prepare(self.args, {})
        (self.root / "notes.md").write_text("Another candidate.\n")
        self.git("add", "notes.md")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "next fixture")
        with self.assertRaisesRegex(ValueError, "another source revision"):
            build.prepare(self.args, {})


class BuildAdmission(unittest.TestCase):
    def args(self, **values):
        return argparse.Namespace(work_dir=Path("/tmp/lg-build-test"), sdk=Path("/tmp/lg-sdk-test"),
                                  jobs=2, rust_opt_level="s", control_discovery_url=None,
                                  control_discovery_sha256=None, **values)

    def test_ambient_development_and_compiler_flags_do_not_enter_build(self):
        with mock.patch.dict(os.environ, {"PLX_CONTROL_DISCOVERY_URL": "unexpected", "FLAVOR": "debug",
                                          "RELEASE": "", "CARGO_TARGET_DIR": "/tmp/other",
                                          "CC": "/tmp/wrong-compiler", "AR": "/tmp/wrong-archiver",
                                          "CC_armv7_unknown_linux_gnueabi": "/tmp/incorrect-target-cc",
                                          "AR_armv7-unknown-linux-gnueabi": "/tmp/incorrect-target-ar",
                                          "TARGET_CFLAGS": "unreviewed-flags", "HOST_CC": "/tmp/incorrect-host-cc",
                                          "BINDGEN_EXTRA_CLANG_ARGS": "unreviewed-clang-flags",
                                          "BINDGEN_EXTRA_CLANG_ARGS_armv7_unknown_linux_gnueabi": "incorrect-target",
                                          "LIBCLANG_PATH": "/tmp/unreviewed-libclang", "LLVM_CONFIG_PATH": "/tmp/wrong-llvm",
                                          "RUSTFLAGS": "--cfg unsafe_fixture", "MAKEFLAGS": "bad",
                                          "CARGO_PROFILE_RELEASE_OPT_LEVEL": "0", "JOBS": "64"}):
            environment = build.build_env(self.args())
        self.assertEqual(environment["FLAVOR"], "stable")
        self.assertEqual(environment["RELEASE"], "1")
        self.assertEqual(environment["CARGO_PROFILE_RELEASE_OPT_LEVEL"], "s")
        self.assertEqual(environment["CARGO_BUILD_JOBS"], "2")
        self.assertEqual(environment["JOBS"], "2")
        for key in ("PLX_CONTROL_DISCOVERY_URL", "RUSTFLAGS", "MAKEFLAGS", "CARGO_TARGET_DIR",
                    "CC", "AR", "CC_armv7_unknown_linux_gnueabi", "AR_armv7-unknown-linux-gnueabi",
                    "TARGET_CFLAGS", "HOST_CC", "BINDGEN_EXTRA_CLANG_ARGS",
                    "BINDGEN_EXTRA_CLANG_ARGS_armv7_unknown_linux_gnueabi", "LIBCLANG_PATH", "LLVM_CONFIG_PATH"):
            self.assertNotIn(key, environment)

    def test_libclang_selection_keeps_discoverable_alias_and_rejects_cpp(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            real = root / "real-library-bytes"
            real.write_bytes(b"synthetic library")
            alias = root / "libclang.dylib"
            alias.symlink_to(real)
            with mock.patch.object(build.platform, "system", return_value="Darwin"), mock.patch.object(build, "probe_libclang") as probe:
                self.assertEqual(build.find_libclang(alias), alias)
                probe.assert_called_once_with(real)
                invalid = root / "libclang-cpp.dylib"
                invalid.write_bytes(b"unsupported library")
                with self.assertRaisesRegex(ValueError, "not discoverable"):
                    build.find_libclang(invalid)

    def test_missing_explicit_libclang_does_not_fall_back_to_another_library(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(ValueError, "Host libclang is required"):
                build.find_libclang(Path(temporary) / "absent")

    def test_companion_requires_paired_explicit_inputs(self):
        with self.assertRaisesRegex(ValueError, "both --companion"):
            build.main(["build", "--companion-bundle", "/missing"])

    def test_isolated_cargo_home_is_stable_and_refuses_local_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            args = self.args()
            args.work_dir = root / "output"
            with mock.patch.dict(os.environ, {"CARGO_HOME": str(root / "cache")}):
                with build.cargo_environment(args) as first:
                    home = Path(first["CARGO_HOME"])
                    self.assertTrue((home / "registry").is_symlink())
                with build.cargo_environment(args) as second:
                    self.assertEqual(second["CARGO_HOME"], str(home))
                (home / "config.toml").write_text("[build]\nrustflags = ['unreviewed']\n")
                with self.assertRaisesRegex(ValueError, "configuration or credentials"):
                    with build.cargo_environment(args):
                        self.fail("unreviewed configuration must not be used")

    def test_changed_external_companion_verifier_never_executes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            (root / "ci").mkdir()
            (root / "ci/companion-source.json").write_text(json.dumps({"verifier_files": {
                "build.py": hashlib.sha256(b"reviewed").hexdigest()}}))
            (root / "build.py").write_text("raise AssertionError('must not execute')")
            with mock.patch.object(build, "ROOT", root):
                with self.assertRaisesRegex(ValueError, "does not match"):
                    build.companion_builder(self.args(companion_source=root, companion_bundle=root))

    def test_real_cargo_artifact_rejects_development_features(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            target = root / "rust-modules/target-release"
            archive = target / build.TARGET / "release/libplxnative_modules.a"
            archive.parent.mkdir(parents=True)
            archive.write_bytes(b"synthetic archive")
            artifact = {"reason": "compiler-artifact", "manifest_path": str(root / "rust-modules/Cargo.toml"),
                        "target": {"name": "plxnative_modules", "src_path": str(root / "rust-modules/src/lib.rs"),
                                   "kind": ["staticlib"], "crate_types": ["staticlib"]},
                        "features": ["devtriggers"], "filenames": [str(archive)],
                        "profile": {"opt_level": "s", "debuginfo": 0, "debug_assertions": False,
                                    "overflow_checks": False, "test": False}}
            raw = "\n".join(json.dumps(row) for row in (artifact, {"reason": "build-finished", "success": True}))
            (target / ".lib-artifacts.json").write_text(raw)
            with self.assertRaisesRegex(ValueError, "selected release configuration"):
                build.native_compile_evidence(self.args(), build.manifest(), root, "synthetic")

    def test_standalone_check_discovers_native_libraries_and_avoids_old_workflow_gates(self):
        metadata = {"packages": [
            {"name": "plxnative-modules", "targets": [{"kind": ["rlib"]}]},
            {"name": "plx_shared_core", "targets": [{"kind": ["lib"]}]},
            {"name": "plxnative-storage", "targets": [{"kind": ["bin"]}]},
        ]}
        environment = {"CARGO_BUILD_JOBS": "2"}
        with mock.patch.object(build, "run", return_value=json.dumps(metadata)) as reader:
            commands = [command for _, command in build.check_commands(self.args(), environment)]
        self.assertEqual(reader.call_args.kwargs["env"], environment)
        self.assertTrue(any("scripts/check-shared-core.cjs" in command for command in commands))
        units = [command for command in commands if "--lib" in command and "test" in command]
        self.assertEqual(len(units), 2)
        self.assertTrue(all("plx_shared_core" in command and "plxnative-modules" in command for command in units))
        self.assertTrue(any("hostsim" in command for command in units))
        self.assertTrue(any("--no-default-features" in command for command in commands))
        self.assertTrue(all("make" not in command for command in commands))
        self.assertFalse(any("test_ci_workflows.py" in str(command) for command in commands))


def tar_bytes(files):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, data in files.items():
            member = tarfile.TarInfo(name)
            member.mode, member.size = 0o644, len(data)
            archive.addfile(member, io.BytesIO(data))
    return buffer.getvalue()


def ar_bytes(files):
    result = b"!<arch>\n"
    for name, data in files.items():
        header = (f"{name + '/':<16}{0:<12}{0:<6}{0:<6}{'100644':<8}{len(data):<10}`\n").encode()
        result += header + data + (b"\n" if len(data) % 2 else b"")
    return result


class PackageAdmission(unittest.TestCase):
    def setUp(self):
        self.spec = build.manifest()
        app = self.spec["application"]
        self.prefix = "usr/palm/applications/" + app["id"] + "/"
        service = app["id"] + ".storage"
        elf = b"\x7fELF\x01\x01" + bytes(12) + b"\x28\x00" + bytes(32)
        self.files = {
            self.prefix + "appinfo.json": json.dumps({**app, "type": "native", "main": "plxnative"}).encode(),
            self.prefix + "plxnative": elf,
            self.prefix + "licenses/LICENSE": (ROOT / "LICENSE").read_bytes(),
            self.prefix + "THIRD-PARTY-NOTICES.md": b"Synthetic package fixture only.",
            self.prefix + "libavformat-plx.so": b"synthetic library",
            "usr/palm/services/" + service + "/plxnative-storage": elf,
            "usr/palm/services/" + service + "/services.json": json.dumps({
                "id": service, "engine": "native", "executable": "plxnative-storage",
                "services": [{"name": service, "commands": []}]}).encode(),
            "usr/palm/packages/" + app["id"] + "/packageinfo.json": json.dumps({
                "id": app["id"], "app": app["id"], "version": app["version"], "services": [service],
                "package_format_version": 2, "requiredPermissions": ["database.operation", "securitykey.operation"]}).encode(),
        }
        self.control = ("Package: " + app["id"] + "\nVersion: " + app["version"]
                        + "\nArchitecture: arm\nWebOS-Package-Format-Version: 2\nLicense: GPL-3.0-or-later\n").encode()

    def verify(self):
        with tempfile.TemporaryDirectory() as temporary:
            package = Path(temporary) / "synthetic.ipk"
            package.write_bytes(ar_bytes({"debian-binary": b"2.0\n", "control.tar.gz": tar_bytes({"control": self.control}),
                                          "data.tar.gz": tar_bytes(self.files)}))
            return build.verify_ipk(package, self.spec)

    def test_generic_package_does_not_advertise_companion(self):
        result = self.verify()
        self.assertNotIn("companion", result)

    def test_license_bytes_must_match_new_repository(self):
        self.files[self.prefix + "licenses/LICENSE"] = b"missing full license"
        with self.assertRaisesRegex(ValueError, "GPL license"):
            self.verify()

    def test_bare_helper_cannot_bypass_source_evidence(self):
        self.files[self.prefix + "bin/ottplay-companion"] = b"unverified helper"
        with self.assertRaisesRegex(ValueError, "Incomplete native companion"):
            self.verify()

    def test_private_state_is_rejected_from_package(self):
        self.files[self.prefix + "ott-state.json"] = b"{}"
        with self.assertRaisesRegex(ValueError, "Private profiles"):
            self.verify()


if __name__ == "__main__":
    unittest.main()
