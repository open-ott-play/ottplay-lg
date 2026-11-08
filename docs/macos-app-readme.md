# OTTPlay Native for macOS

OTTPlay Native for macOS shares its application core with the LG webOS version. The runtime
libraries it needs are included in the app bundle.

## Open it (the first time only)

macOS will refuse to open it on the first try: the app is signed, but not by a registered Apple
developer, and Apple charges for that. This is normal and you only do it once.

1. Drag **OTTPlay Native.app** to your Applications folder (or anywhere you like).
2. **Right-click it → Open**, then click **Open** in the dialog.
   *If macOS says the app "is damaged and can't be opened", that is the download quarantine, not
   the app. Open Terminal and run:*
   `xattr -dr com.apple.quarantine "/Applications/OTTPlay Native.app"`
3. Say **Allow** when macOS asks about finding devices on your local network if you use media
   servers on that network. Local-file access does not require a Plex account; see the playback
   limitation below.

After that it opens by double-click like anything else.

## Choose a provider

A fresh launch without a usable Plex session opens the provider catalogue. Choose the source or
configured profile you want to use. Plex is optional; a retained Plex session may open its home
screen instead.

If you choose Plex sign-in, scan the QR code or enter the displayed code at **plex.tv/link**.
The app then discovers the account's servers. Network requirements depend on the chosen source;
Plex needs a reachable, authorized server endpoint, which can use HTTPS.

## Getting around

It is a TV interface, so it is driven like one — with arrow keys, not the mouse.

| Key | Does |
|---|---|
| ↑ ↓ ← → | Move focus |
| Return | Open / select |
| Esc or Delete | Back |
| Cmd-Q | Quit |

Clicking works too, on most screens.

## Playback in this bundle

**This packaged `.app` does not enable video playback.** Its build disables the developer
features that activate the host playback modes, and it does not include their external FFmpeg
executable. Selecting Play reports the normal playback failure.

A separately configured development simulator can show decoded video when `clocksink` and
`simvideo` are explicitly enabled and external FFmpeg is available. `clocksink` alone discards
media and advances a clock; it is not decoded video or audio output. Successful development
simulator playback does not establish playback in
this bundle, audio output, smoothness, support for every codec/provider, or LG TV playback.

## Small print

Apple Silicon Macs only (M1 and later), macOS 11 or newer.

Unofficial, and not affiliated with, endorsed by, or sponsored by Plex GmbH or LG Electronics. MIT
licensed; the licence and the third-party notices are inside the app bundle, under
`Contents/Resources`.
