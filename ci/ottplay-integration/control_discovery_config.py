"""Validate public catalog build inputs; never fetch metadata or invoke a shell."""

import hashlib
import ipaddress
import os
import re
import sys
from urllib.parse import urlsplit

URL_ENV = "PLX_CONTROL_DISCOVERY_URL"
SHA256_ENV = "PLX_CONTROL_DISCOVERY_SHA256"


def profile(url=None, digest=None):
    if (url is None) != (digest is None):
        raise ValueError("Control discovery requires both catalog URL and SHA-256")
    if url in (None, "") and digest in (None, ""):
        return {"configured": False, "fingerprint": "none"}
    if not isinstance(url, str) or not isinstance(digest, str):
        raise ValueError("Control discovery requires both catalog URL and SHA-256")
    if not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("Control discovery SHA-256 must be 64 lowercase hexadecimal characters")
    # Deliberately narrower than a general URL: no escaping, credentials, query,
    # fragment, shell/Make syntax, Unicode or path normalization is necessary.
    if not 1 <= len(url) <= 2048 or not re.fullmatch(r"https://[a-z0-9.:-]+/[A-Za-z0-9._~/-]+", url):
        raise ValueError("Control discovery requires a canonical HTTPS catalog URL")
    try:
        parsed = urlsplit(url)
        host = parsed.hostname or ""
        port = parsed.port
    except ValueError:
        raise ValueError("Control discovery catalog authority is invalid") from None
    label = r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?"
    if (len(host) > 253 or not re.fullmatch(label + r"(?:\." + label + r")+", host)
            or (port is not None and not 1 <= port <= 65535)
            or parsed.netloc != host + (":" + str(port) if port is not None else "")
            or any(part in ("", ".", "..") for part in parsed.path[1:].split("/"))):
        raise ValueError("Control discovery catalog authority or path is invalid")
    if re.fullmatch(r"[0-9.]+", host):
        try:
            if str(ipaddress.IPv4Address(host)) != host:
                raise ValueError()
        except ValueError:
            raise ValueError("Control discovery numeric host must be canonical IPv4") from None
    fingerprint = hashlib.sha256(b"v1\0" + url.encode("ascii") + b"\0" + digest.encode("ascii")).hexdigest()
    return {"configured": True, "url": url, "sha256": digest, "fingerprint": fingerprint}


def main():
    if sys.argv[1:] != ["stamp"]:
        raise ValueError("Expected the control discovery stamp operation")
    print(profile(os.environ.get(URL_ENV), os.environ.get(SHA256_ENV))["fingerprint"])


if __name__ == "__main__":
    try:
        main()
    except ValueError:
        # Do not echo attacker-controlled values into build logs or Make syntax.
        print("Invalid control discovery build inputs", file=sys.stderr)
        sys.exit(1)
