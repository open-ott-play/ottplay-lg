"""Byte-bound SDK source coverage; no native compiler/device or network required."""
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from ci import native_runtime_sources as runtime


def archive(path, members):
    with tarfile.open(path, 'w:gz') as tar:
        for name, data in members.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            tar.addfile(entry, io.BytesIO(data))


def ar_member(name, data):
    header = f'{name + "/":<16}{0:<12}{0:<6}{0:<6}{"100644":<8}{len(data):<10}`\n'.encode()
    return header + data + (b'\n' if len(data) % 2 else b'')


class RuntimeSourceTests(unittest.TestCase):
    def fixture(self, root, binary_name="plxnative"):
        sdk = root / 'sdk'
        sdk.mkdir()
        objects = {'bin/arm-webos-linux-gnueabi-gcc': b'compiler wrapper', 'usr/lib/Scrt1.o': b'glibc startup', 'lib/gcc/libgcc.a': b'gcc runtime',
                   'sysroot/lib/libc.so.6': b'firmware dynamic library'}
        for relative, data in objects.items():
            path = sdk / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        sdk_archive = root / 'sdk.tar.gz'
        archive(sdk_archive, {'sdk/' + name: data for name, data in objects.items()})
        binary = b'\x7fELFfixture-bound-application'
        data_tar = root / 'data.tar.gz'
        archive(data_tar, {'usr/palm/applications/com.openottplay.player.native/' + binary_name: binary})
        package = root / 'test.ipk'
        package.write_bytes(b'!<arch>\n' + ar_member('debian-binary', b'2.0\n') + ar_member('control.tar.gz', b'control fixture') + ar_member('data.tar.gz', data_tar.read_bytes()))
        evidence = root / 'evidence'
        evidence.mkdir()
        # Include a deleted ephemeral non-SDK input: no attempt to read it is permitted.
        trace = '\n'.join(str(sdk / path) for path in objects) + '\n/nonexistent/cargo/ephemeral.rlib\n'
        map_data = b'LOAD runtime objects\n'
        (evidence / (binary_name + '.link.trace')).write_text(trace)
        (evidence / (binary_name + '.link.map')).write_bytes(map_data)
        record = {'elf_sha256': hashlib.sha256(binary).hexdigest(),
                  'trace_sha256': hashlib.sha256(trace.encode()).hexdigest(),
                  'map_sha256': hashlib.sha256(map_data).hexdigest(), 'archive_excluded': True,
                  'compiler_sha256': hashlib.sha256(b'compiler wrapper').hexdigest()}
        (evidence / (binary_name + '.link.json')).write_text(json.dumps(record))
        sources = root / 'sources'
        sources.mkdir()
        (sources / 'runtime-source.tar.gz').write_bytes(b'fixture corresponding source archive')
        policy = {'toolchain_revision': 'a' * 40, 'buildroot_revision': 'b' * 40,
                  'glibc_revision': 'c' * 40, 'gcc_version': '12.2.0',
                  'sources': {'runtime-source.tar.gz': {'sha256': runtime.sha(sources / 'runtime-source.tar.gz')}}}
        return package, evidence, sdk, sdk_archive, sources, policy

    def collect(self, root, fixture):
        package, evidence, sdk, sdk_archive, sources, policy = fixture
        return runtime.collect(package, evidence, sdk, sdk_archive, sources, root / 'coverage',
                               policy=policy, allowed_sdk_hashes={runtime.sha(sdk_archive)})

    def test_actual_linked_sdk_objects_and_source_bytes_are_bound_to_package(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = self.fixture(root)
            package, evidence, sdk, sdk_archive, sources, policy = fixture
            result = self.collect(root, fixture)
            self.assertEqual(result['sdk_runtime_source_coverage'], 'PASS')
            self.assertFalse(result['complete_corresponding_source'])
            self.assertEqual(result['runtime_inputs']['usr/lib/Scrt1.o']['component'], 'glibc')
            self.assertEqual(result['runtime_inputs']['lib/gcc/libgcc.a']['component'], 'gcc')
            self.assertEqual(result['runtime_inputs']['sysroot/lib/libc.so.6']['kind'], 'dynamic-not-distributed')
            runtime.verify(root / 'coverage', package, expected_sdk_hash=runtime.sha(sdk_archive), policy=policy)
            package.write_bytes(b'changed package')
            with self.assertRaisesRegex(ValueError, 'identity mismatch'):
                runtime.verify(root / 'coverage', package, expected_sdk_hash=runtime.sha(sdk_archive), policy=policy)

    def test_modified_sdk_runtime_cannot_claim_approved_archive_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = self.fixture(root)
            (fixture[2] / 'lib/gcc/libgcc.a').write_bytes(b'changed runtime')
            with self.assertRaisesRegex(ValueError, 'differs from pinned SDK'):
                self.collect(root, fixture)

    def test_unmapped_static_object_requires_new_real_source_policy(self):
        with tempfile.TemporaryDirectory() as directory:
            sdk = Path(directory)
            (sdk / 'unknown.a').write_bytes(b'object')
            with self.assertRaisesRegex(ValueError, 'Unmapped SDK'):
                runtime.trace_inputs(str(sdk / 'unknown.a'), sdk)

    def test_missing_or_changed_source_is_never_relabelled_complete(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = self.fixture(root)
            (fixture[4] / 'runtime-source.tar.gz').write_bytes(b'not approved sources')
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                self.collect(root, fixture)

    def test_packaged_runtime_library_is_not_a_firmware_exclusion(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data_tar = root / 'data.tar.gz'
            archive(data_tar, {'usr/lib/libc.so.6': b'\x7fELFcopied libc'})
            package = root / 'bad.ipk'
            package.write_bytes(b'!<arch>\n' + ar_member('debian-binary', b'2.0\n') + ar_member('control.tar.gz', b'control fixture') + ar_member('data.tar.gz', data_tar.read_bytes()))
            with self.assertRaisesRegex(ValueError, 'redistributed'):
                runtime.packaged_elfs(package)


    def test_complete_companion_combines_real_bound_inputs_and_retains_original_pending_record(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = self.fixture(root, "ottplay-companion")
            package, evidence, sdk, sdk_archive, sources, policy = fixture
            coverage = self.collect(root, fixture)
            supplement = root / "locked"
            supplement.mkdir()
            (supplement / "companion-source.tar.gz").write_bytes(b"locked helper sources")
            (supplement / "SOURCE-MANIFEST.json").write_bytes(b'{"schema":1}')
            original = {"locked_source_coverage": "PASS", "complete_corresponding_source": False,
                        "sdk_runtime_source_coverage": "PENDING_ENCLOSING_RELEASE",
                        "elf_sha256": coverage["binaries"]["ottplay-companion"]["elf_sha256"],
                        "local_inputs_sha256": "a" * 64, "local_source_archive_sha256": "b" * 64,
                        "source_receipt_sha256": "c" * 64,
                        "source_archive": {"sha256": runtime.sha(supplement / "companion-source.tar.gz")},
                        "source_manifest_sha256": runtime.sha(supplement / "SOURCE-MANIFEST.json")}
            original_path = supplement / "source-closure.json"
            original_path.write_text(json.dumps(original))
            original_bytes = original_path.read_bytes()
            combined = runtime.complete_companion(supplement, root / "coverage", package, root / "combined", policy=policy)
            self.assertIs(combined["complete_corresponding_source"], True)
            self.assertEqual(combined["sdk_runtime_source_coverage"], "PASS")
            self.assertEqual(combined["precursor_closure_sha256"], runtime.sha(original_path))
            self.assertEqual(original_path.read_bytes(), original_bytes)
            with tarfile.open(root / "combined/companion-source.tar.gz") as tar:
                self.assertEqual(tar.extractfile("companion-locked-source-closure.json").read(), original_bytes)
                self.assertIn("runtime-source.tar.gz", tar.getnames())
            # Wrong ELF, changed runtime sources and changed package must each refuse.
            original_path.write_text(json.dumps({**original, "elf_sha256": "0" * 64}))
            with self.assertRaisesRegex(ValueError, "same packaged companion"):
                runtime.complete_companion(supplement, root / "coverage", package, root / "bad-helper", policy=policy)
            original_path.write_bytes(original_bytes)
            runtime_source = root / "coverage/runtime-source.tar.gz"
            saved = runtime_source.read_bytes()
            runtime_source.write_bytes(b"changed runtime sources")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                runtime.complete_companion(supplement, root / "coverage", package, root / "bad-sources", policy=policy)
            runtime_source.write_bytes(saved)
            package_bytes = package.read_bytes()
            package.write_bytes(b"changed final package")
            with self.assertRaisesRegex(ValueError, "identity mismatch"):
                runtime.complete_companion(supplement, root / "coverage", package, root / "bad-package", policy=policy)
            package.write_bytes(package_bytes)
            # A changed precursor cannot be combined with unchanged closure hashes.
            (supplement / "companion-source.tar.gz").write_bytes(b"changed helper source")
            with self.assertRaisesRegex(ValueError, "incomplete or changed"):
                runtime.complete_companion(supplement, root / "coverage", package, root / "bad", policy=policy)

    def test_sdk_alias_must_have_same_actual_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = self.fixture(root)
            sdk = fixture[2]
            alias = root / "alias"
            alias.mkdir()
            (alias / "libgcc.a").write_bytes(b"different runtime")
            rows = runtime.trace_inputs(str(alias / "libgcc.a"), sdk, (alias,))
            self.assertNotEqual(rows["libgcc.a"]["sha256"], runtime.sha(sdk / "lib/gcc/libgcc.a"))


    def test_wrong_compiler_and_unapproved_sdk_archive_fail_before_coverage_publication(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = self.fixture(root)
            package, evidence, sdk, sdk_archive, sources, policy = fixture
            with self.assertRaisesRegex(ValueError, "approved pinned"):
                runtime.collect(package, evidence, sdk, sdk_archive, sources, root / "bad-sdk",
                                policy=policy, allowed_sdk_hashes={"0" * 64})
            record_path = evidence / "plxnative.link.json"
            record = json.loads(record_path.read_text())
            record["compiler_sha256"] = "0" * 64
            record_path.write_text(json.dumps(record))
            with self.assertRaisesRegex(ValueError, "compiler differs"):
                self.collect(root, fixture)
