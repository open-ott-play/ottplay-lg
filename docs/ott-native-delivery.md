# Installation and acceptance

This repository starts from native source, not an accepted binary. A package
version alone does not identify the source running on a television. Compare its
executable and package checksums with the matching build receipt.

The native application ID is `com.openottplay.player.native`. Retain this ID for
updates. Do not uninstall to update: uninstalling can remove profiles, Favorites
and pairing state.

## Package requirements

A distributable package needs a clean source commit, pinned SDK/toolchain,
release-mode compilation, ARM/link/package checks, an IPK checksum and complete
matching source/dependency notices. Companion-enabled packages additionally need
the helper's verified source, binary and linker evidence.

No binary release from this new repository is currently qualified. Historical
upstream downloads are not substitutes for a new OTTPlay build.

webOS package metadata keeps a numeric `X.Y.Z` version. Debug builds therefore
may show the same package version in Dev Manager even when their code differs.
The runtime/CLI version identifies the development or candidate line, and the
player also reports its short Git build SHA so a running build can be matched to
its source. Release artifacts must bind that SHA to the package checksum and
toolchain receipt; do not put a Git suffix in `appinfo.json` or the IPK filename.

## Developer Mode

For a retail TV, use LG's webOS CLI and the TV's Developer Mode app. Pair the
TV once with `ares-setup-device`; use the `prisoner` account and port `9922`
shown by Developer Mode. Keep the passphrase and generated key in the CLI's
local device configuration; do not put either in shell history or this repo.

After pairing, install and launch the selected flavor with:

```sh
make ARES_DEVICE=living-room FLAVOR=debug install-devmode
```

`ARES_DEVICE` is the alias registered by `ares-setup-device`. The target builds
the selected IPK, installs it over Developer Mode, then launches its matching
app ID. To install the stable ID, explicitly use
`FLAVOR=stable RELEASE=1`; the standard release/package gates still apply.
The existing `make install` remains the separate root-SSH/Luna route for rooted
development TVs. Neither path uninstalls first, so updating the stable app keeps
its app data and login state. Builds do not pair devices or read passphrases.

Direct CLI equivalent, after the package has been built:

```sh
ares-install --device living-room pkg/com.openottplay.player.native.debug_VERSION_arm.ipk
ares-launch --device living-room com.openottplay.player.native.debug
```

LG documents the CLI install and launch flow in its [CLI Developer Guide](https://webostv.developer.lge.com/develop/tools/cli-dev-guide).

## Build and release candidates

The native candidate workflow builds a stable ARM IPK from one exact `main`
commit on an ARM runner. Dispatch it with that same full commit SHA; the workflow
rejects any other SHA. It verifies pinned dependencies and SDK, package/link
contracts, then uploads the IPK, checksums, linker evidence, and a provenance
receipt as one immutable artifact. The receipt records the source tree, package
hash, build configuration, and toolchain identities.

`Draft native webOS release from candidate` consumes a candidate by run ID and
attempt. It verifies the workflow run, source SHA, package version, checksums,
and linker evidence, and never rebuilds the IPK. It only creates a GitHub draft
from the already-built bytes, and only when a version tag already points to the
candidate's exact source commit. It does not create tags or publish a release;
maintainer review and publication remain manual. Candidates cannot be promoted
until the version's release audit/notes are present (or release configuration
explicitly routes those checks), and the ARM candidate workflow has passed.

The existing host CI is not a substitute for this ARM package candidate. A
successful host/hostsim build alone does not qualify an IPK for TV installation
or release.

## Homebrew Channel

`docs/webosbrew-package.yml` is the current OTTPlay package-file template for
the webOS Homebrew Channel repository. Its stable manifest URL is designed to
follow GitHub's latest stable release. Generate the matching manifest from a
qualified stable release package with:

```sh
make FLAVOR=stable RELEASE=1 hbc-manifest
```

The target refuses debug/nightly packages. Attach the generated
`com.openottplay.player.native.manifest.json` and the exact matching IPK to the
same GitHub release, then submit the YAML file to
[`webosbrew/apps-repo`](https://github.com/webosbrew/apps-repo) under
`packages/com.openottplay.player.native.yml`. Keep the icon URL public and stable.
The HBC repository stores listing metadata and a pointer; the release hosts the
IPK and its hash-bearing manifest.

This checkout still has no qualified public stable release, so the HBC listing
is prepared but not submitted or live. A debug build (including a debug build
installed under the stable app ID) must never be attached to a public HBC
release. HBC submission requires package compatibility review and installation
and launch on a real TV; the project does not claim that qualification from a
desktop build. See the current [Homebrew publishing guide](https://www.webosbrew.org/develop/guides/publishing/how-to/).

## Physical-TV acceptance

Record model, firmware, source revision and package hash. Verify startup,
sustained audio/video, seek, track changes, EOF progression, standby/resume,
remote commands, protected Save and cold restart. Verify both presentations with
the same profiles and Favorites. Mode changes must not reopen media.

Host tests and screenshots qualify their specific contracts only. Keep missing
device observations pending instead of treating them as passes.

## Remote playback capture

The native player advertises the opt-in `native_playback_capture` capability
for authenticated remote diagnostics. A caller starts a capture for the active
playback, reads its bounded samples, and can stop it early. The default window
is 30 seconds; callers may request 5–60 seconds. The ring retains at most 64
one-second samples. A source/session change ends the capture; an internal
stream reopen does not.

The capture is held only in memory and is discarded when a new capture starts
or the app exits. It does not record or return URLs, paths, channel or profile
names, titles, playback position/duration, window IDs, or raw platform log
messages. The exported sample schema contains bounded counters, dimensions,
status codes, and closed state enums only. The existing passive playback
diagnostic remains separate; capture is not started implicitly by status or
capability discovery.

Remote command use requires a CLI and relay that advertise and validate
`native_playback_capture`; older controllers must continue using their existing
passive diagnostics path. Do not infer that the remote capture flow is ready
until the controller update has shipped and a real capture has been read back.
