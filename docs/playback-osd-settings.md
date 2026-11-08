# Playback display settings

The native player exposes two application preferences in its OTT Settings page,
using the same choices in Classic and Modern. Changing either preference preserves
the current source and playback position.

## OSD background level

Choose a level from **0 to 10**. The default **7** retains the existing native
background appearance. Zero removes the playback panel background; higher levels
make its themed background stronger. These levels are not percentages of the final
pixel opacity: Classic has overlapping background layers and Modern uses a sheet.

The preference changes playback panel backgrounds and their rims only. It does
not dim the text, icons, subtitles, selection targets or the rest of the application's
settings windows, and does not change the panel's animation or auto-hide duration.

## Permanent clock

Choose **Off** (the default), **Background**, or **Transparent**. The enabled clock
shows local time to minute precision during foreground playback, even after the
transport controls hide or while the video is paused. Background uses the selected
OSD level; Transparent draws only the clock text.

The clock is hidden while another menu, catalogue or decision window covers
playback. It creates no input target and does not keep the transport controls open.
Until the application has a valid wall-clock sample, no clock is displayed. This
preference does not replace the programme timing or media-position displays.

## Saving and remote settings

A choice takes effect after its settings write is confirmed. A failed or uncertain
write does not publish the candidate appearance. Successful preferences are restored
by the existing settings loader on the next launch.

The existing player-settings schema exposes `osdOpacity` as an integer from 0 to
10 (default 7), and `permanentTime` as 0 for Off, 1 for Background and 2 for
Transparent (default 0). Use the normal schema/get/set revision contract. These
native OSD levels are calibrated to the native theme; a level is not a portable
claim about a percentage of opacity in another renderer.

## Deliberately separate settings

Classic `pageSize` and `listPosition` describe list density and mirrored layout;
Modern uses its measured grid. Classic `arrowFun` configures list navigation, and
`seek13Duration` configures numeric 1/3 seeking. They are not equivalents of these
two display preferences. This change adds no placeholder controls for them and does
not change the native input mappings or the existing Skip interval preference.
