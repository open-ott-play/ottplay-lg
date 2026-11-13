# Troubleshooting native CI

Check the current PR head, not only a failed historical run:

```sh
gh pr view PR --repo open-ott-play/ottplay-lg --json headRefOid,mergeStateStatus,reviewDecision,statusCheckRollup
gh run view RUN --repo open-ott-play/ottplay-lg --log-failed
```

## Native compilation and dependency updates

ICU datetime provider APIs can move between minor versions. The ICU 2.4 update
requires Gregorian pattern markers from `provider::semantic_skeletons` and year
and month-name markers from `provider::names`. Preserve locale fallback and run
the existing exhaustive regional-date oracle rather than weakening its expected
results. sha2 0.11 changed the digest container; the build receipt verifier
formats each digest byte as two lowercase hexadecimal digits and still compares
the exact artifact digest and size against the independent manifest.

The Native host and ARM shared-core gates test these boundaries on their actual
compiler targets. Keep locked dependency resolution, manifest validation, locale
oracles and the aggregate CI gate enabled. A source compatibility error needs a
code fix; repeating the unchanged build does not migrate an API. Host/ARM CI is
not physical television or shipping-package qualification.

## Reviewed merge scans and policy waits

A reviewed single-pass scan examines several PRs. A policy-blocked PR must wait
without marking an unrelated triggering PR's orchestration as failed. GitHub
merge-state metadata, current-head tests, independent reviews and current-base
ancestry are checked before a merge attempt. Known blocked, behind, dirty, draft
or unknown states defer that candidate. Other candidates are still examined.
Each PR retains its own test results: a successful metadata scan cannot turn a
failed Native check green or bypass the CI gate.

A rejected merge is never blindly retried. One read-only reconciliation can
confirm changed eligibility, tests, identity or protection state and defer the
candidate. Account identity is resolved before entering merge-write reconciliation, so a failed
account read cannot be mistaken for a changed policy state. API/authentication
errors, malformed metadata and an unexplained
rejection of an unchanged mergeable PR remain visible failures. No administrator
bypass or permission change is part of this behavior.

Callers pin the reusable workflow to a reviewed immutable commit. Re-running an
old job keeps its old pin; adoption requires updating the caller revision. Use
a fresh invocation of the updated workflow to verify the repair. Preserve failed
and cancelled history as evidence, and use reruns only for an identified
transient problem rather than to hide a deterministic failure.
