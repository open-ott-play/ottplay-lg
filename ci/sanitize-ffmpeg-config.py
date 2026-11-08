#!/usr/bin/env python3
"""Normalize public FFmpeg configuration metadata without changing compiler flags."""

import json
import sys
from pathlib import Path


def sanitize(path, sdk, source):
    text = path.read_text()
    lines = text.splitlines(keepends=True)
    matched = 0
    for index, line in enumerate(lines):
        prefix = '#define FFMPEG_CONFIGURATION '
        if not line.startswith(prefix):
            continue
        matched += 1
        value = json.loads(line[len(prefix):].strip())
        for old, new in ((sdk, '/build/webos-sdk'), (source, '/build/ottplay-native')):
            for spelling in {str(old), str(old.resolve())}:
                value = value.replace(spelling, new)
        lines[index] = prefix + json.dumps(value) + '\n'
    if matched != 1:
        raise ValueError('Expected exactly one FFMPEG_CONFIGURATION definition')
    updated = ''.join(lines)
    if updated != text:
        path.write_text(updated)


if __name__ == '__main__':
    if len(sys.argv) != 4:
        raise SystemExit('usage: sanitize-ffmpeg-config.py config.h sdk source')
    sanitize(*(Path(arg) for arg in sys.argv[1:]))
