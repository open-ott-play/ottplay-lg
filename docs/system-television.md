# System television channels

System television is a separate source from an M3U provider. It uses the LG
television's scanned channels and system player. OTTPlay does not receive a video
URL from the tuner and does not decode that picture itself.

## Setup

The controller must resolve the paired OTTPlay device identity to its physical
TV binding. If the television already has a canonical binding for another
player, add the native player identity to that binding’s `channel_player_ids`
list. Do not duplicate the TV endpoint or replace the older player’s identity.
Channel aliases share the same TV command queue and pairing key, and grant no
power or application-launch access. Developer Mode access and OTT remote pairing
do not by themselves grant TV-channel access.

In the controller's private `televisions` configuration, explicitly enable
`"player_channel_control": true` for that binding. It defaults to false. Keep the
TV address, SSAP key and administrator token on the controller; do not put them in
an OTT profile. The player reuses its own paired controller connection.

Upgrade the controller and explicitly repeat SSAP pairing with the television
visible. The expanded request asks for `READ_TV_CHANNEL_LIST`,
`CONTROL_INPUT_TV` and `READ_CURRENT_CHANNEL`. Old saved keys are not assumed to
have gained those permissions. See the control-server repository's
`docs/television.md` for configuration and CLI details.

## Use and ownership

Open **Providers → System television** to read the bound television's catalogue.
An empty catalogue can mean that no channels have been scanned. Permission,
connection and pairing failures must be resolved before selecting a channel.

Selecting a channel releases OTTPlay's current decoder before asking LG to tune.
An accepted command is not proof that video appeared. Selection is sent once;
an uncertain response is not automatically retried. The system television
application then owns viewing and remote-control behavior. Reopen OTTPlay to
return to its interface.

This is not an M3U import, a portable playlist or a strict kiosk source. OTTPlay
cannot enforce its PIN or kiosk policy after handing control to another app;
system viewing is therefore blocked while incompatible playback restrictions
are active. The existing OTT profile is not replaced by a system-TV selection.

## LG Channels and programme data

Channels returned by the television are selectable by their opaque channel ID.
LG Channels availability in that catalogue depends on model, region and TV
configuration. A controller binding can opt in to the official US metadata with
`"lg_channels_country": "US"`. These additional rows use opaque IDs such as
`lgfast:US:120` and are explicitly labelled a regional catalogue, not channels
verified on every television. The controller fetches the official metadata;
the native package does not bundle a universal regional list. The administrator
CLI also supports an explicit terrestrial channel-number selection. Retire the
native decoder before a direct administrator CLI tune; selecting through the
native System television source performs its own decoder handoff.

Current-channel and current-programme metadata can be queried through the
controller. A full guide, archive support, thumbnails and direct FAST/tuner stream
URLs are not guaranteed by these interfaces.

## Overlay status

The delivered path is system-player handoff. It does not promise OTTPlay's SDL
HUD over live system video. A transparent web overlay is a separate experimental
path: its compositor role, input focus, sleep/wake and visibility over Live TV
must be validated on actual models before enabling it in production. Ordinary
Homebrew installation does not grant private broadcast APIs or root access.

## Acceptance

Host tests and an ARM package establish software/build behavior only. On each TV,
verify consent and catalogue reads first, then deliberate channel selection,
video/audio continuity, return to OTTPlay, remote keys and power/sleep behavior.
Do not interpret an accepted SSAP response as physical playback acceptance.
