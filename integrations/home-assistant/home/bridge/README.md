# Optional OTT Home bridge

This small Node.js 22.4+ service keeps the Home Assistant token on the server.
The common dashboard receives only configured readings, labels and opaque action
IDs. A separate random client key authorizes those selected operations. No root
or additional HA integration is needed for the dashboard. App/HDMI actions use
the TV already paired to HA's LG integration and require the exact source label
in that TV entity's current `source_list`.

Copy `config.example.json` to a private directory, replace the examples, and set
its mode to 0600. Generate a client key with
`node -e "console.log(require('node:crypto').randomBytes(32).toString('base64url'))"`.
Use a dedicated HA account and keep its token only in this private server file;
never place it in the dashboard, a player profile, an IPK or version control.
The selected UI operations do not limit what the raw HA token can do, which is
why it stays on the server. Restart the bridge to apply a configuration change.

```sh
OTT_HOME_CONFIG=/private/path/config.json node server.cjs
```

The default listener is `127.0.0.1:8790`. It serves the adjacent `../dashboard`
assets and the API. Set `OTT_HOME_ASSETS` only if using another asset directory.
For remote clients, place a trusted HTTPS reverse proxy in front of this
listener, add each actual player origin to `allowedOrigins`, and configure that
HTTPS URL plus the **client key**, not the HA token, in the dashboard. A hosted
HTTPS player cannot contact an untrusted HTTP LAN service through mixed-content
restrictions. Certificates remain verified; no TLS-bypass option exists.

`allowPlainHttp` is an explicit operator exception for a trusted network, not a
default. The dashboard independently asks before accepting private-LAN HTTP.
Do not expose the plain listener to the Internet. No wildcard CORS is accepted.
Changing ports, binding on LAN, HA credentials and reverse-proxy deployment are
operator configuration; starting this source does not change an existing NAS,
HA configuration or television. IPv4/VLAN routes and browser local-network
permission still have to permit the television/client to reach this service.

Some packaged TV WebViews send the opaque Origin `null`. To use those clients,
explicitly set `allowPackagedOrigin: true` and add the literal `null` to
`allowedOrigins`. This cannot distinguish one opaque-origin application from
another; the separate strong client key is still required. Prefer a separate
bridge/key for a television with only its necessary actions. This exception is
off by default and does not permit wildcard origins.

The optional Dockerfile is built from the parent integration directory:
`docker build -f bridge/Dockerfile -t ottplay-home .`. Mount the private config at
`/run/ott-home/config.json:ro`; ensure the non-root container user (UID 1000) can
read it while its mode remains 0600. Publish the port only to your trusted reverse
proxy. No dependencies are installed at runtime.

## Contract and lifecycle

- `GET /api/home`: version-one `available`, selected entities, controls and app
  IDs. Raw HA entity IDs, token and unrelated HA entities are not returned.
- `POST /api/action` with exactly `{"id":"configured-id"}`: dispatches the
  configured fixed service/entity or source. Caller-supplied entities/services
  and arbitrary application URLs are rejected. The reply means requested, not
  physical device acceptance.
- Both require `Authorization: Bearer <client-key>`. A present Origin must match
  the configured allowlist. Bodies are capped at 1 KiB; actions are single-flight.
- HA WebSocket state is generation-scoped, selected entities only. Reconnects
  back off to 30 seconds, pending calls are capped at eight with 8-second limits.
  Snapshots older than 45 seconds cannot authorize actions. Concurrent snapshot
  refreshes share one request per connection, and refreshes preserve newer state
  events and deletions rather than overwriting them with an older snapshot.
- Static dashboard files contain no credential and can be served publicly.
  Fixed assets must be single-link regular files of at most 1 MiB. Hardlinks
  are rejected. Symlinks in an asset or
  any parent directory are rejected; use canonical paths (for example `/private`
  instead of the macOS `/var` alias). Descriptor and path identities are checked
  around each bounded read.
  No background speed tests or TV wake/launch commands run on bridge startup.

Run `node --test server.test.cjs`. These are isolated fake-HA/HTTP-handler tests;
they do not call Home Assistant or a television. Deployment and actual device
acceptance are separate checks.
