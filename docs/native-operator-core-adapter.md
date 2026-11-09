# Canonical native operator adapter

`rust-modules/shared-core/src/operators.rs` runs the pinned canonical shared
core in the existing isolated QuickJS context. It does not load a legacy
`provider.js`, install browser globals, or execute network and storage callbacks.
All configuration and responses cross the boundary as bounded JSON data.

The registry mirrors all 52 identities in FOSS `src/provider/driver-profiles.ts`.
Eight are existing built-in provider families and Edem has its separately owned
native codec. `OperatorId` represents the other 43 branded identities with exact
wire strings, including slash/dot spellings. Generic configuration is typed as
`GenericConfig { m3u, server, user, pass }`; brands using the same configuration
still retain distinct profile and Favorites identities.

`generic_plan` calls canonical source selection and `OperatorChannelClient` to
produce the account API request, normalized channel rows or playlist fallback.
A fresh reducer receives the full response; it cannot retain another source's
catalogue. The host supplies whether the request failed, and executes the
returned next request only under its existing cancellation/ownership checks.
`profile_url` supplies named-provider request routes; `catalog` reduces decoded
ITV/OTTCLUB/Shura responses; `live_url` and `guide_url` supply canonical stream and
guide routes. Account-token flows remain explicit host stages.

`parse_operator_playlist` preserves canonical provider IDs and archive metadata.
KbTeam's URL hashing remains host-owned: the adapter requires an explicit
URL-to-hash table and rejects missing inputs. It never invents a hash, thereby
avoiding silently changed Favorites/channel identities.

The adapter bounds request JSON to 2 MiB, returned JSON to 8 MiB, URLs to 64 KiB
and normalized rows to 50,000. VM memory/stack limits remain those of the shared
planner. Errors omit VM exception details and credentials. The caller must still
validate endpoint and redirect policy, apply native transport bounds, reject
stale generations, and publish source-owned catalogue state. A pure request plan
is not evidence that a provider account or a physical LG has played successfully.
