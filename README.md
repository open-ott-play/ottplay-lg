# OTTPlay LG

Native OTTPlay for LG webOS, using SDL2/OpenGL ES, Rust and a small C platform
layer. Classic and Modern present the same providers, catalogues, Favorites and
playback state. This application runs directly on the television.

**Development repository.** The imported source includes both presentations and
major provider families, but full shared-player parity and physical-TV acceptance
have not been established. This repository has not published an installable
release. See [the migration plan](docs/port-plan.md).

## Source and architecture

The starting tree is the native integration from `open-ott-play/ottplay-foss`
commit `47adabad66b41b33e126925f7f00b17c7bf19b58`, derived from
[PlxNative](https://github.com/GLinnik21/plx-native) commit
`aef53a3f6148e41378bb21abd527518a642d733a`. Its 38 integration patches are now
ordinary source files. Their checksums and the prepared tree are recorded in
[SOURCE-PROVENANCE.json](SOURCE-PROVENANCE.json).

- `rust-modules/`: application state, UI, providers, playback and storage.
- `src/`, `include/`: native boot and platform/media interfaces.
- `assets/`, `locales/`: artwork, fonts and translations.
- `pkg/`, `ipkroot/`: webOS package inputs.
- `ci/`, `tests/`, `scripts/`: host checks, device fixtures and build support.

Domain decisions belong to the canonical
[OttPlay core](https://github.com/open-ott-play/ottplay-core). Platform adapters
own network, storage, input and decoder effects. Both SDL presentations consume
the resulting state. Existing native integrations are moving to this boundary
incrementally; not every domain decision has moved yet.

## Build and check

Use Python 3.12 or newer and Node.js 22 for the checks. Rust is pinned in
`rust-modules/rust-toolchain.toml`. Host tests need SDL2 and
SDL2_ttf development libraries; Linux also needs OpenGL, Wayland and GLib.
ARM packages use the pinned webOS SDK and host libclang to generate bindings
from the same QuickJS headers and target ABI as the compiled C library. Install
`libclang-dev` on Linux or LLVM on macOS; `--libclang` selects an explicit library
when it is outside standard locations. The build receipt records its checksum.
The pinned ARM SDK runs on macOS arm64/x86_64 and Linux aarch64. Linux x86_64
supports host tests but has no SDK asset for this pin. Package icon validation
also needs Pillow in the Python environment used for the build.

From the repository root, run the complete host check with:

```sh
python3 scripts/lg-build.py check --work-dir "$HOME/.cache/ottplay-lg/check" --jobs 2
```

For a clean committed source revision, prepare the SDK, compile and package an
ARM candidate:

```sh
python3 scripts/lg-build.py all --work-dir "$HOME/.cache/ottplay-lg/candidate" --jobs 2
```

Use a fresh candidate directory for each source revision. `--sdk` can reuse an
existing verified SDK; `--help` lists the optional companion and discovery
inputs. Host checks, SDK preparation, ARM compilation and packaging are separate
operations; `all` runs the latter three with source preparation.
Build commands never install or launch the application. Keep credentials and
television addresses outside the repository.

The optional media companion has separate build/source requirements. A package
without a qualified companion reports those capabilities unavailable. Source
presence alone does not establish Torrent or plugin playback on a television.

## Installation and releases

The native application ID remains `com.openottplay.player.native`, preserving
the installation identity for future qualified updates. The hosted launcher
uses a different application ID.

Install only a qualified IPK with its checksum, exact source revision, build
receipt and matching source distribution. Developer Mode and Homebrew delivery
must use the same verified package. See
[installation and acceptance](docs/ott-native-delivery.md).

## License

GPL-3.0-or-later. Original PlxNative copyright and earlier MIT grants remain
preserved. See [LICENSE](LICENSE), [LICENSING.md](LICENSING.md),
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) and [licenses](licenses/).
Shared-core artifacts retain their own license/provenance. Proprietary television
firmware libraries are not distributed here.
