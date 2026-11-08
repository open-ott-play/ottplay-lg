# Editing saved provider profiles

Open **Providers**, choose a provider, then use its saved profiles list. **Edit**
opens the profile named by that row, even when another profile is playing.
Opening an editor does not load the profile or change playback.

In **Classic**, the **M3U** profiles list shows numbered slots **1–15**, including
empty positions. Configured slots show their number and profile name; empty
slots show a dash. Pressing OK on the active M3U profile opens its source editor.
Pressing OK on another configured M3U profile loads it. The active profile also
has a separate **Load playlist** command.

Pressing OK on an available empty slot opens a source editor for that position.
Opening it or cancelling does not create a saved profile. A successful save
stores a valid M3U source address in the chosen slot without selecting or
loading it. Select the newly configured slot when you want to load its playlist.

A disabled profile still occupies its slot. If saved profiles have conflicting
slot assignments, that numbered row is unavailable and the profiles remain in
separate rows. Other profiles that cannot be placed in a numbered slot also
remain in separate rows.

Saving an M3U source from this profile-list editor stores the new address
without loading it or selecting another profile. Changing the active source
clears its old channel list so that those channels cannot use the new address.
Use **Load playlist** to apply
an address changed for the active profile, or select another configured profile
to load its saved address. This separation applies to the M3U slot editors;
other settings keep their existing save behavior. In Classic M3U, use an empty
slot to add a profile. Modern keeps its existing **Add profile** command.
The Load command's preview continues to show the profile's name.

In **Classic**, pressing OK on the active **Stalker** profile opens its setup
form. Pressing OK on another configured Stalker profile loads it. The separate
Edit rows remain available. In **Modern**, configured profile rows keep their
existing load action; use the separate Edit row to edit a saved profile.

The editing form depends on the provider:

- **Xtream** uses a setup form for the profile name, server URL, username and
  password. **Stalker** uses a setup form for the profile name, portal URL and
  MAC address. Saving an individual field returns to the form. **Save profile**
  saves the form and loads that profile.
- **VPortal** and **Local** use their existing source-address or folder editor.
  A successful save keeps their existing save-and-load behavior. Local folders
  still require the existing source-access checks.
- **Plex** keeps its existing connection, account sign-in and server-selection
  flow. The generic source editor does not replace Plex authorization.

Cancel or go Back before submitting a source edit to discard it. In an Xtream
or Stalker setup form, Cancel discards the fields staged in that form. Closing
an editor after a save has already been submitted does not undo that save.
If the saved profile changes while its editor is open, reopen the profile
before trying to save again.

The numbered empty-slot view is specific to M3U in Classic. Modern and other
providers keep their existing profile lists and setup flows.
