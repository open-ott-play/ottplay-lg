"""Only exported configuration metadata may change during path normalization."""
import importlib.util
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('normalize', Path(__file__).with_name('sanitize-ffmpeg-config.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ConfigurationMetadataTests(unittest.TestCase):
    def test_paths_change_only_in_configuration_macro_and_are_idempotent(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.h'
            config.write_text('#define FFMPEG_CONFIGURATION "--sysroot=/Users/test/sdk/root --cc=/Users/test/source/cc"\n#define OTHER "/Users/test/sdk"\n')
            module.sanitize(config, Path('/Users/test/sdk'), Path('/Users/test/source'))
            self.assertEqual(config.read_text(), '#define FFMPEG_CONFIGURATION "--sysroot=/build/webos-sdk/root --cc=/build/ottplay-native/cc"\n#define OTHER "/Users/test/sdk"\n')
            timestamp = config.stat().st_mtime_ns
            module.sanitize(config, Path('/Users/test/sdk'), Path('/Users/test/source'))
            self.assertEqual(config.stat().st_mtime_ns, timestamp)

    def test_missing_configuration_macro_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.h'
            config.write_text('#define OTHER 1\n')
            with self.assertRaisesRegex(ValueError, 'exactly one'):
                module.sanitize(config, Path('/sdk'), Path('/source'))


if __name__ == '__main__':
    unittest.main()
