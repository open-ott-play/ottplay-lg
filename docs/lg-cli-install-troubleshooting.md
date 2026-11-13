# Installing an OTTPlay IPK with the LG webOS CLI

This runbook covers installing or updating OTTPlay on a retail LG TV through
the TV's Developer Mode app. It records the working steps and the CLI issue
observed during the 0.3.3 installation on 2026-11-08. The CLI workaround is
version-specific; try the unmodified tool first.

## Before connecting

On the TV, open **Developer Mode**, confirm **Dev Mode Status** is on, and turn
on **Key Server** while pairing or retrieving the key. Keep the Developer Mode
session active. The passphrase shown by the TV is a secret: enter it only at a
prompt, and do not put it in shell history, screenshots, logs, or this repo.

On the Mac, check the configured device aliases:

```sh
ares-setup-device --list
```

If the TV has not been configured, run `ares-setup-device` and add it using the
TV's current address, user `prisoner`, port `9922`, and Developer Mode
authentication. Save the TV under a stable local alias. Do not assume the
configured `default` is the TV you intend to update; pass its alias explicitly
to every CLI command.

Retrieve or refresh the Developer Mode key with the CLI prompt:

```sh
ares-novacom --getkey -d <tv-alias>
```

Enter the passphrase from the TV when prompted. The CLI keeps the key in its
local configuration; do not copy it into the project.

Check that the TV is reachable from the Mac on the same network:

```sh
nc -vz <tv-address> 9922
```

A successful TCP connection confirms reachability only. It does not prove that
the key is valid or that the TV will accept an install.

## Install and verify

Build the intended IPK first. For the repository's Developer Mode target, use
the TV alias explicitly, for example:

```sh
make ARES_DEVICE=<tv-alias> FLAVOR=stable RELEASE=1 install-devmode
```

For a previously built package, the equivalent direct commands are:

```sh
ares-install <absolute-path-to-ipk> --device <tv-alias>
ares-install -F -d <tv-alias>
ares-launch --device <tv-alias> com.openottplay.player.native
```

Check the full package listing for the OTTPlay app ID and expected version.
The plain `ares-install --list` output reports IDs only, so use `--listfull`
when confirming a version. Keep the application ID unchanged when updating.
Installing over the existing app is an update; do not uninstall first, because
uninstalling can remove profiles, Favorites, and pairing state.

## Optional SSH config for manual diagnostics

An entry in `~/.ssh/config` makes manual OpenSSH checks easier by giving the TV
a short alias and scoping its legacy host-key setting to that TV. Replace the
placeholder address locally; do not commit a real device address:

```sshconfig
Host lg-ott
    HostName <tv-address>
    User prisoner
    Port 9922
    IdentityFile ~/.ssh/lg_webos
    IdentitiesOnly yes
    HostKeyAlgorithms +ssh-rsa
    PubkeyAcceptedAlgorithms +ssh-rsa
```

Then a manual connection can use `ssh -v lg-ott`. The `ssh-rsa` options are
needed only if that TV offers the legacy RSA host-key/signature algorithm; keep
them under this one `Host` entry instead of weakening SSH settings globally.
On the Mac used for the documented installation, `ares-novacom --getkey`
stored the key at `~/.ssh/lg_webos`; use the actual key path shown in your
local device configuration if it differs. This is a private key: never paste
its contents into chat or commit it. If OpenSSH reports an authentication
failure, check that `IdentityFile` points to the key retrieved from the TV and
that it is unlocked in the SSH agent as needed.

This config helps diagnose TCP, host-key negotiation, and manual SSH
authentication. **It does not configure `ares-install` or fix its SSH2
library**: the webOS CLI uses its own Node SSH implementation and does not
consume OpenSSH's `~/.ssh/config` settings.

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| The command reaches the wrong TV or says the device is unknown | Run `ares-setup-device --list`; pass the intended alias with `--device` / `-d` instead of relying on `default`. |
| TCP port 9922 is closed or times out | Confirm the TV is on, Developer Mode is enabled, the Mac and TV can reach each other on the current network, and the address is current. Turn on Key Server for pairing/key retrieval. |
| `no matching host key type found; their offer: ssh-rsa` from OpenSSH | Scope `HostKeyAlgorithms +ssh-rsa` to the TV alias in `~/.ssh/config`. This is an OpenSSH diagnostic setting and does not alter the TV or make the webOS CLI use OpenSSH. |
| Key retrieval/authentication fails | Reopen Developer Mode, enable Key Server, re-read the current passphrase, and run `ares-novacom --getkey` for the correct alias. Never put the passphrase in a command line. |
| `ares-install` throws `Callback was already called.` | Retry once with the stock CLI after checking the selected Node runtime and device alias. On the observed `@webos-tools/cli` 3.2.6 installation, the error persisted under Node 22 and Node 24; Node 16 was rejected by that CLI with a `>=20.0.0` requirement. This points to a CLI/library callback issue, not a TV reachability failure. |
| `ares-install` still fails after key retrieval succeeds | Do not repeatedly reinstall or remove the app. Capture the exact error, CLI and Node versions, and check the package/IPK path. `ares-novacom --getkey` succeeding proves key retrieval, not package installation. |

### Recovering after a Developer Mode reset

A Developer Mode session reset can remove the installed development application.
After signing in again and restarting the TV, check the installed application
list before assuming the package is still present. Restore the verified IPK with
the **same application ID**, then verify its version and launch it once.

Do not delete or recreate profiles preemptively. In the recovery observed on
2026-11-13, the application was absent, but reinstalling the same primary ID
restored the saved profiles and remote-control identity. This is an observed
recovery, not a guarantee that every reset preserves data. Confirm both the
profile list and remote readiness before importing a backup or pairing again.

The launcher name is `OTTPlay`, including localized manifests. The internal ID
`com.openottplay.player.native` is a storage/update identity; its `native` suffix
is not the launcher title and should not be changed for a display-name update.

### Last-resort callback workaround observed on 2026-11-08

For the exact `Callback was already called.` failure above, installation of the
0.3.3 IPK succeeded using a **temporary isolated copy** of the installed
`@webos-tools/cli` 3.2.6, run with Node 22.23.4. The isolated copy was kept
outside the repository. For future attempts, use `~/.codex/artifacts` and
remove it afterward. It had two narrow compatibility changes:

1. In `lib/base/novacom.js`, restrict the SSH2 `serverHostKey` algorithms to
   `['ssh-rsa']` for this Developer Mode TV, alongside the existing KEX list.
2. In the copy's `node_modules/async/dist/async.js`, change `onlyOnce` so a
   duplicate callback is ignored (`if (fn === null) return;`) instead of
   throwing `Error("Callback was already called.")`.

The package installed and `ares-install -F` reported the expected app
ID and version. The global npm installation was not modified, and the
temporary copy was removed afterward. Treat this only as a narrowly tested
diagnostic workaround: do not patch the global CLI, do not apply it to other
errors, and re-check the source and behavior when the CLI or its dependencies
change. Prefer an upstream CLI fix once available.

The LG CLI's Node-version guidance and the installed package's runtime check
did not agree in this incident: LG's web documentation listed Node 14/16,
while the installed CLI rejected Node 16 and required Node 20 or later. Check
the requirements of the exact CLI version being used instead of downgrading
Node based on a generic guide.

## What a successful install proves

A successful CLI install plus a full package listing proves that the IPK was
accepted and the expected app version is installed. It does not prove playback
or remote-control behavior. Verify app launch and the TV-specific acceptance
items in [native delivery](ott-native-delivery.md#physical-tv-acceptance).
