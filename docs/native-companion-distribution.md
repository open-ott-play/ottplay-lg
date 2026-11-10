# Packaged native companion

The native candidate workflow builds the app-owned helper from the exact revision in `ci/companion-source.json`. Before running upstream build code it verifies the reviewed verifier files. The helper has its own pinned Rust preparation, Cargo cache and target directory; the native UI continues to use its existing toolchain. Both builds use the checksum-verified webOS SDK archive.

The workflow stages `bin/ottplay-companion`, its manifest, source/ELF receipts and dependency notices after the native application staging step and before IPK packing. It does not stage companion copies of the TV's libc/libdl/libgcc_s/libm/libpthread/librt shared libraries. The helper ELF admission policy checks its dynamic requirements against the existing webOS runtime ABI. This is package validation, not evidence of successful execution on a television.

Candidate provenance binds the packaged helper bytes, the upstream source revision and three additional release assets:

- `companion-source.tar.gz`: local project sources, the locked Cargo dependency supplement, matching Rust standard-library sources and rebuild instructions.
- `companion-source-manifest.json`: the source inventory and hashes.
- `companion-source-closure.json`: the helper ELF/link/source identities and source coverage status.

Those assets are downloaded and checksum-verified before publication, copied without rebuilding during stable promotion, and compared byte for byte after publication. Older candidates without a helper retain their original asset inventory. New native candidates require the helper and its source supplement; a missing or mismatched helper fails candidate assembly. Source archives have a one-GiB per-file bound and are verified as opaque archives rather than extracted during release promotion.

The upstream locked-source supplement retains its original pending SDK declaration.
After final IPK packing, `ci/native_runtime_sources.py` checks the packaged ELF
hashes, linker map/trace hashes, compiler identity and incorporated SDK object
bytes against the exact pinned SDK archive. It collects checksum-pinned GCC,
glibc, Buildroot and native-toolchain sources, including patch/build recipes.
A changed SDK distribution requires reviewing its source provenance pins too.
Shared SDK libraries absent from the package are recorded separately from
incorporated startup objects and static archives; an unmapped static input fails.

`native_companion.py qualify-sources` combines the verified runtime source set
with the original locked helper source archive and creates a new outer archive,
manifest and closure. The original pending record is preserved byte for byte
inside that archive. Complete coverage refers specifically to the companion's
locked sources and its incorporated SDK runtime, bound to the final package and
helper ELF. It does not attest unrelated application components or a reproducible
rebuild. The outer manifest records that scope; the source payload is independently
stream-verified without extraction before candidate receipt assembly.

Stable promotion checks this coverage twice: during preflight and again in the
protected publishing job after environment approval. For a companion candidate,
the exact RC-bound closure must report `complete_corresponding_source: true` and
`sdk_runtime_source_coverage: "PASS"`. Missing, false or pending declarations
block stable promotion. Beta and RC publication may retain the honest pending
supplement. A candidate without an embedded companion retains its earlier
qualification rules. The workflow never rewrites a pending record as complete;
a changed source supplement requires a new candidate and qualification.

The enclosing application's source collector (`ci/collect-source-inputs.py`) can
consume this verified runtime set with `--runtime-coverage DIRECTORY` and
`--runtime-package FINAL.ipk`. It then includes the runtime source archives and
byte-bound coverage record with its separately collected application dependencies.
Without that supplied evidence, its runtime exclusions remain blocked. See
[native runtime sources](native-runtime-source-coverage.md) for commands and
precise coverage boundaries.

A successful build does not enable an unconditional Torrent entry. The application starts its owned helper under its existing foreground, persistence, resource-limit and capability checks. Torrent browsing becomes available only after the running helper reports the required capabilities. Device startup, P2P playback and resource behavior must still be exercised on the target LG.
