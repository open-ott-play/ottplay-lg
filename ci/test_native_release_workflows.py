"""Contracts joining the reusable native checks, exact candidate build, and release promotion."""

from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile
import textwrap
import shlex
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
        self.assertIn("needs: [source, native, arm, release-contracts, renderer-replay, local-decoder-build, local-decoder-sessions, local-virtual-output, harness]", body)
        native = body.split("  native:", 1)[1].split("\n  arm:", 1)[0]
        self.assertLess(native.index("uses: ./.github/actions/rust-toolchain"),
                        native.index("python3 scripts/lg-build.py check"))

    def test_complete_player_harness_is_required_by_the_gate(self):
        body = workflow("ci.yml")
        harness, gate = body.split("  harness:\n", 1)[1].split("\n  gate:", 1)
        self.assertIn("- run: python3 tests/test_harness.py", harness)
        self.assertNotIn("continue-on-error", harness)
        self.assertIn("HARNESS: ${{ needs.harness.result }}", gate)
        self.assertIn('test "$HARNESS" = success', gate)

    def test_isolated_host_cache_resolves_to_the_actual_build_target(self):
        native = workflow("ci.yml").split("  native:\n", 1)[1].split("\n  renderer-replay:", 1)[0]
        self.assertIn('workspaces: rust-modules -> ${{ steps.host-cache.outputs.target }}', native)
        self.assertIn('--work-dir "$NATIVE_CHECK_DIR"', native)
        self.assertIn('NATIVE_CHECK_DIR: ${{ runner.temp }}/lg-check', native)
        code = textwrap.dedent(native.split("python3 - <<'PYCACHE'\n", 1)[1].split("          PYCACHE", 1)[0])
        with tempfile.TemporaryDirectory() as temporary:
            for name in ("checkout", "deep path/repo with spaces"):
                checkout = Path(temporary) / name
                workspace = checkout / "rust-modules"
                workspace.mkdir(parents=True)
                check_dir = Path(temporary) / "runner temp" / name / "lg-check"
                output, environment = checkout / "outputs", checkout / "environment"
                env = dict(os.environ, NATIVE_CHECK_DIR=str(check_dir),
                           GITHUB_OUTPUT=str(output), GITHUB_ENV=str(environment))
                subprocess.run([sys.executable, "-c", code], cwd=checkout, env=env, check=True)
                key, relative = output.read_text().strip().split("=", 1)
                self.assertEqual(key, "target")
                self.assertFalse(Path(relative).is_absolute())
                self.assertEqual((workspace / relative).resolve(), (check_dir / "target").resolve())
                self.assertEqual(environment.read_text(), "NATIVE_CHECK_DIR=" + str(check_dir) + "\n")

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

    def test_candidate_packaging_routes_the_committed_product_configuration(self):
        config = "ci/ottplay-integration/release/product.json"
        commands = [shlex.split(line.strip())
                    for line in workflow("native-candidate.yml").splitlines()
                    if line.strip().startswith("make ")]
        self.assertEqual({tokens[-1] for tokens in commands}, {"ipk-stage", "ipk-pack"})
        for tokens in commands:
            self.assertIn("OTT_PRODUCT_CONFIG=" + config, tokens,
                          "candidate packaging must not fall back to upstream release documents")
        source = workflow("ci.yml").split("  source:", 1)[1].split("\n  native:", 1)[0]
        self.assertIn("python3 ci/check-package.py --tracked --product-config " + config, source)

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

    def test_publisher_credential_is_confined_to_actual_release_creation_steps(self):
        release_workflow = workflow("native-release-pipeline.yml")
        secret_ref = "secrets.BOT_PAT"
        self.assertEqual(release_workflow.count(secret_ref), 2)

        candidate = release_workflow.split("  publish-candidate:", 1)[1].split(
            "  promote-stable:", 1
        )[0]
        candidate_create = candidate.split("      - name: Publish immutable prerelease", 1)[1].split(
            "      - name: Verify published prerelease identity and bytes", 1
        )[0]
        candidate_verify = candidate.split(
            "      - name: Verify published prerelease identity and bytes", 1
        )[1]
        self.assertIn(secret_ref, candidate_create)
        self.assertIn("|| github.token", candidate_create)
        self.assertIn("gh release create", candidate_create)
        self.assertNotIn(secret_ref, candidate_verify)
        self.assertIn("GH_TOKEN: ${{ github.token }}", candidate_verify)

        stable = release_workflow.split("  promote-stable:", 1)[1]
        stable_create = stable.split("      - name: Publish stable from the exact RC package bytes", 1)[1].split(
            "      - name: Verify published stable release identity and bytes", 1
        )[0]
        stable_verify = stable.split(
            "      - name: Verify published stable release identity and bytes", 1
        )[1]
        self.assertIn(secret_ref, stable_create)
        self.assertIn("|| github.token", stable_create)
        self.assertIn("gh release create", stable_create)
        self.assertNotIn(secret_ref, stable_verify)
        self.assertIn("GH_TOKEN: ${{ github.token }}", stable_verify)

        for name in ("ci.yml", "native-candidate.yml"):
            self.assertNotIn(secret_ref, workflow(name))
        self.assertNotIn(secret_ref, release_workflow.split("  preflight:", 1)[1].split("  quality:", 1)[0])
        self.assertNotIn(secret_ref, release_workflow.split("  quality:", 1)[1].split("  candidate:", 1)[0])

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
        self.assertIn("needs: [source, native, arm, release-contracts, renderer-replay, local-decoder-build, local-decoder-sessions, local-virtual-output, harness]", body)
        self.assertIn('test "$RELEASE" = success', body)

    def test_renderer_replay_is_network_denied_complete_and_required(self):
        body = workflow("ci.yml")
        replay = body.split("  renderer-replay:\n", 1)[1].split("\n  local-decoder-build:", 1)[0]
        self.assertIn("runs-on: macos-15", replay)
        self.assertIn('cargo +"$RUST_TOOLCHAIN" build --locked', replay)
        self.assertIn("--features hostsim --bin plxnative-sim", replay)
        self.assertIn("(deny network-outbound)", replay)
        self.assertIn("errno.EPERM, errno.EACCES", replay)
        self.assertIn("tests/replay_fixtures.py", replay)
        self.assertIn("--fixtures tests/fixtures/replay --assets pkg", replay)
        self.assertIn("--window 960x540 --jobs 1 --timeout 300", replay)
        self.assertNotIn("continue-on-error", replay)
        self.assertNotRegex(replay, r"(?m)^    if:")
        self.assertIn("if: always()", replay)
        self.assertIn("if-no-files-found: error", replay)
        self.assertIn('RENDERER_REPLAY: ${{ needs.renderer-replay.result }}', body)
        self.assertIn('test "$RENDERER_REPLAY" = success', body)

    def test_real_decoder_qualification_is_separate_dependent_and_required(self):
        body = workflow("ci.yml")
        build = body.split("  local-decoder-build:\n", 1)[1].split("\n  local-decoder-sessions:", 1)[0]
        real = body.split("  local-decoder-sessions:\n", 1)[1].split("\n  local-virtual-output:", 1)[0]
        self.assertIn("timeout-minutes: 40", build)
        self.assertIn("timeout-minutes: 40", real)
        self.assertIn("needs: [local-decoder-build]", real)
        self.assertIn("CARGO_BUILD_JOBS: '1'", real)
        self.assertIn("ci/qualify-local-multiview.py build", build)
        self.assertIn("ci/qualify-local-multiview.py qualify", real)
        artifact = "local-decoder-${{ github.run_id }}-${{ github.run_attempt }}"
        self.assertIn("name: " + artifact, build)
        self.assertIn("name: " + artifact, real)
        for stage in (build, real):
            self.assertNotIn("continue-on-error", stage)
            self.assertIn("if: always()", stage)
            self.assertIn("RESULT.json", stage)
        self.assertIn('test "$DECODER_BUILD" = success', body)
        self.assertIn('test "$DECODER_SESSIONS" = success', body)

    def test_decoder_receipts_bind_source_libraries_cabi_and_the_one_real_test(self):
        body = (ROOT / "ci/qualify-local-multiview.py").read_text()
        self.assertIn('HOST="1", JOBS="1", RELEASE="1"', body)
        self.assertIn('"GITHUB_RUN_ID"', body)
        self.assertIn('"GITHUB_RUN_ATTEMPT"', body)
        self.assertIn('"HEAD^{tree}"', body)
        self.assertIn('receipt.get("identity") != ident', body)
        self.assertIn('inventory(prefix) != receipt["prefix"]', body)
        self.assertIn('sha(destination / "source" / dep["archive"]) != dep["sha256"]', body)
        self.assertIn('registry(prefix) != receipt["registry"]', body)
        self.assertIn('"ci/ffabi-assert.c"', body)
        self.assertIn("codec.avcodec_find_decoder_by_name", body)
        self.assertIn('nine_independent_real_decoders_video_audio_seek_withdrawal_and_retirement', body)
        self.assertIn('"--exact", "--ignored", "--list"', body)
        self.assertIn('"--exact", "--ignored", "--nocapture", "--test-threads=1"', body)
        self.assertIn('test result: ok. 1 passed; 0 failed; 0 ignored;', body)
        qualification = body.split("def qualify(", 1)[1].split("def main(", 1)[0]
        self.assertLess(qualification.index("verify(runner"), qualification.index('"fixtures"'))
        self.assertIn("signal.SIGTERM", body)
        self.assertIn("signal.SIGKILL", body)
        generator = (ROOT / "ci/generate-local-multiview-fixtures.py").read_text()
        self.assertNotIn("/opt/homebrew", generator)
        self.assertIn("range(9)", generator)
        self.assertIn("timeout=45", generator)
        self.assertIn("fixture destination must be empty", generator)
        self.assertIn('if index in (2, 6, 7):', generator)
        self.assertIn('"video_count": 9', generator)
        self.assertIn('"hls_objects": hls_objects', generator)
        self.assertIn("Fixture object escaped its source directory", body)
        self.assertIn("Duplicate fixture objects", body)
        self.assertIn('["git", "diff", "--quiet", "HEAD", "--"]', body)
        self.assertIn("if identity() != ident:", body)
        self.assertIn("Decoder inventory contains an escaping or absolute symlink", body)
        build = body.split("def build(", 1)[1].split("def verify(", 1)[0]
        self.assertLess(build.index("assert_source(ident)"), build.index('"BUILD.json"'))
        self.assertLess(qualification.index('"nine-sessions"'), qualification.index("assert_source(ident)"))
        self.assertLess(qualification.index("assert_source(ident)"), qualification.index('"QUALIFIED.json"'))

    def test_virtual_output_is_owned_dependent_and_part_of_the_gate(self):
        body = workflow("ci.yml")
        job = body.split("  local-virtual-output:\n", 1)[1].split("\n  arm:", 1)[0]
        self.assertIn("needs: [local-decoder-build, local-decoder-sessions]", job)
        self.assertIn("timeout-minutes: 40", job)
        self.assertIn("CARGO_BUILD_JOBS: '1'", job)
        self.assertIn("ci/qualify-local-multiview.py output", job)
        self.assertIn("if: always()", job)
        self.assertIn("proof-*/**", job)
        self.assertNotIn("continue-on-error", job)
        self.assertIn('test "$VIRTUAL_OUTPUT" = success', body)
        helper = (ROOT / "ci/qualify-local-multiview.py").read_text()
        self.assertIn("nine_real_yuv_textures_and_virtual_sdl_audio", helper)
        self.assertIn('LIBGL_ALWAYS_SOFTWARE="1", SDL_AUDIODRIVER="pulseaudio"', helper)
        self.assertIn('"--daemonize=no"', helper)
        self.assertIn('private.mkdir(mode=0o700)', helper)
        self.assertIn('DBUS_SESSION_BUS_ADDRESS="unix:path=" + str(private / "nonexistent-session-bus")', helper)
        self.assertIn('DBUS_SYSTEM_BUS_ADDRESS="unix:path=" + str(private / "nonexistent-system-bus")', helper)
        self.assertIn('"-displayfd"', helper)
        self.assertIn('"--device=ottplay_ci.monitor"', helper)
        self.assertIn("Virtual audio monitor captured only silence", helper)
        self.assertIn("owned_cleanup", helper)
        self.assertIn('for profile in ("core", "es2"):', helper)
        self.assertIn("PLXNATIVE_LOCAL_VIDEO_GL_PROFILE=profile", helper)
        self.assertIn('"counts_scope": "per configuration"', helper)
        self.assertIn("self.__exit__(*sys.exc_info())", helper)
        self.assertIn('with VirtualOutput(runner, env, profile) as output:', helper)
        self.assertIn('"stale_audio_owner", "stale_gain_owner", "gain_mute_measured"', helper)
        self.assertIn('self.phase_order = ("full", "half", "mute", "unmute")', helper)
        self.assertIn('"trim_start_s": 0.5, "trim_end_s": 0.1', helper)
        self.assertIn('if not 0.4 <= ratio <= 0.65:', helper)
        self.assertIn('if not 0.8 <= ratio <= 1.2:', helper)
        self.assertIn('phase == "mute" and measured["rms"] > 1', helper)
        self.assertIn("tick=output.phase_tick", helper)

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
