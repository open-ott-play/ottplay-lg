# OTTPlay Home and network tools

The portable TV/desktop dashboard contains selected Home Assistant readings,
explicit scene/light controls, configured apps/HDMI sources and **manual**
network checks. It is an optional screen; it does not replace playback controls,
change providers, weaken kiosk policy or start a network benchmark at boot.

## Components

- `dashboard/`: five local ES5 runtime assets and isolated browser-controller
  tests. No CDN, HA token or packaged client key. The browser uses the client key
  only for the configured Home bridge; synthetic network checks never receive it.
- `bridge/`: optional Node.js service beside an existing OTT server or another
  trusted server. Configure the selected HA entities/actions/TV sources there.
  See its README. The HA token stays on that host; the dashboard uses a different,
  revocable client key. This is distinct from the existing HA integration which
  controls OTT playback.
- `network-service/`: optional LG-only read-only TCP service. It does not depend
  on the global Home-key interceptor. Browser-only and desktop clients show this
  measurement as unavailable rather than measuring from another computer.
- `package-webos.cjs`: offline fixed-file staging and optional local IPK packaging.
  It never installs or launches on a television. The image is the repository's
  existing `pkg/icon.png`; its existing licensing applies.

## Standalone browser/server use

Configure and run the bridge according to `bridge/README.md`, then open its
trusted HTTPS address. It serves the dashboard itself. Alternatively serve the
five dashboard assets together and allow that exact origin on the bridge.
Enter the bridge address and its client key in **Connection**. The key lives only
in this page session; it is not a Home Assistant token and is not exported in an
OTT profile or embedded in an IPK. Closing/reloading this page requires reconnecting.

Home controls are admitted only after a current authenticated snapshot. Actions
are never automatically retried: a lost acknowledgment may still have operated
the light/TV. Subsequent status is the observation. An app/HDMI tile uses the
configured TV's exact current HA `source_list` label; unlisted targets are refused.

The shared player adapter presents these assets in an owned, closeable settings
panel and blocks entry during kiosk/standby. That adapter and its native-shell
asset staging must be delivered with the matching player build. Serving these
files alone does not add a menu item to an older installed player.

## LG package, without root

With the webOS CLI installed, choose a **new** output directory:

```sh
node package-webos.cjs --out /private/new-stage --dashboard dashboard \
  --network-service network-service --enable-network-probes --package
```

Omit `--package` for source staging only. Omit `--enable-network-probes` to keep
the TCP service disabled; HTTP checks remain available. The flag permits only
manual bounded probes, never background polling. No helper, credentials or private
settings are included. The manifest binds all staged public bytes.
Inputs must be single-link regular files of at most 1 MiB, with no hardlink or
symlink in a file or symlink in its
parent directories. Use canonical paths, including the output parent; an existing
or dangling output entry is rejected. Reads bind a bounded descriptor to the
admitted file identity, and stage files are created exclusively. This excludes
adjacent private files and symlink targets; it does not redact secrets manually
inserted into one of the five public runtime assets.

Install the generated `com.openottplay.home_1.0.0_all.ipk` through your existing
Developer Mode/Dev Manager workflow. It is a **separate optional app**, OTTPlay
Home; it does not overwrite OTTPlay Native or install the global Home interceptor.
Developer Mode still requires renewal. The optional JavaScript service uses the
TV's built-in `webos-service` module. Real permission/firmware support must be
verified on the target; local packaging success does not prove physical TV operation.

On a TV, Back from Home requests the fixed installed OTTPlay Native app. Ordinary
browsers have no native launch authority. The optional global Home-button package
is documented separately in `integrations/webos-home-helper`; it defaults paused
and needs the per-TV probe and physical controls acceptance before enablement.

## Network checks

- **NAS/proxy HTTP check:** enter an origin, select only `/`, `/health` or
  `/identity`, and run a bounded HEAD check. An HTTP status (including 401/404)
  proves an endpoint replied, not that a stream works. CORS, DNS, TLS or local
  network permission failure is unknown, not proof that the NAS is down.
- **Cloudflare HTTP check:** starts only by pressing Start, stops on Cancel,
  Back, hide/offline or leaving the tab. The fixed plan uses about 12.2 MB,
  individual payloads at most 5 MB, and an overall 60-second deadline. HTTP RTT
  and transfer speed are distinct from the player's measured media bitrate.
  Cloudflare receives the test connection/IP; no gateway key is attached.
- **LG TCP check:** explicit router/internet :443 buttons, private current IPv4
  gateway or a configured fixed public endpoint, bounded requests and a minimum
  15-second start interval. It measures TCP connection establishment, not ICMP
  ping. A closed port does not establish network failure.

No real network benchmark, TCP probe, HA action or TV operation is part of the
offline tests. To check the components locally:

```sh
node --test dashboard/*.test.cjs bridge/server.test.cjs package-webos.test.cjs
```

The Home Assistant startup/renewal tooling is in the parent directory. Keep an
existing session renewer instead of enabling a second one.
