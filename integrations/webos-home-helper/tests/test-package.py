"""Pure owned archive fixtures. No package tools, network or TV."""
import importlib.util, io, pathlib, tarfile, tempfile, unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('patcher', ROOT / 'fix-package.py')
patcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(patcher)

def package(entries):
    tar = io.BytesIO()
    with tarfile.open(fileobj=tar, mode='w:gz') as f:
        for name, link in entries:
            e = tarfile.TarInfo(name); e.mode = 0o755; e.uid = 7; e.gid = 8
            if link:
                e.type = tarfile.SYMTYPE; e.linkname = '/tmp/elsewhere'
            elif name.endswith('/state'):
                e.type = tarfile.DIRTYPE
            else:
                e.size = 1
            f.addfile(e, io.BytesIO(b'x') if e.isfile() else None)
    body = tar.getvalue()
    head = b'data.tar.gz/'.ljust(16) + b'0'.ljust(12) + b'0'.ljust(6) + b'0'.ljust(6) + b'100644'.ljust(8) + str(len(body)).encode().ljust(10) + b'`\n'
    return b'!<arch>\n' + head + body + (b'\n' if len(body) & 1 else b'')

class PackageTest(unittest.TestCase):
    def test_only_own_state_changes(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = pathlib.Path(tmp) / 'fixture.ipk'
            own = 'usr/palm/applications/com.openottplay.homehelper/state'
            untouched = 'usr/palm/applications/com.openottplay.homehelper/controller.js'
            p.write_bytes(package([(own, False), (untouched, False)]))
            patcher.patch(p, dict(developerUid=1001, sharedGid=1002))
            b = p.read_bytes(); size = int(b[56:66])
            with tarfile.open(fileobj=io.BytesIO(b[68:68+size]), mode='r:gz') as f:
                a, c = list(f)
                self.assertEqual((a.uid, a.gid, a.mode), (1001, 1002, 0o770))
                self.assertEqual((c.uid, c.gid, c.mode), (7, 8, 0o755))

    def test_traversal_links_and_root_identity_reject_without_rewrite(self):
        for entries, opts in [([('../escape', False)], dict(developerUid=1001, sharedGid=1002)),
                              ([('link', True)], dict(developerUid=1001, sharedGid=1002)),
                              ([('anything', False)], dict(developerUid=0, sharedGid=1002))]:
            with tempfile.TemporaryDirectory() as tmp:
                p = pathlib.Path(tmp) / 'fixture.ipk'; original = package(entries); p.write_bytes(original)
                with self.assertRaises(ValueError): patcher.patch(p, opts)
                self.assertEqual(p.read_bytes(), original)

if __name__ == '__main__': unittest.main()
