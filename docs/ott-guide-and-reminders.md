# Programme guide, archives and reminders

This describes the native OTT guide/reminder implementation included in the source of `v0.3.3-beta.1`. Source and host-test coverage do not establish physical television playback validation. See [native delivery](ott-native-delivery.md) for package and installation evidence and [capability status](native-capability-status.md) for current limits.

## Open the guide

Select a live channel, open **Browse**, then **Programme guide**. The guide uses the same channel-list navigation, description reader and Back behavior as the catalogue. Selecting a programme opens its available actions. Long descriptions remain readable through the existing description reader.

M3U sources use their configured XMLTV feed. A compatible Stalker `/api` source can supply a requested day. MAG and Xtream sources use their supported short programme guide; they do not advertise a day browser. Refresh updates the current time for a short guide. Day browsing uses local civil-day boundaries, including daylight-saving transitions.

A film is not a live channel. Local, Plex and VPortal items continue to show film metadata, artwork and the file sequence supplied by their existing playback projection. An absent television schedule is not an error for those items.

## Watch an archived programme

**Watch archive** appears only when the retained programme is already complete and the M3U channel supplies a valid, explicit catch-up rule within its retention window. Supported rule forms are template, append, shift and Flussonic. Do not infer archive access merely from an EPG entry or a provider's channel name.

The player resolves the channel again, applies its archive rule and uses the normal protected playback path. Changing a profile or source invalidates the old intent. Unsupported or expired intervals fail without falling back to an unrelated live stream. This native path currently admits direct HTTP M3U sources; server-managed leases are not admitted for archive playback.

An archived programme ends without advancing to another channel. Its offset is not saved as the live channel's resume position. **Watch live** makes a fresh live-channel request. This does not promise persistent archive resume after a full application restart.

The playback footer shows the retained archive programme's title and description under the original channel name. It does not substitute the current live programme or invent a next programme. The start time comes from the first confirmed video presentation, and seeking does not change it. Duration comes from the playing media when available, otherwise from the selected archive interval.

Standby, an unavailable source and strict kiosk restrictions still apply before a guide action can start work. In an ordinary channel kiosk, Back closes the guide before returning to playback. A rejected archive request leaves the existing source and pending work intact.

## Set a programme reminder

For a future programme on the active saved source, choose **Add reminder**. **Remove reminder** removes the committed reminder. A pending storage write is not displayed as a successful save.

Reminders are local installation state, separate from a provider profile import. A reminder binds the source fingerprint, channel identity and programme interval; channel-list numbers are not its identity. The fullscreen player can show a passive advance notice, followed by a confirmation when the programme starts. Playback changes only after confirmation through the ordinary parental-control and kiosk admission path.

The local reminder store currently holds at most **128 saved reminders across all sources**, with a **256 KiB document limit**. Adding a reminder that would exceed either limit is refused; existing reminders are retained, not silently removed. Remove unneeded reminders before adding more. The advance-notice interval is limited to 24 hours.

These are in-application reminders, not an operating-system alarm or wake-up service. The player must be running with the corresponding source active. Standby, source changes, unavailable channels and restrictive kiosk state invalidate a pending confirmation. Transient sources and unresolved cross-source favorites do not create new reminders in this slice.

## Validation boundaries

Integration must run the prepared data, runtime and screen tests, update recording schema pins together with their replay fixtures, and then verify playback on an appropriate target. Parsing Rust source or successfully applying a patch is not compilation or device validation. Controller notifications use a separate transport and are not included in this guide/reminder slice.
