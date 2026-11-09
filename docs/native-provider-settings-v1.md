# Bound native provider configuration, version 1

This additive contract configures native Edem and the 43 registered named
operators. It retains the existing `settings_compare` and `provider_settings`
actions and their operator permissions. Existing unversioned provider workflows
remain separate. A request containing `version` must satisfy this entire contract;
it never falls through to an unversioned handler.

The [shared fixture](../contracts/native-provider-settings-v1.json) freezes
the provider allowlist and complete request/result examples. Native implementation
is owned by `ottplay-lg`; source qualification is separate from device rollout.

## Negotiation and fields

Capabilities advertise exactly
`native_provider_settings:{version:1,operations:["compare","set"],providers:[...]}`.
`providers` is a unique nonempty subset of the 44 canonical fixture IDs. Operation
order is immaterial, but neither duplicates nor extra operations are allowed.
The CLI checks the selected provider is advertised. Provider aliases do not
change portable or saved identity; bound configuration files use canonical IDs.

First send the relay probe `capabilities` with
`{native_provider_settings:true}`. A compatible controller validates the probe
and forwards the ordinary empty capability parameters to the player. An older
controller refuses it before any credential request. The controller does not
invent player support. The CLI requires the exact player marker in the reply.

Edem patches may contain only `key`, `list`, `portal`. `key` is 1–512 ASCII
letters, digits, underscores or hyphens. `list` is an integer 0–3. `portal` is a
string of at most 8,192 UTF-8 bytes; empty removes the embedded portal. The native
codec validates nonempty links. Stream host/CDN fields are not accepted.

Other admitted operators accept only string fields `server`, `username`,
`password`, `playlist`, `mac` (0–8,192 UTF-8 bytes each), integer `list` (0–3) and
integer `mode` (0–2). Strings exclude control characters and malformed Unicode;
booleans, floating-point spellings, unknown keys and empty patches are invalid.
Native provider codecs remain authoritative for provider-specific meaning.

The complete queued request envelope, including its ID, action, parameters,
expiry and JSON escaping, must fit 16 KiB before journal admission or enqueue.
The CLI additionally bounds its private file and generated requests before
submission. No request or response includes a raw private source fingerprint.

## Compare, then one conditional save

`settings_compare` accepts exactly:

```json
{"version":1,"runtime":"RUNTIME","provider":"edem","number":3,"settings":{"list":2}}
```

`number` is the explicit saved profile slot 1–15, separate from Edem's playlist
template. Runtime, context and revision use the existing 1–128 ASCII token grammar
`[A-Za-z0-9_.:-]+`. The success result is exactly:

```json
{"version":1,"runtime":"RUNTIME","provider":"edem","number":3,"context":"CONTEXT","revision":"REVISION","matched":false}
```

Comparison prepares the canonical provider-codec candidate and compares it with
the canonical current configuration. Codec defaults, such as Shura's default
server and TVTeam's default playlist, therefore do not cause endless mismatches.
Unspecified fields are preserved. A matching candidate needs no save or reload.

For a nonmatching comparison, submit `provider_settings` once with the same
version, runtime, provider, number and exact patch plus the returned `context`
and `revision`. The player stores bounded, expiring, single-use private tickets.
A ticket binds runtime, canonical provider, explicit slot and profile identity,
private source fingerprint, active-context/edit epoch and patch digest. Tokens
are opaque random handles, not publicly guessable hashes of credentials.

A changed runtime, source, active context, profile, revision or patch rejects the
write, including a change away and back. The player consumes the ticket once;
re-reading metadata does not authorize replaying a possibly completed write.

After confirmed persistence, success is exactly:

```json
{"version":1,"runtime":"RUNTIME","provider":"edem","number":3,"context":"CONTEXT","revision":"REVISION","saved":true,"fields":["list"],"reloaded":true,"reload_required":false}
```

`fields` contains each submitted patch key exactly once. Runtime/provider/slot
match the request; context and revision echo the **consumed input receipt**.
They are not new write authority. Every later operation begins a fresh compare.
`reloaded` and `reload_required` are booleans and cannot both be true. A changed
active target may reload after verified saving. An inactive target preserves the
active provider, profile, selection and standby state. A no-op does not reload.
The response confirms persistence, not visible playback or provider connectivity.

## Failure and privacy

Rejected/unsupported responses contain exactly `version`, `runtime`, `provider`,
`number`, `reason`, with the requested identity echoed. Allowed reasons are
`invalid_request`, `unsupported`, `policy_restricted`, `runtime_changed`,
`source_changed`, `revision_conflict`, `invalid_value`, `storage_failed`, `busy`,
`expired`. There is no free-text error or credential/configuration echo.

The controller validates closed replies against retained request identity, token
and field-name metadata, including after completion. Binding metadata does not
retain patch values. The ordinary authenticated pending queue necessarily holds
the private submitted payload; existing queue lifetime and access controls apply.
Operation journals retain only the existing safe operation metadata.
Native configuration response `data` is limited to 2,048 bytes, including error
responses; the existing outer transport envelope limit remains unchanged.

The CLI prints fixed diagnostics and projected public results. Failed, malformed,
late or missing save replies are never automatically retried, switched to another
slot or sent through a legacy route. An unverified persistence outcome is not
reported as proof that nothing changed. Inspect before deciding on a new request.


## Native receipt and persistence ownership

The LG implementation retains at most 64 private receipts for 120 seconds.
Each compare returns independently random context/revision handles. Admitted
conditional save attempts consume their matching receipt, including conditional
guard failures; requests rejected before conditional admission do not. Success
only echoes the consumed handles. No public source hash or credential value is
included in a response. Admission requires the original duplicate-checked queued
request bytes; Value-only legacy admission cannot authorize these providers.

A profile or navigation edit away and back invalidates the receipt through the
existing edit/navigation epochs. Canonical no-ops consume authority and skip
persistence/reload. Inactive slot writes preserve the current provider/profile.
Changed active settings may reload only after confirmed persistence and a fresh
source/context check at save completion. A storage failure or expired completion
is not evidence that no durable change occurred; begin a new compare to inspect.
