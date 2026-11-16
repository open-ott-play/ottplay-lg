# Modern provider browsing

Modern presents provider, profile and category choices as native collection
cards using the same card components as media libraries. Opening a card performs
its existing canonical navigation action; it does not manufacture playback or
preview authority. Add/Edit/Connect and disabled choices remain in the shared
Browse controls, so one management row cannot turn the whole catalogue into a
text form. Actual settings and editors retain their normal form controls.

Category cards use real current membership counts and, within a bounded display
budget, representative member artwork. A member's plot is not presented as its
folder's synopsis, and profiles do not borrow another profile's old catalogue art.
Containers can display explicitly supplied descriptions through the shared reader.

The Modern OTT catalogue shows a presentation-only **All** row followed by a
horizontal row for each real provider category. It uses the same catalogue,
category membership and user-defined ordering as the Classic channel list. Native Plex Home continues to use its
server-provided hubs; the OTT catalogue does not invent Plex recommendations.

In the unfiltered **All** view, Left and Right move within a category. Up and
Down, including the mouse wheel, move between rows and return to that row's
remembered item. Each row keeps its own horizontal scroll. Opening an item's
actions or returning through navigation memory preserves the position while
the source is unchanged. This is application navigation memory, not a new
persisted profile setting or a guarantee across process restarts.

A title may belong to multiple categories. Each occurrence has an independent
focus position, but Play, Preview and Favorites still address the same canonical
item. Category rows reference one shared metadata table; they do not copy each
film's description and media locator into every row.

Search, Favorites, a selected category, temporary Local sessions and Classic
retain their existing list/grid presentation. Empty categories have no shelf.
The All row contains every canonical item, including uncategorized items; it is
not a fabricated provider category. Real category overlap references the same
canonical rows. Progressive discovery can append to these rows without selecting
or playing a new item.
The presentation admits at most 50,000 total occurrences, including the All row.
Deeper or larger overlap falls back to the complete flat All grid; it never truncates
categories or content. Removed shelf focus keys are retired on refresh rather
than accumulating across source replacements.

Source replacement retires row memory, including provider/profile/category card
focus and open descriptions. Returning through navigation retains the caller's
focus only while its current source remains valid. Live M3U rows use their explicit
live metadata for landscape cards and programme captions. Channel logos and
collection thumbnails fit inside their card; film and episode covers retain the
existing cover treatment.

Catalogue appends, navigation tokens and
presentation updates do not grant permission to reuse old playback requests;
those requests still use the current source-bound command contract.

## VPortal descriptions and artwork

The source parser converts recognised formatting tags and one layer of HTML
entities into plain text. Paragraph and line breaks remain. Unknown or malformed
angle-bracket prose is retained as text; the native player does not execute it.
Double-escaped text is not repeatedly decoded.

The source marks where the synopsis starts after its generated year, duration
and age facts. Classic and Modern previews start there, rather than spending
their limited space on those facts. **Description** retains the facts and the
entire admitted text. No title/year-looking sentence is guessed or removed.
The annotation is checked against the complete text during projection; painting
uses a constant-time checked slice, and stale annotations fall back to full text.

Native VPortal artwork supports explicit HTTP and HTTPS addresses and relative
paths, including inherited episode artwork. Its explicit portrait `img` is
preferred to `imglr`; validated inherited artwork remains a fallback. The image
request does not
borrow VPortal or Plex authentication headers. Existing request, byte and decode
limits remain in force; no browser artwork proxy or HTTPS downgrade was added.
The selected-item detail panel retains its shared aspect-fit image renderer. A
supplied URL still needs a successful fetch and sufficient panel space.

## Local artwork retention

Already discovered ordinary Local covers can remain visible after focus moves.
The source-owned RAM cache retains at most 64 positive artwork proofs and 256 KiB
of accounted model data. It retains no media-file copies or full descriptions and
starts no extra scan. Each lookup checks the current profile, canonical item and
locator, and authorized root receipt; stale proof cannot authorize artwork reads.
Favorite previews retain their separate selected-item guards. The shared image
store continues to own texture loading and eviction.

## Verification scope

Host regressions exercise the real focus dispatcher, per-row memory, catalogue
updates, full-description reader inputs and a loopback HTTP JPEG request. These
checks do not establish physical-TV appearance or a new installed package.
Use the source revision and delivery receipt when qualifying a later LG build.
