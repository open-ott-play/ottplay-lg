import hashlib
import tempfile
import unittest
from pathlib import Path
from ci.native_hbc_manifest import manifest


class HomebrewManifestTests(unittest.TestCase):
    def test_manifest_binds_exact_approved_bytes_and_version(self):
        with tempfile.TemporaryDirectory() as directory:
            package = Path(directory) / 'com.openottplay.player.native_1.2.3_arm.ipk'
            package.write_bytes(b'approved-package')
            digest = hashlib.sha256(package.read_bytes()).hexdigest()
            result = manifest(package, '1.2.3', digest)
            self.assertEqual(result['ipkHash']['sha256'], digest)
            self.assertIn('/v1.2.3/', result['ipkUrl'])
            package.write_bytes(b'changed-package')
            with self.assertRaises(ValueError):
                manifest(package, '1.2.3', digest)

    def test_channel_suffix_and_other_package_are_rejected(self):
        for version in ('1.2.3-beta.1', '../1.2.3', '01.2.3'):
            with self.assertRaises(ValueError):
                manifest(Path('different.ipk'), version, '0' * 64)
        with self.assertRaises(ValueError):
            manifest(Path('different.ipk'), '1.2.3', '0' * 64)
