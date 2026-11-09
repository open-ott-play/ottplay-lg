"""Create Homebrew metadata for an already verified stable RC package."""
import argparse
import hashlib
import json
import re
from pathlib import Path

APP_ID = 'com.openottplay.player.native'


def manifest(package: Path, version: str, expected_sha256: str) -> dict:
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', version):
        raise ValueError('Expected numeric stable version')
    if package.name != f'{APP_ID}_{version}_arm.ipk':
        raise ValueError('Package filename does not match stable identity')
    digest = hashlib.sha256(package.read_bytes()).hexdigest()
    if digest != expected_sha256 or package.stat().st_size == 0:
        raise ValueError('Package does not match approved RC checksum')
    return {
        'id': APP_ID, 'version': version, 'type': 'native', 'title': 'OTTPlay',
        'iconUri': 'https://raw.githubusercontent.com/open-ott-play/ottplay-lg/main/pkg/icon160.png',
        'ipkUrl': f'https://github.com/open-ott-play/ottplay-lg/releases/download/v{version}/{package.name}',
        'ipkHash': {'sha256': digest}, 'ipkSize': package.stat().st_size,
        'sourceUrl': 'https://github.com/open-ott-play/ottplay-lg',
        'rootRequired': False,
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', type=Path, required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--sha256', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.write_text(json.dumps(manifest(args.package, args.version, args.sha256), indent=2) + '\n')
