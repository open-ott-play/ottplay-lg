"""Focused tests for the native-candidate provenance boundary."""
from __future__ import annotations

import importlib.util
import hashlib
import json
import unittest
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).with_name("make-native-candidate-receipt.py")
SPEC = importlib.util.spec_from_file_location("native_candidate_receipt", SCRIPT)
assert SPEC and SPEC.loader
receipt = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(receipt)

VERIFY_SCRIPT = Path(__file__).with_name("verify-native-candidate.py")
VERIFY_SPEC = importlib.util.spec_from_file_location("verify_native_candidate", VERIFY_SCRIPT)
assert VERIFY_SPEC and VERIFY_SPEC.loader
verify_candidate = importlib.util.module_from_spec(VERIFY_SPEC)
VERIFY_SPEC.loader.exec_module(verify_candidate)


class CandidateReceiptTests(unittest.TestCase):
    def test_source_sha_requires_full_lowercase_commit(self):
        self.assertTrue(receipt.valid_source_sha("a" * 40))
        for value in ("a" * 39, "a" * 41, "A" * 40, "g" * 40):
            with self.subTest(value=value):
                self.assertFalse(receipt.valid_source_sha(value))

    def test_ipk_name_is_stable_identity_and_version(self):
        expected = receipt.expected_ipk_name()
        self.assertTrue(expected.startswith("com.openottplay.player.native_"))
        self.assertTrue(expected.endswith("_arm.ipk"))

    def test_release_stamp_accepts_release_build_fields(self):
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            stamp = Path(directory) / ".build-config"
            stamp.write_text("features:--no-default-features+tel:abc123+control:none\n")
            self.assertEqual(receipt.validate_release_stamp(stamp),
                             "features:--no-default-features+tel:abc123+control:none")

    def test_release_stamp_rejects_debug_and_rc_builds(self):
        import tempfile

        with tempfile.TemporaryDirectory() as directory:
            stamp = Path(directory) / ".build-config"
            for value in ("features:", "features:--no-default-features+profile:tvdev",
                          "features:--no-default-features+rc:1"):
                with self.subTest(value=value):
                    stamp.write_text(value)
                    with self.assertRaises(ValueError):
                        receipt.validate_release_stamp(stamp)

    def test_candidate_verifier_rejects_changed_package_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source_sha = "a" * 40
            run_id, attempt, version = "1234", "2", "0.3.0"
            filename = f"com.openottplay.player.native_{version}_arm.ipk"
            package = root / filename
            package.write_bytes(b"candidate-package")
            package_hash = hashlib.sha256(package.read_bytes()).hexdigest()
            sidecar = f"{package_hash}  {filename}\n"
            (root / (filename + ".sha256")).write_text(sidecar)
            (root / f"ottplay-lg-{version}.sha256").write_text(sidecar)
            evidence = root / "link-evidence"
            evidence.mkdir()
            link_file = evidence / "plxnative.link.json"
            link_file.write_text("{}\n")
            evidence_hash = hashlib.sha256(link_file.read_bytes()).hexdigest()
            data = {
                "schema": 1,
                "artifact_kind": "native-webos-stable-candidate",
                "candidate": {"run_id": run_id, "run_attempt": attempt},
                "source": {"commit": source_sha},
                "package": {"filename": filename, "app_id": "com.openottplay.player.native",
                            "version": version, "sha256": package_hash,
                            "size_bytes": package.stat().st_size},
                "link_evidence_sha256": {"plxnative.link.json": evidence_hash},
            }
            path = root / "candidate-provenance.json"
            path.write_text(json.dumps(data))
            (root / "candidate-provenance.json.sha256").write_text(
                f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n")
            verified, _ = verify_candidate.verify(root, source_sha=source_sha, run_id=run_id,
                                                   run_attempt=attempt, version=version)
            self.assertEqual(verified, package.resolve())

            package.write_bytes(b"candidate-packagE")
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                verify_candidate.verify(root, source_sha=source_sha, run_id=run_id,
                                        run_attempt=attempt, version=version)


if __name__ == "__main__":
    unittest.main()
