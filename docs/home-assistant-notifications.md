# Home Assistant notifications

The native player receives Home Assistant banners through its paired OTT
controller. The controller must have its Home Assistant bridge and notification
rules configured. Pairing a remote control alone does not configure those rules.

Both Classic and Modern use the same overlay and installation-wide preference:

```sh
ott PLAYER notifications
ott PLAYER notifications on
ott PLAYER notifications off
ott ha status
```

Replace `PLAYER` with the player's saved CLI name. The preference defaults to on
after its storage has been read successfully. It survives a restart and is
independent of providers, profiles and Favorites. A storage failure does not
silently grant permission or report a successful save. Remote preference changes
remain available in standby and strict kiosk.

Text banners appear at the upper right without taking focus, changing channels
or waking the player. Up to three cards can be visible. Entry takes 480 ms; after
another 500 ms the background and icon begin fading, while text remains legible
until exit. Cards slide back to the right on expiry. Each new card receives a
bounded 0–5 px vertical offset.

When the controller offers camera snapshots, the player accepts bounded JPEG
images up to 640 × 360 pixels and 160 KiB. A camera card contains the image alone,
preserving its aspect ratio. The newest camera replaces the preceding camera;
other text cards may remain. Failed image retrieval uses the notification text.
Camera URLs and Home Assistant credentials stay on the controller.

Turning notifications off, entering standby, leaving the foreground, changing
the remote connection or closing the app cancels pending delivery and clears
cards. Expired or previous-session messages are not replayed after returning.

## Delivery and diagnosis

Notification polling is separate from the command queue, uses the paired device
credential and preserves a configured controller URL prefix. It follows the
controller's version 1 notification and preference contracts. Feed polling is a
destructive read: a second diagnostic client must not poll it, because it would
consume messages intended for the television.

Use `ott ha status` to inspect the bridge and recipient state. An active recipient
and increasing queued/polled counters establish transport progress; they do not
prove that a banner was visible. Verify display on the television separately.
An isolated test event must use a controller rule explicitly targeted at the
intended player. A `targets` field in an arbitrary Home Assistant event does not
override a rule's recipient selection.

Host tests cover contract parsing, bounded decoding, session retirement,
preference persistence and overlay rendering. ARM compilation and physical-TV
acceptance are separate checks; see [installation and acceptance](ott-native-delivery.md).
