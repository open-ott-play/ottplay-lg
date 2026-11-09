# Packaged native companion

The native candidate workflow builds the app-owned helper from the exact revision in `ci/companion-source.json`. Before running upstream build code it verifies the reviewed verifier files. The helper has its own pinned Rust preparation, Cargo cache and target directory; the native UI continues to use its existing toolchain. Both builds use the checksum-verified webOS SDK archive.

The workflow stages `bin/ottplay-companion`, its manifest, source/ELF receipts and dependency notices after the native application staging step and before IPK packing. It does not stage companion copies of the TV's libc/libdl/libgcc_s/libm/libpthread/librt shared libraries. The helper ELF admission policy checks its dynamic requirements against the existing webOS runtime ABI. This is package validation, not evidence of successful execution on a television.

Candidate provenance binds the packaged helper bytes, the upstream source revision and three additional release assets:

- `companion-source.tar.gz`: local project sources, the locked Cargo dependency supplement, matching Rust standard-library sources and rebuild instructions.
- `companion-source-manifest.json`: the source inventory and hashes.
- `companion-source-closure.json`: the helper ELF/link/source identities and source coverage status.

Those assets are downloaded and checksum-verified before publication, copied without rebuilding during stable promotion, and compared byte for byte after publication. Older candidates without a helper retain their original asset inventory. New native candidates require the helper and its source supplement; a missing or mismatched helper fails candidate assembly. Source archives have a one-GiB per-file bound and are verified as opaque archives rather than extracted during release promotion.

The supplement reports locked dependency coverage separately from final SDK runtime-source coverage. Its `complete_corresponding_source` remains false: the supplementary collector does not independently establish the enclosing native application's full source closure, SDK runtime exception coverage or a reproducible rebuild. Do not change these declarations to claim evidence that was not collected. TV firmware libraries already present on the device are not copied into the companion payload. Final packaging/link evidence remains available to identify statically incorporated SDK objects separately.

Stable promotion checks this coverage twice: during preflight and again in the
protected publishing job after environment approval. For a companion candidate,
the exact RC-bound closure must report `complete_corresponding_source: true` and
`sdk_runtime_source_coverage: "PASS"`. Missing, false or pending declarations
block stable promotion. Beta and RC publication may retain the honest pending
supplement. A candidate without an embedded companion retains its earlier
qualification rules. The workflow never rewrites a pending record as complete;
a changed source supplement requires a new candidate and qualification.

The current enclosing collector (`ci/collect-source-inputs.py`) still records
`glibc/GCC runtime source closure` and `LG firmware/sysroot` as blocked inputs.
Resolving them requires the final helper and application linker input inventory,
the exact pinned SDK archive/compiler identity, matching GCC/glibc/Buildroot
source and patch recipes, and evidence distinguishing incorporated objects from
firmware libraries already on the TV. Neither a toolchain archive checksum nor
the companion's locked Cargo source archive supplies that missing evidence.
Until the genuine enclosing coverage is collected, companion stable promotion
is deliberately blocked.

A successful build does not enable an unconditional Torrent entry. The application starts its owned helper under its existing foreground, persistence, resource-limit and capability checks. Torrent browsing becomes available only after the running helper reports the required capabilities. Device startup, P2P playback and resource behavior must still be exercised on the target LG.
