# Native Favorites protocol storage profile

This native profile provides read-only recovery inspection of the existing protected Favorites state. It does not implement acknowledgement or qualify a new write backend.

## Storage interpretation

The native player retains one protected `Saved` value containing both its portable Favorites document and its operation journal. Inspection does not split that store or create another Favorites list.

Both `ledger_revision` and `store_revision` are SHA-256 of the exact UTF-8 namespace value read from canonical protected storage. This includes the complete encrypted/sealed envelope of that value. It excludes transport JSON, the enclosing generic host record, unrelated namespaces and the backing database wrapper. Hash the exact captured string, without trimming, normalization, reserialization or an additional layer of JSON quoting. Decode that same captured value; return no snapshot or digest if authentication fails.

On the host, read the namespace payload through the existing held-directory, bounded, no-follow storage path, but reject unsafe modes rather than repairing them. Both host and ARM reads use the existing writer guard. There is no new Favorites store or worker service. Inspection does not create directories, migrate data or repair permissions.

On ARM, connect once to the existing helper through its dedicated `inspect-v1.sock` endpoint. Keep the existing descriptor schema and verify the peer, nonce, protocol and helper generation. The inspection listener accepts only an authenticated canonical namespace read (`OttLoad`); it does not perform the normal Hello setup, protection probes or diagnostic clearing. Rejected requests do not enter the ordinary mutation dispatcher. Endpoint publication and removal belong to the normal helper lifecycle, outside inspection.

A missing endpoint, an older helper without the inspection endpoint, or failed authentication returns an explicit unavailable/error result. The client must not fall back to the ordinary socket, create an activation request, launch the helper, migrate storage or retry through a write-capable path. Update the bundled helper together with the native player to enable inspection on older installations. This restriction concerns explicit helper activation and storage mutation; it does not claim that the operating system never starts a service while decrypting an existing protected value.

The inspection helper exchange has one monotonic eight-second budget covering connection and framed input/output. Its database call is limited to the smaller of two seconds or the remaining exchange budget; an expired request does not start another database call. This transport budget does not describe the separate protected-value decryption step.

If the platform refuses to cancel a timed-out database call, the helper retains its existing fail-safe process termination to prevent a dangling callback. Inspection suppresses the startup-failure marker on that path and reports an unsuccessful read. It does not restart the helper. A later ordinary helper start owns cleanup of the fixed endpoint.

Only demonstrated absence of the canonical namespace value uses SHA-256 of the literal bytes `null`. A present value with an empty or terminal-only journal still uses its actual value hash. A cleared host record, malformed value, unsupported legacy-only storage, failed authentication, unavailable helper or uncertain read is an explicit error, never a successful empty collection.

## Context and original records

The context preimage is compact UTF-8 JSON with this exact array order:

`["ottplay-native-favorites-recovery-context-v1","namespace-value-v1","ott-state",1,app_id,persisted_receiver_device_id]`

It is independent of the active provider, category and playback checkpoint. Runtime, authenticated authority, deadline and applicable policy are checked independently at admission and completion. Fixed byte/hash fixtures pin this encoding.

Validate the entire private journal before filtering any terminal entries. Unknown versions, duplicate operation identifiers, malformed identities and inconsistent original evidence fail the collection. Original record hashes cover the complete original native `DurableStatus` encoding, including original receipt evidence and null optional fields. Freeze the field order with byte/hash fixtures; do not hash a reconstructed public projection.

Return original unresolved records in sorted operation-ID order, at most 16 per page and 32 total. Continuation requests reread canonical storage and compare the captured namespace-value revisions and context. A changing or busy writer yields a typed error. The recovery-relevant membership and complete journal are checked against the captured/current runtime state; unrelated unsaved category or playback position changes do not invalidate a truthful read.

## Boundaries

`backend_proof` is always null. A selected inspection has `acknowledge_allowed:false`, and all three acknowledgement-plan fields are null. A valid acknowledgement request is rejected as `unsupported_persistence`, with zero writes. The original uncertain operation is never replayed, hidden, cleared or relabeled as verified.

The installed controller accepts this complete read-only inspection before ordinary import. A clean collection does not itself qualify the core writer: existing native core admission, durable intent, protected write and exact readback checks remain necessary. Unresolved records continue to block new writes.

Full acknowledgement requires a separate versioned native contract: changing the one native container would change both hashes, whereas the current acknowledgement response requires an unchanged membership-store hash. Adding CAS alone would not resolve that semantic difference.

## Separate item identities

The portable Favorites document remains unchanged. Receiver-local item IDs are genuine UUIDv4 values persisted as bounded metadata beside that document. They survive title/order changes while the same portable identity exists. Imports do not inherit another receiver's row IDs. Reads never initialize or persist IDs; guarded migration and normal write paths own that work. Failed or uncertain migration preserves the original readable Favorites and retains the applicable write fence.

## Qualification boundaries

Tests cover actual absence, present empty/terminal journals, malformed and unresolved records, exact hashes, pagination, concurrent changes, no-write acknowledgement refusal, legacy storage, receiver-ID migration and mixed-provider item dispatch. Pure protocol tests, protected-storage tests and device playback acceptance are separate evidence. Passing a protocol test does not establish protected-storage durability or device playback.
