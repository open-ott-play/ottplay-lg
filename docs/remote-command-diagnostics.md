# Remote command correlation

A playback interruption can be an intentional remote command rather than a media
failure. The optional `command_activity_diagnostics` version-2 capability exposes
correlation keys for the current native player's bounded ingress history. It is
separate from the existing `status.command_activity` version-1 view, which remains
unchanged.

## Read the evidence

A matching controller and CLI provide `ott PLAYER audit` for retained controller
metadata, or `ott PLAYER audit --activity` to correlate that metadata with a
negotiated native activity view. These commands require the corresponding
controller/CLI implementation; a player without the version-2 capability cannot
supply native request IDs. Do not infer support merely from remote control being
connected.

The native view retains at most eight selected command-family receipts, oldest
first. Each entry includes its closed action/result labels, age, informational
sequence and a canonical request ID, or `null` when its original ID cannot be
safely exposed. Read-only diagnostic queries do not displace these entries.
Control-authority replacement or retirement clears the history.

Correlate only an exact request ID within the same configured controller, device
and current native runtime/authority context. Sequence numbers reset with the
authority and may repeat after their safe-integer limit; neither a sequence nor a
similar timestamp identifies a command.

## Interpret the limits

An `ok` receipt reports the command response, not proof that playback changed.
Some selected families also contain read operations. A canonical-looking request
ID alone does not authenticate a controller or identify a human, application or
chat. Shared administrator credentials cannot distinguish their users.

Controller retention, restart, access restrictions, an old player, a legacy
request ID or ring eviction can leave a command uncorrelated. Report that result
as unknown or not retained. Do not attribute it by timing, replay a command to
reconstruct history, or switch sources while investigating a newer selection.

The synthetic wire fixture is
`ci/ottplay-integration/command-activity-v2.json`. It contains no operational IDs,
credentials or device state.
