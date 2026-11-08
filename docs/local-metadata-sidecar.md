# Explicit metadata for user-prepared Local files

The native Local asset worker optionally reads `.ottplay/metadata.json` from
an already authorized root. The user supplies the videos and any poster files.
No metadata file is created or migrated automatically. The pure parser does
not download, copy, move or inspect media, fetch artwork, contact Plex, or
grant access to a folder. See [Local assets](local-assets.md) for operation.

The existing Local catalog identifies a file by the selected profile and its
exact relative path. A display overlay preserves that identity, the media
locator, folder category, provider fields and catalog order. Moving the same
folder to a different mount point therefore does not change this module's
matching. A new receiver must still explicitly authorize its own Local root.

## Format

```json
{
  "version": 1,
  "items": [
    {
      "relative_path": "Family/Film.mkv",
      "title": "Film title",
      "description": "A full description.\nA second paragraph.",
      "duration_ms": 120000,
      "poster_relative_path": ".ottplay/posters/film.jpg"
    }
  ]
}
```

`relative_path` is required. At least one display field is required. Omitted
fields leave the catalog unchanged; an empty description explicitly clears an
old description. Present `null` values, duplicate object fields, duplicate
paths, unknown fields and unsupported versions are rejected. The whole document
must validate before an overlay is available.

Paths use `/` and exact UTF-8 spelling. Matching never folds case, normalizes
Unicode, searches by basename or guesses from a title. Absolute paths, URLs,
drive prefixes, colons, backslashes, empty path components, `.` / `..` and control
characters are rejected. Media paths must use a container extension already
admitted by Local. Poster references may end in `.jpg`, `.jpeg` or `.png`.

The parser reuses the catalog bounds: at most 8 MiB of JSON, 50,000 entries and
32 MiB of accounted model data. Paths follow Local's 8,192-byte limit. Titles
are at most 1,024 UTF-8 bytes; descriptions at most 128 KiB. Description newlines
and tabs are preserved. Duration is an integer number of milliseconds greater
than zero and no greater than `i64::MAX`; it is declared metadata, not a measured
decode duration or evidence that a file is playable.

## Plex metadata is an input, not a path authority

The native Plex preparation adapter already retains complete metadata while
preparing a transient queue. Its `Metadata` records contain titles, summaries,
duration, artwork keys and media parts. That transient table is not a persistent
USB sidecar. The user must explicitly supply the exact Local path for a record.

`Part.file` is a path on the Plex server, not on the receiving SSD. A rating key
is scoped to its server. Neither is converted into a Local path or Local
identity. Matching by title, basename, a guessed folder prefix or a Plex key is
not implemented. Multi-version and multi-part media require an explicit choice;
a whole-title duration is not silently attributed to every part.

## Local artwork

`poster_relative_path` is an explicit root-relative reference. `apply_to` keeps
it separate from `CatalogEntry.poster`; it never emits a `file:` URL or makes
an HTTP request. The app registers a typed Local artwork key only after
validating the exact current root receipt. The existing shared artwork worker
performs bounded reads and decoding, with no symlink traversal or HTTP fallback.
Revoked or replaced roots invalidate pending and displayed artwork.

## Integration boundary

The parser remains pure:

```rust
LocalMetadataMap::parse(bytes: &[u8]) -> Result<LocalMetadataMap, Error>
LocalMetadataMap::lookup(&self, relative_path: &str) -> Option<&LocalMetadata>
LocalMetadataMap::apply_to(&self, row: &mut CatalogEntry) -> Option<&LocalMetadata>
```

The existing metadata worker owns the optional read and carries the Local
source authority, metadata generation and cancellation token. During startup
it yields to current-load playback evidence. No folder scan, video hashing,
duration probe or poster preload is required before the first playable batch.
Metadata refreshes preserve row IDs and playback position and are discarded
after cancellation or source changes. The app admits updated catalog rows
through `Catalog::merge` so the complete catalog model limit still applies.
Sidecar entries never create phantom media rows: only files already admitted by
Local can receive display fields. Existing path validation and containment at
playback remain authoritative.

The written host tests cover exact mapping, identity preservation, parser bounds
and rejected input. They do not prove runtime wiring, Local poster rendering,
USB permissions or LG playback. Those require the corresponding integration
tests and an actual device check of the complete read/attachment layer.
