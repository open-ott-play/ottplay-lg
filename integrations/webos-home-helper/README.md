# Optional no-root Home helper

This is a separate, disabled-by-default Developer Mode package. It targets
`com.openottplay.player.native`. It does not change OTTPlay's playback code,
install itself, obtain root, replace LG Home, or create a firmware boot service.
No television has been qualified by the host tests in this directory.

A single prioritized foreground intent prevents a pending OTT launch from swallowing the long-Home escape; pause/failure clears deferred intents. The native worker preserves short Home → OTTPlay, approximately 2-second Home
hold → LG Home, and 8-second hold → release/pause. Input disagreement, lost
controller pipe or missed heartbeat releases the grab. `pause` must confirm both
recorded PID/start-tick identities are gone before another trial can start.
No competing input interceptor may run at the same time.

## Compatibility first

Only the reviewed 32-bit `arm-linux-musleabi` event ABI is supported. A 64-bit
kernel alone does not establish application ABI. Exact model, firmware, named
input devices, non-root native UID and shared GID must be checked for each TV.
Never fix a failed probe with global chmod, root, or changed system users.
Developer Mode still needs renewal. Firmware, package removal/reinstallation
or Developer Mode expiry can invalidate identity/permissions; repeat the probe.

Copy config.example.json privately and fill model, firmware, developerUid and
sharedGid from read-only device identity. Keep sourceName/destinationName only
if those exact, distinct device names are present uniquely. Defaults contain
no addresses, credentials or valid identity and cannot build an active helper.

## Build without installation

Node 22+ and Python 3.9+ are local tools. By default this only stages source:

```
node build.cjs probe /absolute/private/options.json /absolute/fresh/probe-stage
```

For an actual local ARM build, set absolute `ZIG` (exactly version 0.14.1),
`ARES_PACKAGE` (already-installed official ares-package) and `PYTHON`, then add
`--compile --package`. No dependencies are installed/downloaded automatically.
Outputs use a fresh directory and explicit package checksums. No ARM binary
is committed or published by these tools.

Install/launch the probe manually only when authorized, using the existing
paired webOS tooling. It has the SAME helper app ID as the eventual package,
so never install it over a running helper. The probe opens device handles and
reads identity/capabilities but never grabs or writes input events. It writes
only its own `state/identity.json`. Retrieve that file privately. Require a
non-root uid, expected shared gid, compatible:true, inputGrabbed:false and
eventsWritten:0, eventBytes:16, pointerBytes:4, and the exact configured input names. Set nativeUid to that verified uid, then stage/build:

```
node build.cjs helper /absolute/private/options.json /absolute/fresh/helper-stage --probe-receipt /absolute/private/identity.json
```

Again, `--compile --package` is explicit. App and control-service are packaged
together; state ownership is rewritten only inside those package directories.
Installing the same helper ID must retain the probed identity. If it does not,
the runtime refuses to intercept input; re-probe and rebuild.

## Explicit trial, then opt-in

commands.cjs PRINTS a command; it never connects to a TV. Execute the printed
command manually through your existing paired CLI only when ready:

```
node commands.cjs status
node commands.cjs trial 30
node commands.cjs pause
```

After trial request acceptance, `status` must report `active:true`, not merely
`starting:true`. Read status while the trial is active to retain its session ID.
With the physical remote check short Home, the 2-second LG escape, 8-second
release, other keys and normal pointer behavior. Verify missed-heartbeat and
controller-exit release on that model before permanent acceptance; pure host
tests alone do not qualify these device paths. Trial is capped at 120 seconds.
Pause, then require `released:true` (actual PID identity checks). An unresolved launch without a matching process report fails closed; it cannot mint another owner by waiting a guessed timeout. Recover/restart normally and re-probe rather than clearing state files by hand.

Only after physical acceptance, explicitly generate:

```
node commands.cjs enable --accept-tested-controls YOUR_32_HEX_TRIAL_SESSION
```

Enable rejects a session that was never observed active. It remains reversible
with pause or the 8-second escape. Optional external startup may call `ensure`
after explicit enablement; status and installation never start interception.
`ensure` refuses paused/unaccepted state. No HA automation is installed here.

If release is not confirmed, do not start another helper. Use the physical
8-second escape, then LG's normal restart/recovery and remove this optional
package before further trials. Existing LG Home and ordinary OTTPlay remain
the fallback. There is no promise of cold-boot/standby persistence on every TV.

## Host checks

`node tests/test.cjs` runs local configuration/lease/acceptance/staging checks
and compiles/runs only the pure C event filter using host cc. It does not build
ARM packages, install/launch apps, read TV input devices or contact any network.
`node tests/test-controller.cjs` and `node tests/test-service.cjs` execute the actual controller/service against VM/in-memory doubles, including delayed-launch escape and cancellation ordering. `python3 -B tests/test-package.py` checks only owned archive fixtures. These checks are distinct from package installation and physical TV acceptance.
