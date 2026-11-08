# Modern provider browsing

The Modern OTT catalogue can show a horizontal row for each real provider
category. It uses the same catalogue, category membership and user-defined
ordering as the Classic channel list. Native Plex Home continues to use its
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
retain their existing list/grid presentation. Empty categories have no row. If
some All items have no real category, the player keeps the flat All grid so no
item disappears and no provider category is fabricated. Progressive discovery
can switch to category rows once current items are fully represented.
The presentation admits at most 50,000 total category occurrences. Deeper or
larger overlap falls back to the complete flat All grid; it never truncates
categories or content. Removed shelf focus keys are retired on refresh rather
than accumulating across source replacements.

Source replacement retires row memory. Catalogue appends, navigation tokens and
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

Native VPortal artwork already supports explicit HTTP and HTTPS addresses and
relative paths, including inherited episode artwork. The image request does not
borrow VPortal or Plex authentication headers. Existing request, byte and decode
limits remain in force; no browser artwork proxy or HTTPS downgrade was added.
Both presentations retain the shared aspect-fit image renderer. A supplied URL
still needs a successful fetch and sufficient space in the detail panel.

## Verification scope

Host regressions exercise the real focus dispatcher, per-row memory, catalogue
updates, full-description reader inputs and a loopback HTTP JPEG request. These
checks do not establish physical-TV appearance or a new installed package.
Use the source revision and delivery receipt when qualifying a later LG build.
