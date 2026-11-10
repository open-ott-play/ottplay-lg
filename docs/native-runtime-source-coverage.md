# Sources for incorporated SDK runtime inputs

`ci/native_runtime_sources.py` supplies the runtime source evidence that the
upstream companion collector intentionally leaves to its enclosing release.
It uses the final IPK and byte-bound linker evidence, rather than choosing source
versions from library names or declarations alone.

The committed policy in `ci/native-runtime-sources.json` pins all accepted SDK
archives, the native-toolchain and Buildroot recipe snapshots, the glibc source
revision, and the GCC source archive. Source files retain their upstream notices;
Buildroot's patches and configuration recipes are included in its snapshot.
Changing an SDK version or archive hash requires reviewing these source pins.

The producer verifies every packaged ELF against its staged link receipt,
map and trace. It checks the actual compiler and runtime input bytes against the
checksum-pinned SDK archive. GCC startup objects and static runtime archive
inputs map to GCC sources; glibc startup/nonshared inputs map to glibc sources.
Unknown SDK static objects fail instead of receiving an assumed exception.
Shared SDK libraries are recorded separately and must not be copied into the
IPK. Explicit SDK aliases support retained local linker evidence, with actual
alias bytes checked against the selected SDK and its original archive.

Example acquisition and collection (all output directories must be fresh):

```sh
python3 ci/native_runtime_sources.py fetch --source-dir /tmp/runtime-downloads
python3 ci/native_runtime_sources.py collect \
  --source-dir /tmp/runtime-downloads --package pkg/com.openottplay.player.native_0.4.0_arm.ipk \
  --evidence pkg/link-evidence/stable --sdk "$WEBOS_SDK" --sdk-archive /path/to/pinned-sdk.tar.bz2 \
  --output /tmp/runtime-coverage
python3 ci/native_runtime_sources.py verify \
  --source-dir /tmp/runtime-coverage --package pkg/com.openottplay.player.native_0.4.0_arm.ipk \
  --sdk-sha256 '<exact approved SDK archive SHA-256>'
```

A standalone runtime record can report `sdk_runtime_source_coverage: PASS` while
`complete_corresponding_source` remains false: it describes the SDK inputs and
sources, not the application's or helper's other dependencies.

After a helper has been built and staged, the candidate workflow runs
`native_companion.py qualify-sources` against the final package. It checks the
packaged helper receipt and source identity against its locked-source supplement,
collects the runtime set, and creates a new combined companion source archive.
The outer archive contains the original locked source archive and its original
pending closure, the runtime sources/recipes and coverage record, and extraction
instructions. The producer independently stream-verifies every outer member and
its manifest hash without extracting archive paths.

Only this combined, hash-bound companion source set declares complete companion
coverage. Its scope does not attest unrelated application dependencies or a
bit-identical rebuild. Pending precursor records are preserved unchanged. A
wrong final IPK, wrong helper ELF, changed runtime/source bytes, an unapproved
SDK archive or a changed recipe pin prevents completion.

The main native source collector can include the same verified runtime set:

```sh
python3 ci/collect-source-inputs.py --archive-root . --output /tmp/native-source-inputs \
  --runtime-coverage /tmp/runtime-coverage \
  --runtime-package pkg/com.openottplay.player.native_0.4.0_arm.ipk
```

This augments its existing application/Cargo/Rust source collection; it does not
replace those inputs. Without runtime evidence its SDK exclusions stay blocked.
Physical playback, firmware behavior and reproducible-build validation remain
separate checks and are not inferred from source collection.
