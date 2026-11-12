"""Safety boundaries for the deliberately isolated compositor experiment."""
import json
from html.parser import HTMLParser
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parent / "source"

class ProbeValidation(unittest.TestCase):
    def test_package_cannot_replace_production_or_request_privileges(self):
        manifest = json.loads((ROOT / "appinfo.json").read_text())
        production = json.loads((ROOT.parents[2] / "pkg/appinfo.json").read_text())
        self.assertNotEqual(manifest["id"], production["id"])
        self.assertTrue(manifest["id"].endswith(".overlayprobe"))
        self.assertEqual(manifest["type"], "web")
        self.assertEqual(manifest.get("requiredACG"), [])
        self.assertNotIn("requiredPermissions", manifest)
        self.assertTrue((ROOT / manifest["main"]).is_file())
        self.assertTrue((ROOT / manifest["icon"]).is_file())

    def test_no_remote_resources_or_media_decoder(self):
        class Resources(HTMLParser):
            def handle_starttag(self, tag, attrs):
                if tag in ("video", "audio", "iframe", "object", "embed"):
                    raise AssertionError("probe must not own media")
                for key, value in attrs:
                    if key in ("src", "href") and value:
                        raise AssertionError("probe must be self contained")
        html = (ROOT / "index.html").read_text()
        Resources().feed(html)
        for network_api in ("fetch(", "XMLHttpRequest", "WebSocket", "WebOSServiceBridge", "luna://", "ssap://"):
            self.assertNotIn(network_api, html)

if __name__ == "__main__":
    unittest.main()
