# Troubleshooting native CI

Check the current PR head, not only a failed historical run:

```sh
gh pr view PR --repo open-ott-play/ottplay-lg --json headRefOid,mergeStateStatus,reviewDecision,statusCheckRollup
gh run view RUN --repo open-ott-play/ottplay-lg --log-failed
```

## Native compilation and dependency updates

ICU datetime provider APIs can move between minor versions. The ICU 2.4 update
requires Gregorian pattern markers from `provider::semantic_skeletons` and year
and month-name markers from `provider::names`. Numeric M/MM and L/LL patterns
also request the `NUMERIC` month marker: return `MonthNames::Numeric` without
requiring a localized text table. The ICU 2.4 CLDR data also changes Australian
English, Latin-American Spanish and Austrian/Italian German abbreviations;
keep custom tables aligned with that version and its locale-parent fallback.
Preserve locale fallback and run
the existing exhaustive regional-date oracle rather than weakening its expected
results. sha2 0.11 changed the digest container; the build receipt verifier
formats each digest byte as two lowercase hexadecimal digits and still compares
the exact artifact digest and size against the independent manifest.

The Native host and ARM shared-core gates test these boundaries on their actual
compiler targets. Keep locked dependency resolution, manifest validation, locale
oracles and the aggregate CI gate enabled. A source compatibility error needs a
code fix; repeating the unchanged build does not migrate an API. Host/ARM CI is
not physical television or shipping-package qualification.

## Early failures and hung test binaries

The locked locale preflight runs before the full native suite, with a required
nonzero test match. This catches missing compiled locale data before unrelated
media workers begin. Both complete normal and hostsim suites still run after a
successful preflight; the exhaustive date oracles are unchanged.

The test runner limits each test executable to 900 seconds by default. An
overrun terminates that executable's process group, retains its partial output,
reports `TIMED OUT`, and fails the gate. Other package results remain visible.
The group leader remains unreaped during the TERM grace period, followed by
a final group kill before output is drained. A parent exiting or a child
redirecting its output does not exempt the child from timeout cleanup.
`--timeout-seconds` accepts only positive finite values for explicit diagnosis.
This limit covers binary execution, not compilation or the entire suite; the
outer 45-minute job limit remains unchanged. Fake-process regression tests run
before native compilation and exercise timeouts, output and process cleanup.

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

## Dependency notices

Source contracts check committed crate classifications and notice versions and run
the notice generator unit suite before native compilation. Native host checks also compare those classifications with the
locked ARM application and packaged storage-helper runtime graph using `ci/gen-third-party-notices.py --check --verify-graph`;
this resolves Cargo metadata without building or running dependency code. Update
`ci/third-party-crates.json` from that graph and regenerate notices after a dependency
change. Both checks fail the existing aggregate CI gate; neither writes in check mode.
