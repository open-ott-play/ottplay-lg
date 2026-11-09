# Native webOS release process

## Workflow and current status

`.github/workflows/native-release-pipeline.yml` is the manually dispatched release entry point. It accepts `channel=beta|rc|stable` and runs only from `main`. The beta and RC paths freeze a release plan, run required CI, call the reusable `.github/workflows/native-candidate.yml` build, verify the resulting IPK and evidence, and publish an immutable GitHub prerelease. Stable promotion validates an accepted RC and its physical-device qualification, waits at the `release` environment, then publishes the exact RC package bytes without rebuilding.

The first pipeline beta, `v0.3.3-beta.1`, was published from source `14c5af1f09d35d71af5c179338c5deeb088ffb21` by Actions run `42617296886`. Its build and publication gates passed. No physical LG qualification record is supplied by this document. The first actual beta/RC run and physical acceptance must be recorded against the exact artifact before making release claims.

## Release identity and dispatch inputs

The `version` input is the numeric webOS application/package version `X.Y.Z` already committed to both `pkg/appinfo.json` and `ci/ottplay-integration/release/product.json`. Channel suffixes are release metadata only: they do not appear in `appinfo.json` or the IPK filename. The IPK remains `com.openottplay.player.native_X.Y.Z_arm.ipk`.

For beta or RC, dispatch from `main` with:

- `channel`: `beta` or `rc`.
- `version`: the committed numeric `X.Y.Z` version, with authored notes at `ci/ottplay-integration/release/notes/vX.Y.Z.md`.
- `source_sha`: the full commit SHA being dispatched; it must equal the workflow's exact `github.sha` on `main`.
- `sequence`: a positive integer greater than all already used sequence numbers for that version and channel.
- `rc_tag`: leave empty.

The workflow reserves the release identity as `vX.Y.Z-beta.N` or `vX.Y.Z-rc.N`. Existing tags/releases cannot be overwritten. The frozen plan binds repository, channel, version, sequence, tag, source SHA, workflow run ID, and run attempt. The same plan is verified by the candidate builder and publisher.

For stable promotion, dispatch from `main` with `channel=stable`, the same numeric `version`, and the exact `rc_tag` (`vX.Y.Z-rc.N`). Leave `source_sha` and `sequence` empty. The workflow checks that the RC source is an ancestor of current `main`, the RC is a published prerelease, its release assets match their provenance and checksums, and its producing release run succeeded for the same source and attempt.

Example dispatches from a checkout whose `origin/main` is current:

```sh
SOURCE_SHA=$(git rev-parse origin/main)
gh workflow run native-release-pipeline.yml --ref main \
  -f channel=beta -f version=0.3.4 -f source_sha="$SOURCE_SHA" -f sequence=1

gh workflow run native-release-pipeline.yml --ref main \
  -f channel=rc -f version=0.3.4 -f source_sha="$SOURCE_SHA" -f sequence=1

gh workflow run native-release-pipeline.yml --ref main \
  -f channel=stable -f version=0.3.4 -f rc_tag=v0.3.4-rc.1
```

Use the next unused sequence for each channel and version. The examples use a future version only to show the command shape; the version must already be committed in both package manifests and have authored release notes before dispatch. Each run freezes its own plan and artifact identity. Do not blindly rerun a failed release workflow: first inspect whether its tag or GitHub Release was created. Dispatch a fresh run after correcting the problem; if the tag or release already exists, choose a new unused sequence and never replace the published release.

## Required quality gates

For beta and RC, `native-release-pipeline.yml` invokes `.github/workflows/ci.yml`. Its final CI gate requires all of these jobs to succeed:

- **Source contracts**: public-source checks, LG script tests, and shared-core source contract.
- **Native host tests**: native application and layer checks using the repository's pinned toolchain.
- **ARM shared core**: compile and link validation against the pinned webOS SDK/ABI on ARM.
- **Release contracts**: tests for release-plan and qualification validation, artifact verification, candidate receipt, workflow wiring, and package/product document contracts.

The gate is fail-closed: a failed or missing required result cannot proceed to candidate building/publication. The reusable candidate workflow then validates the source, pins/dependencies and release plan; builds the stable-flavor IPK; and preserves the IPK checksum, package/link evidence, provenance receipt, and release plan. The publisher independently verifies candidate bytes and link evidence before creating the prerelease.

Stable does not build a new candidate or rerun the beta/RC build. It revalidates the original RC identity and qualification before publication. The repository's protected GitHub environment `release` is configured with required reviewer `4alvit` and a protected-branches-only deployment policy. The stable job pauses for that approval before it publishes.

## Beta and RC publication

Both beta and RC use the same required native CI gate and exact-source package builder. After successful checks, the pipeline publishes the candidate IPK and matching checksum/provenance artifacts as a prerelease with the appropriate tag:

- Beta: `vX.Y.Z-beta.N`.
- RC: `vX.Y.Z-rc.N`.

The release notes include the exact tag, source SHA, candidate run, and generated IPK SHA-256. Prerelease publication proves the build and repository gates passed; it does not by itself claim physical-TV qualification. A beta is for testing and is not eligible for direct stable promotion.

An RC becomes eligible for stable only after actual physical-device acceptance has been recorded in `ci/ottplay-integration/release/qualifications/vX.Y.Z.json`. The file must match the repository, numeric version, accepted RC tag, RC source SHA, and exact IPK SHA-256. Its `physical_evidence` must contain real records with TV model, webOS firmware, UTC test timestamp, and checks. The validator requires at least `install`, `launch`, `live-playback`, `channel-switching`, and `audio-video-sync`; it also accepts `remote-control` and `standby-resume` checks. Do not create or infer device results from CI output, remote status alone, screenshots from a different build, or an untested device.

No qualification file or device result is added by the release-pipeline implementation change. Stable promotion remains blocked until real evidence is written and passes identity validation.

## Stable promotion and exact bytes

Stable promotion uses `channel=stable` and an exact `rc_tag`. If the RC embeds a companion, its hash-bound source closure must declare both complete corresponding-source coverage and passing SDK runtime-source coverage. Pending coverage blocks stable in preflight and after environment approval; beta and RC may retain the pending supplement. Before and after environment approval it verifies the RC release assets against the original Actions artifact fetched by run ID, package SHA-256, provenance, frozen plan, source SHA, run ID/attempt, successful producing workflow, source ancestry, and the qualification file. It then creates `vX.Y.Z` from the RC source SHA using the downloaded RC assets and qualification record.

The stable job does not invoke the package build. It compares every published stable asset, including the IPK, checksum sidecars, receipt, plan, link evidence archive, and qualification JSON, against the approved files byte for byte; it also checks the final GitHub Release metadata and tag target. Beta and RC publication likewise downloads the created release and checks its tag, prerelease state, source target, and each uploaded artifact. Original Actions artifacts are retained for 90 days; if an RC artifact expires before stable promotion, create and qualify a fresh RC rather than weakening this check. A changed package, version, source, build input, dependency, toolchain, or package metadata requires a new RC, a new physical acceptance pass, and a new matching qualification record. Published tags and release assets are not replaced.

## Device evidence to record

For each physical TV used to qualify an RC, the qualification record must contain the actual model, webOS firmware, UTC test time, and checks performed. Required checks establish installation, launch, live playback, channel switching, and audio/video synchronization. Add remote-control and standby/resume only when actually exercised. Keep logs/screenshots sanitized and tied to the exact package identity when retained alongside the record.

The qualification schema intentionally records only supported evidence fields. Profile persistence, EPG/archive details, seek behavior, or other scenarios may be documented in release notes or a linked test report, but do not add unsupported keys to the JSON schema. Mark unavailable or untested targets as pending in accompanying notes; never fill the qualification record with assumed results.

Qualification record shape (replace every placeholder with observed device data):

```json
{
  "schema": 1,
  "repository": "open-ott-play/ottplay-lg",
  "channel": "stable",
  "version": "0.3.4",
  "rc_tag": "v0.3.4-rc.1",
  "source_sha": "<40-character source SHA>",
  "ipk_sha256": "<64-character RC package SHA-256>",
  "physical_evidence": [
    {
      "model": "<observed LG model>",
      "firmware": "<observed webOS firmware>",
      "tested_at": "<UTC timestamp, for example 2026-11-08T20:15:00Z>",
      "checks": ["install", "launch", "live-playback", "channel-switching", "audio-video-sync"]
    }
  ]
}
```

Host tests and ARM linking establish their own source/build contracts. They do not prove installation, screen output, synchronized playback, remote input, saved state, or firmware compatibility on a television.

## Automation boundaries

The workflow can automate dispatch validation, plan generation and verification, sequence monotonicity checks, source/version matching, quality gates, reproducible identity checks, package/link/checksum/provenance validation, prerelease publication, RC qualification-schema matching, protected stable approval, and exact-byte promotion.

Physical acceptance is manual unless a future real-device lab can observe and attest to the same checks against the candidate hash. A successful compile, `ares-install`, remote-control connection, or playback status response is not proof that the TV rendered synchronized video or that app state survived an upgrade. The release environment protects the stable publishing step; it does not replace the operator's responsibility to test and record the RC.

## Homebrew metadata on stable promotion

Stable promotion generates `com.openottplay.player.native.manifest.json` directly from the approved RC package and verifies its SHA-256 before publication. The manifest is attached beside the unchanged IPK and downloaded again for a byte comparison. Its package URL names the exact stable version. This closes manifest delivery; admission into the external Homebrew catalogue still requires submitting the listing and passing its review.
