# Optional Home Assistant setup for OTTPlay on LG

This directory generates a startup blueprint and optional Developer Mode session-maintenance package. Python 3.9+ and its standard library are sufficient. Generation is offline: it does not install anything, contact LG/HA, launch a TV app or change Developer Mode. No root is required. Install OTTPlay and pair the LG webOS integration in Home Assistant separately.

## Generate configuration

Copy `examples/settings.json` to a private working directory, choose a unique `id` for each TV, then run:

```sh
python3 generate.py generate /path/to/settings.json /path/to/new-output
```

The output directory must not exist. Existing user configuration is never overwritten. Fields and limits are described in `settings.schema.json`; unknown fields, including credentials, are rejected. The ID supplies unique command, helper, automation and notification names. Do not reuse an ID for a second TV.

The default `renewal_mode: external` deliberately generates **no renewal automation or REST commands**. Keep an existing renewer such as Dev Manager, Camera Overlay or another HA package. Select `manual` for LG's manual EXTEND workflow. Choose `managed` only if no other service renews this television's shared Developer Mode session. Startup can be used independently of renewal.

## Once after TV startup

The checked-in `ott_startup.yaml` is identical to the generated blueprint. Copy it to `config/blueprints/automation/ottplay/ott_startup.yaml`, reload automations/blueprints as appropriate, and create an automation from **OTTPlay once after LG TV startup**.

Select the LG entity and enter the exact, case-sensitive installed app name from that entity's `source_list` (it might differ from the default OTTPlay). Set a protected app ID if a camera overlay or another app must never be interrupted. The automation checks that the requested source exists and is not already selected.

It acts once on an observed `off` → `on` transition, after the TV remains on for the configured 1–30 seconds (default 3). The preceding off period must last 30–600 seconds (default 60). Brief false-off reports, `unknown`/`unavailable` reconnections and HA restarts do not trigger it. There is one `select_source` action, no retry loop and no watching the active app: switching to HDMI or another app afterward is respected. It never powers the TV on.

A long connection outage reported by HA as off can still look like a power cycle. Quick genuine standby cycles below the minimum off interval intentionally require opening OTTPlay manually. Stable-on waits do not survive an HA restart. Application-launch acknowledgment is not proof that OTTPlay decoded video. Test this behavior on your own LG integration before relying on it.

## Optional verified Developer Mode renewal

For `managed`, generation adds `ott_lg_<id>.yaml` and `secrets.example.yaml`. The former is a complete HA **package**, not an automation-UI snippet. Copy it into your configured HA packages directory. If packages are not enabled, merge the following under the existing `homeassistant:` entry; do not duplicate or replace that entry:

```yaml
homeassistant:
  packages: !include_dir_named packages
```

Copy the two placeholder secret **keys** into HA's private `secrets.yaml`. Obtain the personal renewal URL from webOS Dev Manager → Info → Renew automatically → Manual Setup. The renewal endpoint is `https://developer.lge.com/secure/ResetDevModeSession.dev?sessionToken=...`. For the second URL change only `ResetDevModeSession.dev` to `CheckDevModeSession.dev`, retaining the same private session token. Do not put either URL in settings JSON, Git, a chat, a profile or a public uptime monitor.

Optional offline validation accepts a private JSON object with exactly `renew` and `check` URL fields on stdin. It checks the exact HTTPS hostname/path and matching tokens, emits only a closed result and makes no request:

```sh
python3 generate.py validate-private-urls < /private/path/urls.json
```

This is format validation, not proof the token works. `secrets.yaml` separates credentials from shared configuration but does not encrypt them. HA traces, REST errors and backups can contain those URLs; keep them private. Never send credentials as shell command arguments.

Run HA's **Check configuration** and restart HA at a suitable time to load the package. The generated `input_boolean.ott_lg_<id>_renew_enabled` toggle starts off on first creation and restores explicit state on subsequent HA restarts. Turn it on only after confirming no existing renewer manages that TV. Disable it or choose external/manual mode to opt out; regeneration alone does not remove a previously installed HA package.

The automation runs daily at `daily_time` (HA's time zone) and on HA startup, with a one-minute settling delay. It checks the toggle again after that delay, then requests renewal once and independently checks remaining time five seconds later. Each request times out after 20 seconds; no retries or TV actions occur. Turning the toggle off cannot recall an already sent request; its read-only verification may finish.

Success requires both HTTP statuses 200, both JSON results `success`, a strict `hours:minutes:seconds` check response, and at least `minimum_hours` (default/minimum 168). A reset acknowledgment such as GNL alone is insufficient. Unknown formats, malformed JSON, short remaining time or transport failures lead to one persistent HA notification; later verified success dismisses it. This is a notification inside HA, not a phone alert. Add your own notification delivery separately if desired.

When ready, run the automation manually **in HA**, inspect the private trace and confirm the TV's Remain Session. Neither generator tests nor HTTP success prove that the TV synchronized its session. Keep checking after HA/internet outages. Expired Developer Mode may require re-enabling, pairing and reinstalling apps; this package cannot recover an expired session. Do not toggle Developer Mode off as troubleshooting: other development-installed apps may be removed.

To remove this package, first disable its renewal toggle/automation, remove only its package and two secret entries, check HA configuration and reload/restart as needed. Keep another renewal method or extend manually before expiry.

## Offline checks and limits

```sh
python3 -m unittest discover -s tests -v
```

The standard-library suite exercises policy cases, generated action shape, malformed settings/response rejection, unique per-TV IDs, source/overlay guards, exact LG URL validation and no-overwrite behavior. It does **not** run HA's YAML/Jinja engine, call LG endpoints or establish physical-TV acceptance. Generated YAML is the deliverable; HA's own configuration check and a supervised first run remain necessary.

References: [HA blueprints](https://www.home-assistant.io/docs/blueprint/schema/), [HA packages](https://www.home-assistant.io/docs/configuration/packages/), [REST response variables](https://www.home-assistant.io/integrations/rest_command/), [HA secrets](https://www.home-assistant.io/docs/configuration/secrets/), [restoring toggle state](https://www.home-assistant.io/integrations/input_boolean/), [LG Developer Mode](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app). See `NOTICE` for pinned GPL source provenance; this directory contains no native Home-key interception helper.
