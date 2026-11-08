# Clearing a remote source slot

For numbered M3U and VPortal profiles, `profile_settings` accepts an explicit empty source string:

```json
{"number":7,"settings":{"playlist":""}}
{"number":7,"settings":{"vportal":""}}
```

This clears the address, retaining the slot number, profile name, enabled state, archive settings, active-profile identity and selected provider. The corresponding `playlist_configured` or `vportal_configured` value becomes `false`. Omitting a field leaves it unchanged. Whitespace, control characters, non-string values and invalid nonempty addresses are not an empty-source clear.

An empty slot remains editable. It cannot be loaded or start a catalogue scan, including after restart. Saving the empty address requests no reload or playback and does not take over an already running decoder. The previous ordinary catalogue is retired after the saved change; late results cannot repopulate it. Playback already bound to a source is independent of the saved address.

Set a valid address to configure the same slot again. A source parser still requires a valid nonempty address before fetching or playing; storage validation alone does not grant playback authority.

Dedicated VPortal `provider_settings` uses the same clear semantics with its existing runtime/context/revision checks. Comparing an already cleared slot against the exact empty address reports `matched: true` without writing. Save acknowledgments describe saved configuration, not proof of decoder playback.
