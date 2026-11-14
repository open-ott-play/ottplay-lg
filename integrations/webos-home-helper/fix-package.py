"""Adapted from MBS helper/fix-package.py @204d4ba, GPL-3.0-only.
Only own state-directory ownership is rewritten. Never extracts or contacts a TV.
"""
import gzip, io, json, pathlib, sys, tarfile
LIMIT = 32 * 1024 * 1024
APP = 'com.openottplay.homehelper'

def patch(filename, options):
    for key in ('developerUid', 'sharedGid'):
        if type(options.get(key)) is not int or not 1 <= options[key] <= 65534:
            raise ValueError('Verified non-root identity required')
    p = pathlib.Path(filename)
    if p.is_symlink() or not p.is_file() or p.stat().st_size > LIMIT:
        raise ValueError('Expected bounded regular IPK')
    data = p.read_bytes()
    if not data.startswith(b'!<arch>\n'): raise ValueError('Expected ar IPK')
    offset, members, changed, payloads = 8, [], 0, 0
    while offset < len(data):
        h = data[offset:offset+60]
        if len(h) != 60 or h[58:] != b'`\n': raise ValueError('Malformed ar')
        size = int(h[48:58]); body = data[offset+60:offset+60+size]
        if len(body) != size: raise ValueError('Truncated ar')
        if h[:16].decode().strip().rstrip('/') == 'data.tar.gz':
            payloads += 1
            raw = gzip.GzipFile(fileobj=io.BytesIO(body)).read(LIMIT + 1)
            if len(raw) > LIMIT: raise ValueError('Tar limit')
            out = io.BytesIO()
            with tarfile.open(fileobj=io.BytesIO(raw), mode='r:') as src, tarfile.open(fileobj=out, mode='w:gz') as dst:
                for entry in src:
                    name = entry.name.removeprefix('./')
                    if name.startswith('/') or '..' in pathlib.PurePosixPath(name).parts or not (entry.isfile() or entry.isdir()):
                        raise ValueError('Unsafe tar member')
                    state = any(name == prefix or name.startswith(prefix + '/') for prefix in (
                        'usr/palm/applications/' + APP + '/state',
                        'usr/palm/services/' + APP + '.control/state'))
                    if state:
                        entry.uid, entry.gid = options['developerUid'], options['sharedGid']
                        entry.uname = entry.gname = ''
                        entry.mode = 0o770 if entry.isdir() else 0o660
                        changed += 1
                    dst.addfile(entry, src.extractfile(entry) if entry.isfile() else None)
            body = out.getvalue()
        members.append((h, body)); offset += 60 + size + (size & 1)
    if offset != len(data) or payloads != 1 or changed == 0: raise ValueError('Missing unique state payload')
    result = bytearray(b'!<arch>\n')
    for h, body in members:
        result += h[:48] + str(len(body)).ljust(10).encode() + h[58:] + body + (b'\n' if len(body) & 1 else b'')
    if len(result) > LIMIT: raise ValueError('IPK limit')
    p.write_bytes(result)

if __name__ == '__main__':
    patch(sys.argv[1], json.loads(pathlib.Path(sys.argv[2]).read_text()))
