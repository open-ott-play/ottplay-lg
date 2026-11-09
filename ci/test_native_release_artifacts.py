"""Tests for RC release asset inventory, provenance, and safe evidence extraction."""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

from ci import native_release as plans
from ci import native_release_artifacts as artifacts


VERSION = "0.3.4"
TAG = "v0.3.4-rc.1"
REPOSITORY = "open-ott-play/ottplay-lg"
SOURCE_SHA = "0123456789abcdef0123456789abcdef01234567"
RUN_ID = "1234"
RUN_ATTEMPT = "2"
PACKAGE = f"com.openottplay.player.native_{VERSION}_arm.ipk"
EVIDENCE = {
    "plxnative.link.json": b'{"symbols":[]}',
    "nested/transport.link.json": b'{"symbols":["one"]}',
}
ROOT = Path(__file__).resolve().parents[1]


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_tar(path: Path, entries: list[tuple[str, bytes | None, bytes | None]]) -> None:
    """Entries are (name, payload, tar type); None type means regular file."""
    with tarfile.open(path, "w:gz") as archive:
        for name, payload, kind in entries:
            member = tarfile.TarInfo(name)
            if kind is not None:
                member.type = kind
                if kind == tarfile.SYMTYPE:
                    member.linkname = payload.decode() if payload else "target"
                archive.addfile(member)
            else:
                assert payload is not None
                member.size = len(payload)
                import io
                archive.addfile(member, io.BytesIO(payload))


def fixture(root: Path) -> dict[str, Path]:
    root.mkdir(parents=True, exist_ok=True)
    package_bytes = b"verified native IPK fixture"
    (root / PACKAGE).write_bytes(package_bytes)
    package_hash = digest(package_bytes)
    checksum = f"{package_hash}  {PACKAGE}\n"
    (root / f"{PACKAGE}.sha256").write_text(checksum, encoding="ascii")
    (root / f"ottplay-lg-{VERSION}.sha256").write_text(checksum, encoding="ascii")

    evidence_map = {name: digest(content) for name, content in EVIDENCE.items()}
    receipt = {
        "schema": 1,
        "artifact_kind": "native-webos-stable-candidate",
        "candidate": {"run_id": RUN_ID, "run_attempt": RUN_ATTEMPT},
        "source": {"repository": "https://github.com/open-ott-play/ottplay-lg.git",
                   "commit": SOURCE_SHA, "tree": "b" * 40},
        "package": {"filename": PACKAGE, "app_id": "com.openottplay.player.native",
                    "version": VERSION, "sha256": package_hash, "size_bytes": len(package_bytes),
                    "build_config": "features:--no-default-features",
                    "build_config_sha256": "c" * 64},
        "toolchain": {
            "runner": "ubuntu-24.04-arm", "webos_ndk_version": "2.27.0",
            "webos_ndk_asset": "webos-ndk.tar.gz", "webos_ndk_archive_sha256": "d" * 64,
            "webos_gcc_sha256": "e" * 64, "webos_gcc_backend_sha256": "f" * 64,
            "rust_toolchain_pin": "1.90.0", "rustc_version": "rustc 1.90.0",
            "cargo_version": "cargo 1.90.0", "cargo_lock_sha256": "1" * 64,
            "dependency_manifest_sha256": "2" * 64, "makefile_sha256": "3" * 64,
        },
        "link_evidence_sha256": evidence_map,
        "checks": ["ci/deps.py check", "make FLAVOR=stable RELEASE=1 ipk"],
    }
    receipt_path = root / "candidate-provenance.json"
    receipt_path.write_text(json.dumps(receipt, sort_keys=True) + "\n", encoding="utf-8")
    (root / "candidate-provenance.json.sha256").write_text(
        f"{digest(receipt_path.read_bytes())}  candidate-provenance.json\n", encoding="ascii")

    plan = plans.create_plan(repository=REPOSITORY, channel="rc", version=VERSION, sequence=1,
                             source_sha=SOURCE_SHA, run_id=int(RUN_ID), run_attempt=int(RUN_ATTEMPT))
    (root / "native-release-plan.json").write_text(json.dumps(plan, sort_keys=True) + "\n", encoding="utf-8")

    archive_entries = [("link-evidence/", None, tarfile.DIRTYPE)]
    for name, content in EVIDENCE.items():
        archive_entries.append((f"link-evidence/{name}", content, None))
    write_tar(root / "link-evidence.tar.gz", archive_entries)
    return {path.name: path for path in root.iterdir()}


def original_candidate(release_root: Path, candidate_root: Path) -> Path:
    candidate_root.mkdir(parents=True)
    for name in artifacts.expected_inventory(VERSION) - {"link-evidence.tar.gz"}:
        shutil.copyfile(release_root / name, candidate_root / name)
    evidence = candidate_root / "link-evidence"
    for name, content in EVIDENCE.items():
        target = evidence / Path(*name.split("/"))
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
    return candidate_root


class VerifyRcAssetTests(unittest.TestCase):
    def verify(self, root: Path) -> dict:
        return artifacts.verify_rc(root, version=VERSION, tag=TAG, repository=REPOSITORY)

    def test_valid_rc_assets_return_exact_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture(Path(directory))
            self.assertEqual(self.verify(Path(directory)), {
                "source_sha": SOURCE_SHA, "run_id": 1234, "run_attempt": 2,
                "version": VERSION, "ipk_sha256": digest(b"verified native IPK fixture"),
            })

    def test_original_run_candidate_artifact_matches_release_assets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "release-assets"
            fixture(root)
            candidate = original_candidate(root, Path(directory) / "candidate-run-artifact")
            self.assertEqual(
                artifacts.verify_rc(root, version=VERSION, tag=TAG, repository=REPOSITORY,
                                    candidate_artifact_dir=candidate)["ipk_sha256"],
                digest(b"verified native IPK fixture"),
            )

    def test_consistently_replaced_release_package_and_receipt_fail_against_original_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "release-assets"
            fixture(root)
            candidate = original_candidate(root, Path(directory) / "candidate-run-artifact")

            # Replace the package and every checksum-bearing release sidecar consistently.
            replacement = b"replaced native IPK fixture"
            self.assertEqual(len(replacement), len(b"verified native IPK fixture"))
            package = root / PACKAGE
            package.write_bytes(replacement)
            replacement_digest = digest(replacement)
            sidecar = f"{replacement_digest}  {PACKAGE}\n"
            (root / f"{PACKAGE}.sha256").write_text(sidecar, encoding="ascii")
            (root / f"ottplay-lg-{VERSION}.sha256").write_text(sidecar, encoding="ascii")
            receipt_path = root / "candidate-provenance.json"
            receipt = json.loads(receipt_path.read_text())
            receipt["package"]["sha256"] = replacement_digest
            receipt_path.write_text(json.dumps(receipt, sort_keys=True) + "\n", encoding="utf-8")
            (root / "candidate-provenance.json.sha256").write_text(
                f"{digest(receipt_path.read_bytes())}  candidate-provenance.json\n", encoding="ascii")

            # Release-only validation is internally consistent, but it is not the run artifact.
            self.assertEqual(self.verify(root)["ipk_sha256"], replacement_digest)
            with self.assertRaisesRegex(artifacts.ArtifactError, "differs from downloaded release asset"):
                artifacts.verify_rc(root, version=VERSION, tag=TAG, repository=REPOSITORY,
                                    candidate_artifact_dir=candidate)

    def test_candidate_artifact_requires_exact_inventory_and_evidence_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "release-assets"
            fixture(root)
            candidate = original_candidate(root, Path(directory) / "candidate-run-artifact")
            (candidate / "extra.txt").write_text("unexpected")
            with self.assertRaisesRegex(artifacts.ArtifactError, "candidate artifact has unexpected assets"):
                artifacts.verify_rc(root, version=VERSION, tag=TAG, repository=REPOSITORY,
                                    candidate_artifact_dir=candidate)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "release-assets"
            fixture(root)
            candidate = original_candidate(root, Path(directory) / "candidate-run-artifact")
            (candidate / "link-evidence" / "nested" / "transport.link.json").write_bytes(b"altered")
            with self.assertRaisesRegex(artifacts.ArtifactError, "candidate evidence checksum mismatch"):
                artifacts.verify_rc(root, version=VERSION, tag=TAG, repository=REPOSITORY,
                                    candidate_artifact_dir=candidate)

    def test_tag_channel_version_and_repository_are_bound_to_plan(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            with self.assertRaisesRegex(artifacts.ArtifactError, "requested RC tag"):
                artifacts.verify_rc(root, version=VERSION, tag="v0.3.4-rc.2", repository=REPOSITORY)
            with self.assertRaisesRegex(artifacts.ArtifactError, "expected repository"):
                artifacts.verify_rc(root, version=VERSION, tag=TAG, repository="other/repo")

            plan_path = root / "native-release-plan.json"
            plan = json.loads(plan_path.read_text())
            plan["channel"] = "beta"
            plan["tag"] = "v0.3.4-beta.1"
            plan_path.write_text(json.dumps(plan), encoding="utf-8")
            with self.assertRaisesRegex(artifacts.ArtifactError, "channel must be rc"):
                self.verify(root)

    def test_plan_source_and_run_identity_must_match_receipt(self):
        for key, value, error in (("source_sha", "f" * 40, "source SHA"),
                                  ("run_id", 9999, "run id"), ("run_attempt", 1, "run attempt")):
            with self.subTest(key=key), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                fixture(root)
                plan_path = root / "native-release-plan.json"
                plan = json.loads(plan_path.read_text())
                plan[key] = value
                plan_path.write_text(json.dumps(plan), encoding="utf-8")
                with self.assertRaisesRegex(artifacts.ArtifactError, error):
                    self.verify(root)

    def test_package_bytes_and_both_checksum_sidecars_are_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            (root / PACKAGE).write_bytes(b"tampered native IPK fixture")
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                self.verify(root)
        for name in (f"{PACKAGE}.sha256", f"ottplay-lg-{VERSION}.sha256"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                fixture(root)
                (root / name).write_text("0" * 64 + "  wrong.ipk\n", encoding="ascii")
                with self.assertRaisesRegex(ValueError, "checksum|asset"):
                    self.verify(root)

    def test_receipt_sidecar_and_receipt_content_are_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            (root / "candidate-provenance.json.sha256").write_text("0" * 64 + "  candidate-provenance.json\n")
            with self.assertRaisesRegex(artifacts.ArtifactError, "receipt checksum"):
                self.verify(root)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            receipt_path = root / "candidate-provenance.json"
            receipt = json.loads(receipt_path.read_text())
            receipt["unknown"] = "not allowed"
            receipt_path.write_text(json.dumps(receipt), encoding="utf-8")
            (root / "candidate-provenance.json.sha256").write_text(
                f"{digest(receipt_path.read_bytes())}  candidate-provenance.json\n", encoding="ascii")
            with self.assertRaisesRegex(artifacts.ArtifactError, "unknown fields"):
                self.verify(root)

    def test_duplicate_json_markers_are_rejected_even_with_updated_sidecar(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            receipt_path = root / "candidate-provenance.json"
            receipt_path.write_text('{"schema":1,"schema":1}', encoding="utf-8")
            (root / "candidate-provenance.json.sha256").write_text(
                f"{digest(receipt_path.read_bytes())}  candidate-provenance.json\n", encoding="ascii")
            with self.assertRaisesRegex(artifacts.ArtifactError, "duplicate JSON field"):
                self.verify(root)

    def test_evidence_archive_digest_must_match_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            write_tar(root / "link-evidence.tar.gz", [("link-evidence/plxnative.link.json", b"different", None),
                                                       ("link-evidence/nested/transport.link.json", EVIDENCE["nested/transport.link.json"], None)])
            with self.assertRaisesRegex(ValueError, "link evidence checksum"):
                self.verify(root)

    def test_evidence_archive_requires_exact_file_inventory(self):
        cases = (
            [("link-evidence/plxnative.link.json", EVIDENCE["plxnative.link.json"], None)],
            [("link-evidence/plxnative.link.json", EVIDENCE["plxnative.link.json"], None),
             ("link-evidence/nested/transport.link.json", EVIDENCE["nested/transport.link.json"], None),
             ("link-evidence/extra.link.json", b"{}", None)],
        )
        for archive_entries in cases:
            with self.subTest(archive_entries=archive_entries), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                fixture(root)
                write_tar(root / "link-evidence.tar.gz", archive_entries)
                with self.assertRaisesRegex(artifacts.ArtifactError, "inventory does not match"):
                    self.verify(root)

    def test_archive_rejects_traversal_absolute_unexpected_root_and_links(self):
        cases = (
            [("link-evidence/../escape.link.json", b"{}", None)],
            [("/link-evidence/plxnative.link.json", b"{}", None)],
            [("other-root/plxnative.link.json", b"{}", None)],
            [("link-evidence/plxnative.link.json", b"target", tarfile.SYMTYPE)],
            [("link-evidence/plxnative.link.json", b"", tarfile.FIFOTYPE)],
        )
        for entries in cases:
            with self.subTest(entries=entries), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                fixture(root)
                write_tar(root / "link-evidence.tar.gz", entries)
                with self.assertRaises(artifacts.ArtifactError):
                    self.verify(root)

    def test_archive_rejects_duplicate_members_and_unexpected_empty_directories(self):
        cases = (
            [("link-evidence/plxnative.link.json", EVIDENCE["plxnative.link.json"], None),
             ("link-evidence/plxnative.link.json", EVIDENCE["plxnative.link.json"], None),
             ("link-evidence/nested/transport.link.json", EVIDENCE["nested/transport.link.json"], None)],
            [("link-evidence/plxnative.link.json", EVIDENCE["plxnative.link.json"], None),
             ("link-evidence/nested/transport.link.json", EVIDENCE["nested/transport.link.json"], None),
             ("link-evidence/unexpected/", None, tarfile.DIRTYPE)],
        )
        for entries in cases:
            with self.subTest(entries=entries), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                fixture(root)
                write_tar(root / "link-evidence.tar.gz", entries)
                with self.assertRaises(artifacts.ArtifactError):
                    self.verify(root)

    def test_download_directory_requires_exact_seven_asset_inventory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            (root / "unexpected.txt").write_text("extra")
            with self.assertRaisesRegex(artifacts.ArtifactError, "unexpected assets"):
                self.verify(root)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            (root / "native-release-plan.json").unlink()
            with self.assertRaisesRegex(artifacts.ArtifactError, "missing assets"):
                self.verify(root)

    def test_root_symlink_asset_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture(root)
            package = root / PACKAGE
            saved = root.parent / f"{root.name}-saved-ipk"
            package.rename(saved)
            package.symlink_to(saved)
            with self.assertRaisesRegex(artifacts.ArtifactError, "regular non-symlink"):
                self.verify(root)

    def test_identity_output_refuses_overwrite(self):
        identity = {"source_sha": SOURCE_SHA, "run_id": 1, "run_attempt": 1,
                    "version": VERSION, "ipk_sha256": "a" * 64}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "identity.json"
            artifacts.write_identity(identity, output)
            self.assertEqual(json.loads(output.read_text()), identity)
            with self.assertRaisesRegex(artifacts.ArtifactError, "refusing to overwrite"):
                artifacts.write_identity(identity, output)

    def test_verify_rc_cli_writes_strict_identity_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "assets"
            fixture(root)
            candidate = original_candidate(root, Path(directory) / "candidate-run-artifact")
            output = Path(directory) / "verified-identity.json"
            command = [sys.executable, str(ROOT / "ci/native_release_artifacts.py"), "verify-rc",
                       "--artifact-dir", str(root), "--version", VERSION, "--tag", TAG,
                       "--repository", REPOSITORY, "--candidate-artifact-dir", str(candidate),
                       "--output-json", str(output)]
            first = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
            self.assertEqual(first.returncode, 0, first.stderr)
            expected = {"source_sha": SOURCE_SHA, "run_id": 1234, "run_attempt": 2,
                        "version": VERSION, "ipk_sha256": digest(b"verified native IPK fixture")}
            self.assertEqual(json.loads(output.read_text()), expected)
            self.assertEqual(json.loads(first.stdout), expected)
            second = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
            self.assertNotEqual(second.returncode, 0)
            self.assertIn("refusing to overwrite", second.stderr)




class CompanionInventoryTests(unittest.TestCase):
    def test_source_assets_are_required_only_for_companion_receipts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture(root)
            self.assertEqual(set(artifacts._inventory_root(root, VERSION)), artifacts.expected_inventory(VERSION))
            path = root / "candidate-provenance.json"
            receipt = json.loads(path.read_text())
            receipt["companion"] = {"metadata": {}, "source_revision": "a" * 40,
                "source_assets": {name: "b" * 64 for name in artifacts.COMPANION_SOURCE_ASSETS}}
            path.write_text(json.dumps(receipt))
            artifacts._validate_receipt(receipt, VERSION)
            with self.assertRaisesRegex(artifacts.ArtifactError, "missing assets"):
                artifacts._inventory_root(root, VERSION)
            for name in artifacts.COMPANION_SOURCE_ASSETS:
                (root / name).write_bytes(b"source fixture")
            self.assertEqual(set(artifacts._inventory_root(root, VERSION)),
                artifacts.expected_inventory(VERSION) | artifacts.COMPANION_SOURCE_ASSETS)
            del receipt["companion"]["source_assets"]["companion-source.tar.gz"]
            with self.assertRaisesRegex(artifacts.ArtifactError, "missing fields"):
                artifacts._validate_receipt(receipt, VERSION)


if __name__ == "__main__":
    unittest.main()
