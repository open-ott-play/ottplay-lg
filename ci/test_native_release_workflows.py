"""Contracts joining the reusable native checks, exact candidate build, and release promotion."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]


def workflow(name: str) -> str:
    return (ROOT / ".github" / "workflows" / name).read_text(encoding="utf-8")


class NativeReleaseWorkflowContracts(unittest.TestCase):
    def test_required_source_lane_checks_notices_without_cargo(self):
        body = workflow("ci.yml")
        source = body.split("  source:", 1)[1].split("\n  native:", 1)[0]
        self.assertIn("- run: python3 ci/gen-third-party-notices.py --check", source)
        self.assertIn("- run: python3 -m unittest ci.test_gen_notices -v", source)
        self.assertNotIn("--verify-graph", source)
        self.assertNotIn("continue-on-error:", source)
        self.assertNotRegex(source, r"(?m)^    if:")
        self.assertIn("needs: [source, native, arm, release-contracts]", body)
        native = body.split("  native:", 1)[1].split("\n  arm:", 1)[0]
        self.assertLess(native.index("uses: ./.github/actions/rust-toolchain"),
                        native.index("python3 scripts/lg-build.py check"))

    def test_release_dispatch_is_manual_serialized_and_channel_scoped(self):
        body = workflow("native-release-pipeline.yml")
        self.assertRegex(body, r"(?m)^  workflow_dispatch:")
        self.assertNotRegex(body, r"(?m)^  (?:push|schedule|pull_request):")
        self.assertIn("options: [beta, rc, stable]", body)
        self.assertRegex(body, r"(?m)^      sequence:\n        description: Positive beta or RC sequence; leave empty for stable promotion\n        type: string$")
        self.assertIn('test "$GITHUB_REF" = refs/heads/main', body)
        self.assertIn("group: native-release-${{ inputs.version }}", body)
        self.assertIn("cancel-in-progress: false", body)

    def test_beta_and_rc_require_quality_and_a_frozen_source_plan_before_build(self):
        body = workflow("native-release-pipeline.yml")
        self.assertIn("quality:\n    name: Required native quality gates", body)
        self.assertIn("uses: ./.github/workflows/ci.yml", body)
        self.assertRegex(body, r"(?s)candidate:.*?needs: \[preflight, quality\].*?uses: \./\.github/workflows/native-candidate.yml")
        self.assertIn("native_release.py plan", body)
        self.assertIn("native_release.py verify-plan", body)
        self.assertIn("source_sha must equal github.sha", body)
        self.assertIn("must be greater than the existing $CHANNEL sequence", body)

    def test_candidate_build_is_reusable_and_consumes_the_frozen_plan(self):
        body = workflow("native-candidate.yml")
        self.assertRegex(body, r"(?s)^name: Build native webOS candidate\n\non:\n  workflow_call:")
        self.assertNotIn("workflow_dispatch:", body)
        self.assertIn("ci/native_release.py verify-plan", body)
        self.assertIn("persist-credentials: false", body)
        self.assertIn("native-release-plan.json", body)
        self.assertIn("webos-native-candidate-${{ inputs.source_sha }}-${{ github.run_id }}-${{ github.run_attempt }}", body)

    def test_beta_and_rc_publish_only_verified_artifact_bytes_as_prereleases(self):
        body = workflow("native-release-pipeline.yml")
        publish = body.split("  publish-candidate:", 1)[1].split("  promote-stable:", 1)[0]
        self.assertIn("needs: [preflight, quality, candidate]", publish)
        self.assertIn("verify-native-candidate.py", publish)
        self.assertIn("check-packaged-elf.py", publish)
        self.assertIn("--prerelease", publish)
        self.assertIn("--target \"$SOURCE_SHA\"", publish)
        self.assertIn("candidate-provenance.json.sha256", publish)
        self.assertIn("link-evidence.tar.gz", publish)
        self.assertIn("Verify published prerelease identity and bytes", publish)
        self.assertIn('cmp "$CANDIDATE_DIR/$asset" "$RUNNER_TEMP/published-candidate/$asset"', publish)

    def test_stable_requires_protected_environment_and_exact_rc_evidence(self):
        body = workflow("native-release-pipeline.yml")
        stable = body.split("  promote-stable:", 1)[1]
        self.assertIn("environment: release", stable)
        self.assertIn("verify-qualification", body)
        self.assertIn("native_release_artifacts.py verify-rc", body)
        self.assertIn('--candidate-artifact-dir "$ORIGINAL_DIR"', body)
        self.assertIn("isPrerelease", body)
        self.assertIn("git merge-base --is-ancestor", body)
        self.assertIn("cmp \"$CANDIDATE_DIR/com.openottplay.player.native_${VERSION}_arm.ipk\"", stable)
        self.assertIn('cmp "$CANDIDATE_DIR/$asset" "$RUNNER_TEMP/stable-public/$asset"', stable)
        self.assertIn('cmp "ci/ottplay-integration/release/qualifications/v${VERSION}.json"', stable)
        self.assertIn('"published stable release metadata is incorrect"', stable)
        self.assertNotIn("make FLAVOR=stable", stable)

    def test_reusable_ci_has_release_contracts_inside_the_fail_closed_gate(self):
        body = workflow("ci.yml")
        self.assertRegex(body, r"(?m)^  workflow_call:")
        self.assertIn("name: Release contracts", body)
        self.assertIn("ci.test_native_release_workflows", body)
        self.assertIn("needs: [source, native, arm, release-contracts]", body)
        self.assertIn('test "$RELEASE" = success', body)

    def test_legacy_draft_path_is_removed(self):
        self.assertFalse((ROOT / ".github/workflows/native-candidate-draft.yml").exists())

    def test_release_actions_are_pinned_and_network_jobs_audit_egress(self):
        for name in ("native-release-pipeline.yml", "native-candidate.yml"):
            body = workflow(name)
            for action in re.findall(r"(?m)^\s+uses: ([^\s]+)", body):
                if action.startswith("./"):
                    continue
                self.assertRegex(action, r"@(?:[0-9a-f]{40})(?:\s|$)", f"unpinned action in {name}: {action}")
            if name == "native-release-pipeline.yml":
                self.assertIn("step-security/harden-runner@1d88384fd173af6ecb012dfd0264a306b711241a", body)


if __name__ == "__main__":
    unittest.main()
