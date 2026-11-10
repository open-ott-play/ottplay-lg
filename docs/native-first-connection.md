# First native connection and Home preset

Updating the native application preserves its own saved data. A web-player installation on the same TV has a separate identity and storage; installing the native package does not import the web player's profiles.

1. Open Settings → Remote control and read Device ID. Only a confirmed saved identity is displayed; wait if it says identity loading or unavailable.
2. With the dynamic registry enabled, choose Find command server → home on the TV first. Inspect `ott pending`, match the displayed code and exact device ID, then use `ott registry approve lg2_native DEVICE_ID CODE` to register and approve that receiver together.
3. A controller without the dynamic registry requires registration first: `ott add lg2_native DEVICE_ID`, then Find command server → home and `ott approve lg2_native CODE`. Unregistered receivers are rejected in that mode.
4. Keep an existing web alias unchanged. Verify `ott lg2_native status`, then run `ott lg2_native load home`.
5. Read back `ott lg2_native profiles` and check the M3U slots, attached portals and other configured providers. A successful package installation alone does not establish that Home was applied.

The Device ID is a public receiver identifier, not a credential. After configuration load confirms its persistence, the device writes this identifier once to its existing private diagnostic event log for local Developer Mode troubleshooting. No controller endpoint or access token is added to that message. Credentials received during pairing use the protected storage transaction; failure to save them is not successful pairing.

## Existing native Plex sign-in

Choosing Plex remotely restores missing profile metadata from the same currently credential-eligible native server snapshot used by the interface. This is one protected profile save with the original remote receipt; it does not copy or replace native account credentials or start a sign-in flow. Existing usable profiles retain their IDs and deliberately disabled duplicates are not enabled unnecessarily.

A Home preset carrying another Plex origin or token is a different configuration even if it reaches the same server. The import refuses to overwrite an existing native authorization; exact `settings_compare` remains false. Use native Plex sign-in for an intentional account change. Functional recovery of the existing native Plex catalogue is reported separately from exact preset credential equality.

## Plex commands after native sign-in

Once the existing native Plex sign-in has created a saved server profile, library and queue commands can use that account directly. For example, `ott PLAYER plex shuffle "Movies"` prepares a shuffled library using the same authorized server as the native player. It does not require importing another copy of the account credentials.

An active Plex profile supplies the exact server. When a different provider is active, there must be exactly one enabled saved Plex profile; multiple candidates require an explicit profile choice rather than guessing. Unavailable, signed-out or replaced clients are rejected, including if authorization changes while preparation is running. Existing imported Plex settings remain bound to their exact saved origin and token; an invalid import cannot silently fall back to another account. Read-only list and preview operations preserve playback and saved settings.
