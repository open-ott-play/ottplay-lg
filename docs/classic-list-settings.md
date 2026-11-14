# Classic list and numeric playback settings

## Choose Classic and navigate back

Open **Settings → Interface mode → Classic**. This choice has its own save
flow: it takes effect after a successful save and is restored on reopening.
The native default when no mode is saved is **Classic**; an update retains
the installation's saved choice. The list settings below are separate from
this mode selection.

Physical **Blue** opens Categories where that action is available, including
from Providers. It does not replace input handling in an editor, Description
or Browse. **Back** first closes those nested views. In a Classic Channel list
or Favorites list with a nonempty search, Back clears the filter before leaving
the list. Otherwise it returns through the existing category, channel and profile
hierarchy. At the settled **Providers** root, Back opens an exit confirmation. Only confirming
exits; **Cancel**, or Back within that confirmation, keeps the application open.
Reaching a horizontal focus boundary with Left does not itself mean Back.
The explicit contextual-arrow choice described below retains its own actions.

## List and playback preferences

The native player exposes these preferences in Settings. They use the same
protected settings store and remote settings contract as the other application
preferences. Changing a preference does not select a different provider, start a
new source or cancel an active scan. A failed or uncertain save leaves the last
accepted list and playback behavior in effect.

Open **Settings → Lists** for row count and list side, **Buttons** for arrows
and numeric seeking, or **Channel list** for the display flags below. Choices
remain a group draft until **Save Settings** succeeds. **Cancel** discards
unsubmitted changes; Back from a value picker returns to its group and keeps
other draft choices. Interface mode, profile, remote-control and parental
operations have separate save flows and are not part of this draft.

- **Classic list rows** (`pageSize`): a preferred count from 10 to 30, default 25.
  The effective count can be smaller because of larger interface text or the
  available screen height. The shared list uses that effective count for both
  drawing and page navigation; this is not the number of files to scan or fetch.
- **Classic list side** (`listPosition`): right (`0`, default) or left (`1`).
  The list, preview and description move together. Pointer targets and the video
  viewport use the same layout as the visible content.
- **Classic list arrows** (`arrowFun`): paging (`0`, default), volume (`1`) or
  contextual navigation (`2`). Volume uses the current playback volume controls
  when available. Contextual navigation opens categories and programme details
  for channels, or the existing Back and description actions for media. Category
  lists continue to page. These choices affect the Classic catalogue body, not
  editors, the Modern grid or fullscreen controls.
- **Seek step for 1/3** (`seek13Duration`): seconds to skip backward with `1` or
  forward with `3` during a video or archive. The default is 15 seconds. Choices
  are 5, 10, 15, 20, 30, 60, 120, 180, 240, 300, 600, 900, 1200, 1800 and 3600.
  Live channels and the mixed Favorites list retain numeric channel selection.
  Existing ownership, foreground, modal and kiosk restrictions still apply.
- **Show channel number/name, program name and description** (`showNumber`,
  `showName`, `showProgram`, `showDescription`): integer Off (`0`) or On (`1`),
  default On. These affect the Classic list and adjacent synopsis, not source
  membership or playback. The full Description action remains available.
  They are saved per provider and shared by that provider's profile slots;
  switching providers loads that provider's flags.
- **Show program progress** (`showProgress`): Off (`0`) or On (`1`, default).
  Shows a small progress bar at the end of a Classic channel row when a valid
  current-programme interval and current time are available. It is independent
  of **Show program name** and is saved per provider across its profile slots.
  It does not invent a programme schedule for files without one.

When **Show program name** is enabled, ordinary M3U channel lists can show current
programme titles for multiple channels from the playlist's XMLTV feeds. The
player reads each feed once per batch and expires captions at the earliest
programme transition. Full descriptions and guide actions still belong to the
selected channel. This compact batch does not merge mixed Favorites, attached
sources, or other providers by channel ID; those keep their existing metadata
behavior. Missing or unusable guide data leaves the row caption empty.

Row EPG is bounded preview data, not a promise of full coverage for a large
catalogue. Each XMLTV document is limited to 8 MiB and 50,000 programme records;
the compact index and output have separate 32 MiB accounting budgets. Documents
are processed sequentially, so two valid documents are not rejected merely
because their combined size exceeds 8 MiB. The row batch considers at most the
first 16 distinct configured feeds and stops optional work at its deadline.
Already admitted captions can remain available as a partial result. Selected
channel lookup keeps its own fallback order and per-request timeout when the
optional batch stops.

Authenticated `state` / `status` responses expose `row_guide` diagnostics with a
coverage state, fixed reason codes and counts. They contain no source addresses
or programme text. Partial or unavailable coverage does not add a warning line
to the playback footer. Reasons describe the batch as a whole, not each row:
`unmatched` covers rows not covered by the result, including empty schedules,
unmatched channel identities and rows not reached before a limit. `coverage`
describes the latest admitted attempt, while `fresh` describes the retained row
cache; unavailable coverage can therefore coexist with a fresh earlier cache.
Coverage depends on feed size, programme counts and matching identities, not just
the number of channels in the playlist.

The Classic row and side preferences do not change the Modern grid. Desktop
fullscreen Left/Right continue to control volume, Up/Down retain their existing
channel behavior, and Alt/Ctrl+Left/Right retain their existing ten-second seek.

## List navigation and selected content

Numbers in an open Classic Channel list or Favorites list follow the displayed
order, starting at 1. Filtering or reordering changes these positions. Number-key
entry in that list selects the same displayed position; changing the list or
presentation retires pending input. Hidden number labels do not disable numeric
input. Modern lists, bare fullscreen video and remote channel APIs keep their
existing canonical numbers. Numbers are presentation positions, not portable
media identities.

The first nine category rows in Classic **Categories** display **1–9**. A fresh,
unmodified number key selects the same row as ordinary activation, including
All and Favorites. Disabled rows keep their number and cannot be selected;
management actions do not consume a number. Later categories remain reachable
by normal list navigation. Editors, covered screens and Modern retain their
existing input behavior.

**Actions → Search**, when Search is enabled in the menu preferences, opens the
existing search editor for the current source and category. Searching filters the list in memory; it does not change catalogue
membership, Favorites or playback. Stale Actions menus cannot open a search for
a replaced source. The separate Search control follows the same menu visibility setting.

The selected Classic sidebar can use a valid compact current-programme title
and interval while its full guide is unavailable. An admitted full guide,
including an empty one, takes precedence. **Show program name** controls row
captions, not the selected sidebar title. **Show description** hides both the
synopsis and selected-item poster; the explicit Description action remains
available. File metadata does not acquire a synthetic current/next schedule.

After ordinary Back from an M3U, VPortal or Plex catalogue, reopening that
retained source preserves its last completed or failed scan outcome. A completed empty list remains empty and complete. A failed
scan retains its error and any rows already received; returning does not silently
retry it. A scan cancelled while still loading resumes when that source's list
is reopened. Replacing the source invalidates its old return state. Returning to
the list alone does not start playback or rewrite profiles and Favorites.
Temporary media sessions retain their separate lifecycle and source checks.

## Existing profiles and remote changes

Renaming a saved profile, including through an import, preserves its remembered
category when every other profile field is unchanged. This also applies to an
inactive profile. A changed source, profile identity, alias or enabled state does
not inherit that category history. The global Favorites selection is unchanged.

The settings schema exposes the four list/playback keys and five display flags
above. Remote callers should discover the schema, read the current context and
revision, and use the existing guarded settings write operation. Values are
integers, not strings. Row count, list side, arrows and numeric seeking belong
to the client application; the five display flags have provider scope. A local
group draft does not change the remote write contract.

The legacy `arrowFun=3` value is accepted when reading an existing profile or
through the remote contract. In the Classic implementation it has the same
paging behavior as `0`; the UI therefore shows one Paging choice. Opening or
closing that picker never rewrites `3`. Explicitly choosing Paging may save the
canonical value `0`.

Missing or malformed stored values fall back to the corresponding defaults.
Unknown settings, invalid ranges and stale write contexts are rejected by the
existing settings contract. Consumers receive new preferences only after an
authoritative load or a successful durable save.
