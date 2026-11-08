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

## Developer Mode

Enable Developer Mode and configure the webOS CLI locally. For a qualified IPK:

```sh
ares-install --device YOUR_TV ottplay-lg-VERSION.ipk
ares-launch --device YOUR_TV com.openottplay.player.native
```

These are operator commands. Builds never execute them automatically. Do not
commit device configuration, SSH keys or access codes.

## Homebrew Channel

A future feed must reference the same verified IPK and version metadata used by
Developer Mode. This source bootstrap does not publish a Homebrew feed. A hosted
player feed is not a native-app update channel.

## Physical-TV acceptance

Record model, firmware, source revision and package hash. Verify startup,
sustained audio/video, seek, track changes, EOF progression, standby/resume,
remote commands, protected Save and cold restart. Verify both presentations with
the same profiles and Favorites. Mode changes must not reopen media.

Host tests and screenshots qualify their specific contracts only. Keep missing
device observations pending instead of treating them as passes.
