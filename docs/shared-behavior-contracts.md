# Shared behavior reference

These are parity requirements, not claims that every native rendering path has
already passed acceptance. The initial reference is OTTPlay FOSS commit
`47adabad66b41b33e126925f7f00b17c7bf19b58`; later coordinated changes must identify
their new reference and update the relevant regression fixtures.

## Repeated seek feedback

A repeated seek updates the existing visible banner in place. Keep its position
and do not restart the entry animation. Every accepted press starts a complete
new three-second lifetime, including cancellation of an exit animation already
in progress. Display rounding never changes the actual seek target.

The first live-to-archive commit must not show a second preview when delayed
programme-guide or authorization results arrive. Those completions retain the
same operation/session ownership.

Reference regressions live in the FOSS `notification-persistence.spec.cjs`,
`test_archive_session.cjs` and `test_playback_session.cjs` suites. Native drawing,
clock and input fixtures must establish equivalent behavior before this item is
marked complete.

## Shared finite seek planning

The initial native integration routes admitted `seek` and `seek_relative`
commands through the pinned canonical `playbackSeekPlan`. The native controller
still verifies authorization, active decoder and finite duration. Absolute
positions beyond duration are rejected; relative offsets clamp at the bounds.
An invalid request cannot mutate the queued seek or playback identity.

The 35 semantic fixtures run through the same generated core in Node and
QuickJS. Native controller tests additionally check execution and response
semantics. These do not by themselves qualify physical decoder seeking or the
on-screen banner.
