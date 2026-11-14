# System-TV overlay probe — untested on hardware

This standalone diagnostic app uses an ordinary transparent **web** overlay window. It does not modify the native SDL app, does not tune channels, does not connect to a server, and has no video/audio decoder. It therefore tests compositor visibility and input only. It closes after 20 seconds or Back/Escape; a visible Close button is also available. Its ID is separate from the production app and contains no profiles or credentials.

Package explicitly when a TV validation session is authorized:

```sh
ares-package ./source -o /private/output/directory
```

Install/launch only during a scheduled test. Run the packaging command from this experiment directory. This is not a production integration. Developer Mode installation is the intended experiment; manifest acceptance and visibility still vary with platform/model. A successful web overlay does not prove that native SDL receives an overlay surface role.

## Acceptance checklist

1. Start known moving system Live TV and confirm sound before launching this probe.
2. Confirm picture and sound continue under the translucent lower panel; record model/webOS version/source.
3. Check Back closes the panel, Home still works, and the automatic 20-second close restores normal viewing without relaunching a channel.
4. In a separate session check OK, CH+/CH−, wheel/pointer focus and suspend/wake. Input routing must be recorded; this probe does not remap keys or enforce kiosk.
5. Repeat over LG Channels and HDMI separately; success on one source does not establish another.
6. Native SDL overlay would require a separate implementation with explicit surface ownership and background lifecycle, not bypassing its existing render gate.

## Reference

The window mechanism is independently specified using the webOSbrew `defaultWindowType` documentation and the transparent web-overlay precedent documented in LG Camera Overlay. No camera/video code or third-party assets were copied.

- https://www.webosbrew.org/develop/guides/appinfo/#defaultwindowtype-string
- https://github.com/mbsintegrations/LG-Camera-Overlay/blob/3c27605a93123e87244a277b49c464a763274f34/source/appinfo.json

No install, launch, tune, pairing or physical-TV test was performed while preparing this probe.

## Static validation

Run `python3 -m unittest discover -s experiments/system-tv-overlay -p test_probe.py -v` from the repository root. This checks package isolation and the absence of network/media dependencies; it does not simulate a TV compositor.
