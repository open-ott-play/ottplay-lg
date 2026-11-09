# Native hosted EPG fallback

The native player can use an explicitly configured hosted guide service when an
M3U live playlist does not advertise an XMLTV guide. This supplies current and
next programmes, descriptions, supported day schedules, and current programme
captions in the channel list. Finite files and other providers keep their own
metadata adapters.

The setting is `nativeEpgApiV1`. Its wire type is **string**, with a 4096-byte
UTF-8 limit. The default empty string disables the fallback. Otherwise its value
is one line of JSON text:

```json
{"version":1,"url":"https://guide.example/epg/v1","source":"epg-one"}
```

The endpoint is the hosted JSON API, not an XMLTV download address. Version 1
accepts the `epg-one` source and an HTTP(S) endpoint ending in `/epg/v1`. URL
credentials, query parameters, fragments, and unknown configuration fields are
rejected. Use an endpoint reachable from the television; a Mac's loopback
fallback address does not refer to that Mac when used on the television.

Save the example as a one-line file, then use the existing settings command:

```sh
ott PLAYER setting nativeEpgApiV1 --value-file /absolute/path/native-epg.json
ott PLAYER setting nativeEpgApiV1
```

The CLI removes one final line feed from a value file. Authored JSON spacing and
key order remain unchanged in the stored string and its settings receipt; the
player separately parses a typed configuration for requests. Disable it with:

```sh
ott PLAYER setting nativeEpgApiV1 ''
```

The native schema fixture is
[`native-epg-setting-v1.json`](../rust-modules/src/app/fixtures/native-epg-setting-v1.json).
The existing settings runtime, source-context and revision checks protect writes.
An older native release that does not advertise this setting must be updated
before using it.

## Source priority and request ownership

An M3U header's `url-tvg`, `x-tvg-url`, or `tvg-url` XMLTV reference takes priority.
An unavailable or temporarily empty provider guide is not permission to replace
it with another service. The hosted fallback is used only when the source
advertises no guide.

The CLI's global `epg` configuration is for CLI programme searching. It is not
silently copied into the player. Configure this native setting explicitly.

Guide requests run in the existing cancellable metadata worker. Matching sends
channel identifiers and display names, never playlist URLs, media URLs or
provider credentials. The service's match, current and programmes responses must
agree on their source and accepted feed generation. Catalogue-row batches are
bounded; partial coverage is reported rather than presented as a complete scan.

Changing the effective endpoint or disabling the setting cancels the previous
metadata generation and invalidates its selected and row-guide facts. A delayed
reply cannot publish into the replacement configuration. Playback, the selected
channel, Favorites and the media catalogue are retained. Cosmetic JSON
reformatting alone does not restart guide work.

Host fixtures qualify protocol validation and ownership checks. They do not prove
that a particular television can reach a configured endpoint or that its current
channel has matching guide data; those require a live device check.
