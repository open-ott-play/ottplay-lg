"""Contracts for immutable native webOS release plans and stable qualification."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from ci import native_release as release


SOURCE_SHA = "0123456789abcdef0123456789abcdef01234567"
IPK_SHA256 = "a" * 64
REPOSITORY = "open-ott-play/ottplay-lg"
ROOT = Path(__file__).resolve().parents[1]


def sample_plan() -> dict:
    return release.create_plan(repository=REPOSITORY, channel="beta", version="0.3.4", sequence=2,
                               source_sha=SOURCE_SHA, run_id=123456789, run_attempt=1)


def sample_qualification() -> dict:
    return {
        "schema": 1,
        "repository": REPOSITORY,
        "channel": "stable",
        "version": "0.3.4",
        "rc_tag": "v0.3.4-rc.1",
        "source_sha": SOURCE_SHA,
        "ipk_sha256": IPK_SHA256,
        "physical_evidence": [{
            "model": "LG OLED 2025",
            "firmware": "webOS 25.1.0",
            "tested_at": "2026-11-09T02:00:00Z",
            "checks": ["install", "launch", "live-playback", "channel-switching", "audio-video-sync"],
        }],
    }


class NativeReleasePlan(unittest.TestCase):
    def test_plan_uses_canonical_beta_tag_and_binds_every_identity_field(self):
        plan = sample_plan()
        self.assertEqual(plan, {
            "schema": 1,
            "repository": REPOSITORY,
            "channel": "beta",
            "version": "0.3.4",
            "sequence": 2,
            "tag": "v0.3.4-beta.2",
            "source_sha": SOURCE_SHA,
            "run_id": 123456789,
            "run_attempt": 1,
        })
        self.assertEqual(release.validate_plan(plan), plan)

    def test_plan_supports_rc_tag_with_the_same_numeric_package_version(self):
        plan = release.create_plan(repository=REPOSITORY, channel="rc", version="1.2.3", sequence=7,
                                   source_sha=SOURCE_SHA, run_id=9, run_attempt=3)
        self.assertEqual(plan["tag"], "v1.2.3-rc.7")
        self.assertEqual(plan["version"], "1.2.3")

    def test_plan_verification_compares_exact_expected_identity(self):
        plan = sample_plan()
        expected = {key: plan[key] for key in (
            "repository", "channel", "version", "sequence", "tag", "source_sha", "run_id", "run_attempt")}
        self.assertEqual(release.verify_plan(plan, expected), plan)
        for key, wrong in (("repository", "other/repo"), ("channel", "rc"), ("version", "0.3.5"),
                           ("sequence", 3), ("tag", "v0.3.4-rc.2"), ("source_sha", "f" * 40),
                           ("run_id", 1), ("run_attempt", 2)):
            with self.subTest(key=key), self.assertRaisesRegex(release.ReleaseError, f"plan {key}"):
                release.verify_plan(plan, {**expected, key: wrong})

    def test_plan_rejects_missing_unknown_or_wrongly_typed_fields(self):
        plan = sample_plan()
        with self.assertRaisesRegex(release.ReleaseError, "missing fields"):
            release.validate_plan({key: value for key, value in plan.items() if key != "run_id"})
        with self.assertRaisesRegex(release.ReleaseError, "unknown fields"):
            release.validate_plan({**plan, "extra": "no"})
        for changed in (
            {**plan, "schema": True},
            {**plan, "sequence": True},
            {**plan, "run_id": "123"},
            {**plan, "run_attempt": 0},
            {**plan, "tag": "native-webos-v0.3.4-beta.2"},
        ):
            with self.subTest(changed=changed), self.assertRaises(release.ReleaseError):
                release.validate_plan(changed)

    def test_plan_rejects_malformed_repository_versions_channels_and_sha(self):
        bad_fields = (
            ("repository", "../repo"), ("repository", "owner/name/extra"),
            ("version", "01.2.3"), ("version", "1.2"), ("version", "1.2.3-beta.1"),
            ("source_sha", "A" * 40), ("source_sha", "0" * 39), ("channel", "stable"),
        )
        for field, value in bad_fields:
            arguments = {"repository": REPOSITORY, "channel": "beta", "version": "0.3.4",
                         "sequence": 1, "source_sha": SOURCE_SHA, "run_id": 1, "run_attempt": 1}
            with self.subTest(field=field, value=value), self.assertRaises(release.ReleaseError):
                release.create_plan(**{**arguments, field: value})

    def test_plan_rejects_boolean_and_zero_sequences_or_run_ids(self):
        for field, value in (("sequence", 0), ("sequence", True), ("run_id", 0),
                             ("run_id", False), ("run_attempt", -1)):
            arguments = {"repository": REPOSITORY, "channel": "rc", "version": "0.3.4",
                         "sequence": 1, "source_sha": SOURCE_SHA, "run_id": 1, "run_attempt": 1}
            with self.subTest(field=field, value=value), self.assertRaises(release.ReleaseError):
                release.create_plan(**{**arguments, field: value})

    def test_plan_json_rejects_duplicate_keys_including_nested_objects(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "plan.json"
            path.write_text('{"schema":1,"schema":1}', encoding="utf-8")
            with self.assertRaisesRegex(release.ReleaseError, "duplicate JSON field: schema"):
                release.read_json(path)

    def test_plan_output_is_canonical_and_will_not_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "plan.json"
            release.write_plan(sample_plan(), path)
            payload = path.read_text(encoding="utf-8")
            self.assertEqual(json.loads(payload), sample_plan())
            self.assertEqual(payload, json.dumps(sample_plan(), indent=2, ensure_ascii=False, sort_keys=True) + "\n")
            with self.assertRaisesRegex(release.ReleaseError, "refusing to overwrite"):
                release.write_plan(sample_plan(), path)


class NativeReleaseQualification(unittest.TestCase):
    def test_valid_stable_qualification_requires_and_normalizes_core_evidence(self):
        qualification = sample_qualification()
        qualification["physical_evidence"][0]["checks"].append("remote-control")
        result = release.validate_qualification(qualification)
        self.assertEqual(result["rc_tag"], "v0.3.4-rc.1")
        self.assertEqual(result["physical_evidence"][0]["checks"], sorted(qualification["physical_evidence"][0]["checks"]))
        expected = {key: result[key] for key in ("repository", "version", "rc_tag", "source_sha", "ipk_sha256")}
        self.assertEqual(release.verify_qualification(qualification, expected), result)

    def test_stable_qualification_rejects_identity_mismatch(self):
        qualification = sample_qualification()
        expected = {"repository": REPOSITORY, "version": "0.3.4", "rc_tag": "v0.3.4-rc.1",
                    "source_sha": SOURCE_SHA, "ipk_sha256": IPK_SHA256}
        self.assertEqual(release.verify_qualification(qualification, expected),
                         release.validate_qualification(qualification))
        for key, value in (("repository", "fork/ottplay-lg"), ("version", "0.3.5"),
                           ("rc_tag", "v0.3.4-rc.2"), ("source_sha", "f" * 40),
                           ("ipk_sha256", "b" * 64)):
            with self.subTest(key=key), self.assertRaisesRegex(release.ReleaseError, f"qualification {key}"):
                release.verify_qualification(qualification, {**expected, key: value})
        with self.assertRaisesRegex(release.ReleaseError, "must include"):
            release.verify_qualification(qualification, {"version": "0.3.4"})

    def test_qualification_rejects_unknown_fields_and_empty_evidence(self):
        qualification = sample_qualification()
        with self.assertRaisesRegex(release.ReleaseError, "unknown fields"):
            release.validate_qualification({**qualification, "note": "unreviewed"})
        qualification["physical_evidence"] = []
        with self.assertRaisesRegex(release.ReleaseError, "at least one device"):
            release.validate_qualification(qualification)

    def test_qualification_requires_rc_tag_for_same_version(self):
        for tag in ("v0.3.4-beta.1", "v0.3.5-rc.1", "v0.3.4-rc.0", "native-webos-v0.3.4-rc.1", "v0.3.4"):
            qualification = sample_qualification()
            qualification["rc_tag"] = tag
            with self.subTest(tag=tag), self.assertRaisesRegex(release.ReleaseError, "rc_tag"):
                release.validate_qualification(qualification)

    def test_qualification_rejects_invalid_schema_channel_digest_and_sha(self):
        for key, value in (("schema", True), ("channel", "rc"), ("ipk_sha256", "A" * 64),
                           ("source_sha", "0" * 39)):
            qualification = sample_qualification()
            qualification[key] = value
            with self.subTest(key=key), self.assertRaises(release.ReleaseError):
                release.validate_qualification(qualification)

    def test_evidence_has_exact_fields_and_nonempty_trimmed_model_and_firmware(self):
        cases = []
        item = sample_qualification()["physical_evidence"][0]
        cases.append({**item, "extra": "no"})
        cases.append({key: value for key, value in item.items() if key != "model"})
        cases.extend([{**item, "model": value} for value in ("", " model", "model ", "bad\nmodel", "x" * 161)])
        cases.extend([{**item, "firmware": value} for value in ("", " firmware", "firmware ", "bad\x00version")])
        for record in cases:
            qualification = sample_qualification()
            qualification["physical_evidence"] = [record]
            with self.subTest(record=record), self.assertRaises(release.ReleaseError):
                release.validate_qualification(qualification)

    def test_evidence_timestamp_must_be_a_valid_utc_rfc3339_value(self):
        for timestamp in ("2026-11-09T02:00:00+00:00", "2026-11-09T02:00:00-08:00",
                          "2026-13-09T02:00:00Z", "2026-11-09 02:00:00Z", "2026-11-09T02:00Z"):
            qualification = sample_qualification()
            qualification["physical_evidence"][0]["tested_at"] = timestamp
            with self.subTest(timestamp=timestamp), self.assertRaisesRegex(release.ReleaseError, "tested_at"):
                release.validate_qualification(qualification)

    def test_evidence_requires_core_checks_and_allows_optional_supported_checks(self):
        cases = (
            (["install", "launch", "live-playback", "channel-switching"], "missing required"),
            (["install", "launch", "live-playback", "channel-switching", "audio-video-sync", "mystery"], "unsupported"),
            (["install", "launch", "live-playback", "channel-switching", "audio-video-sync", "launch"], "duplicates"),
            (["install", 1, "live-playback", "channel-switching", "audio-video-sync"], "entries must be strings"),
            ([], "nonempty list"),
        )
        for checks, error in cases:
            qualification = sample_qualification()
            qualification["physical_evidence"][0]["checks"] = checks
            with self.subTest(checks=checks), self.assertRaisesRegex(release.ReleaseError, error):
                release.validate_qualification(qualification)

    def test_duplicate_evidence_keys_are_rejected_by_json_reader(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "qualification.json"
            path.write_text('{"physical_evidence":[{"model":"A","model":"B"}]}', encoding="utf-8")
            with self.assertRaisesRegex(release.ReleaseError, "duplicate JSON field: model"):
                release.read_json(path)


class NativeReleaseCli(unittest.TestCase):
    def run_cli(self, *arguments: str) -> subprocess.CompletedProcess:
        return subprocess.run([sys.executable, str(ROOT / "ci/native_release.py"), *arguments],
                              cwd=ROOT, text=True, capture_output=True, check=False)

    def test_plan_and_verify_plan_cli_round_trip(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "candidate-plan.json"
            created = self.run_cli("plan", "--repository", REPOSITORY, "--channel", "rc", "--version", "0.3.4",
                                   "--sequence", "1", "--source-sha", SOURCE_SHA, "--run-id", "42",
                                   "--run-attempt", "2", "--output", str(path))
            self.assertEqual(created.returncode, 0, created.stderr)
            self.assertIn("v0.3.4-rc.1", created.stdout)
            verified = self.run_cli("verify-plan", "--plan", str(path), "--repository", REPOSITORY,
                                    "--channel", "rc", "--version", "0.3.4", "--sequence", "1",
                                    "--tag", "v0.3.4-rc.1", "--source-sha", SOURCE_SHA,
                                    "--run-id", "42", "--run-attempt", "2")
            self.assertEqual(verified.returncode, 0, verified.stderr)
            self.assertIn("Plan identity verified", verified.stdout)

    def test_verify_qualification_cli_round_trip(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "qualification.json"
            path.write_text(json.dumps(sample_qualification()), encoding="utf-8")
            verified = self.run_cli("verify-qualification", "--qualification", str(path), "--repository", REPOSITORY,
                                    "--version", "0.3.4", "--rc-tag", "v0.3.4-rc.1",
                                    "--source-sha", SOURCE_SHA, "--ipk-sha256", IPK_SHA256)
            self.assertEqual(verified.returncode, 0, verified.stderr)
            self.assertIn("Stable qualification verified", verified.stdout)

    def test_cli_rejects_leading_zero_sequence(self):
        result = self.run_cli("plan", "--repository", REPOSITORY, "--channel", "beta", "--version", "0.3.4",
                              "--sequence", "01", "--source-sha", SOURCE_SHA, "--run-id", "42",
                              "--run-attempt", "1", "--output", "/tmp/unused-plan.json")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("without leading zeroes", result.stderr)


if __name__ == "__main__":
    unittest.main()
