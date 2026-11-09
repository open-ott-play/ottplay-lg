"""Stable-only source coverage must be complete and remain bound to RC bytes."""
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from ci import native_release_artifacts as artifacts


class StableCompanionCoverageTests(unittest.TestCase):
    def fixture(self, root, complete=False, sdk="PENDING_ENCLOSING_RELEASE"):
        contents = {
            "companion-source.tar.gz": b"source archive bytes",
            "companion-source-manifest.json": b'{"schema":1}',
            "companion-source-closure.json": json.dumps({
                "complete_corresponding_source": complete,
                "sdk_runtime_source_coverage": sdk,
            }).encode(),
        }
        assets = {}
        hashes = {}
        for name, data in contents.items():
            assets[name] = root / name
            assets[name].write_bytes(data)
            hashes[name] = hashlib.sha256(data).hexdigest()
        return {"companion": {"source_assets": hashes}}, assets

    def test_legacy_without_companion_keeps_existing_release_requirements(self):
        artifacts.require_complete_companion_source({}, {})

    def test_pending_supplement_cannot_qualify_stable(self):
        with tempfile.TemporaryDirectory() as directory:
            receipt, assets = self.fixture(Path(directory))
            with self.assertRaisesRegex(artifacts.ArtifactError, "complete companion"):
                artifacts.require_complete_companion_source(receipt, assets)
            # Neither flag is sufficient on its own and true must be a JSON boolean.
            for complete, sdk in [(True, "PENDING_ENCLOSING_RELEASE"), (False, "PASS"), ("true", "PASS"), (1, "PASS")]:
                receipt, assets = self.fixture(Path(directory), complete, sdk)
                with self.assertRaises(artifacts.ArtifactError):
                    artifacts.require_complete_companion_source(receipt, assets)

    def test_complete_record_is_accepted_only_with_exact_source_asset_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            receipt, assets = self.fixture(Path(directory), True, "PASS")
            artifacts.require_complete_companion_source(receipt, assets)
            assets["companion-source.tar.gz"].write_bytes(b"different sources")
            with self.assertRaisesRegex(artifacts.ArtifactError, "digest mismatch"):
                artifacts.require_complete_companion_source(receipt, assets)

    def test_cli_does_not_require_stable_coverage_for_general_rc_validation(self):
        args = ["verify-rc", "--artifact-dir", "/unused", "--version", "0.3.4",
                "--tag", "v0.3.4-rc.1", "--repository", "open-ott-play/ottplay-lg",
                "--output-json", "/unused.json"]
        self.assertFalse(artifacts.parser().parse_args(args).require_complete_companion_source)
        self.assertTrue(artifacts.parser().parse_args(args + ["--require-complete-companion-source"]).require_complete_companion_source)

    def test_workflow_checks_coverage_before_and_after_environment_approval(self):
        root = Path(__file__).resolve().parents[1]
        workflow = (root / ".github/workflows/native-release-pipeline.yml").read_text()
        before, after = workflow.split("  promote-stable:", 1)
        self.assertEqual(before.count("verify-rc --require-complete-companion-source"), 2)
        self.assertEqual(after.count("verify-rc --require-complete-companion-source"), 1)
        self.assertIn("environment: release", after)
