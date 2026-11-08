#!/usr/bin/env python3
"""Read-only HostFixture verifier derived from qualified native-local-m3u.py.
Never writes fixture files. Never emits the plaintext value except to its caller.
"""
import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import stat
import struct

DOMAIN = b"ott-host-fixture-v1\0NONCONFIDENTIAL"


def private_read(path: Path, limit: int) -> bytes:
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0))
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise ValueError("fixture input must be our private regular file")
        if info.st_size > limit:
            raise ValueError("fixture input exceeds its size limit")
        data = os.read(fd, limit + 1)
        if len(data) > limit:
            raise ValueError("fixture input grew past its size limit")
        return data
    finally:
        os.close(fd)


def private_directory(path: Path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise ValueError("fixture directory must be our private 0700 directory")


def frame(parts):
    return b"".join(struct.pack(">Q", len(part)) + part for part in parts)


def json_value(data):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate fixture JSON field")
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=pairs, parse_constant=lambda _: (_ for _ in ()).throw(ValueError("non-finite fixture JSON")))


def exact(obj, keys):
    if not isinstance(obj, dict) or set(obj) != set(keys):
        raise ValueError("unexpected fixture record shape")


def canonical_base64(value, limit):
    if not isinstance(value, str) or len(value) > 4 * ((limit + 2) // 3):
        raise ValueError("invalid fixture base64 size")
    result = base64.b64decode(value, validate=True)
    if len(result) > limit or base64.b64encode(result).decode() != value:
        raise ValueError("noncanonical fixture base64")
    return result


def inspect_record(root: Path, namespace="ott-control"):
    if namespace not in {"ott-control", "ott-state"}:
        raise ValueError("only explicit OTT fixture namespaces are accepted")
    """Read ONLY explicit NONCONFIDENTIAL HostFixture records, authenticating before JSON parse."""
    if not root.is_absolute() or root.resolve(strict=True) != root:
        raise ValueError("fixture path is not canonical absolute")
    private_directory(root)
    private_directory(root / "records")
    key = private_read(root / "key", 32)
    marker = private_read(root / "marker", 8192)
    if len(key) != 32 or len(marker) < 32:
        raise ValueError("invalid synthetic fixture authority")
    body, tag = marker[:-32], marker[-32:]
    if not hmac.compare_digest(hmac.digest(key, body, "sha256"), tag):
        raise ValueError("synthetic fixture marker authentication failed")
    parts, offset = [], 0
    while offset < len(body):
        if offset + 8 > len(body):
            raise ValueError("invalid fixture marker frame")
        length = struct.unpack_from(">Q", body, offset)[0]
        offset += 8
        if length > len(body) - offset:
            raise ValueError("invalid fixture marker length")
        parts.append(body[offset:offset+length]); offset += length
    if len(parts) != 4 or parts[:3] != [DOMAIN, b"marker", str(root).encode()]:
        raise ValueError("not this explicit NONCONFIDENTIAL fixture root")
    install = parts[3].decode("utf-8", "strict")
    if not install or len(install) > 256 or any(ord(c) < 32 for c in install):
        raise ValueError("invalid fixture install identity")
    raw = private_read(root / "records" / (namespace + ".json"), 2 * 1024 * 1024)
    record = json_value(raw)
    exact(record, ["format", "version", "domain", "key", "revision", "state"])
    if (record["format"] != "plxnative-record" or type(record["version"]) is not int or record["version"] != 1
            or record["domain"] != namespace or record["key"] != namespace
            or type(record["revision"]) is not int or record["revision"] < 1):
        raise ValueError("unexpected OTT fixture record identity")
    exact(record["state"], ["Data"])
    exact(record["state"]["Data"], ["payload"])
    payload = json_value(record["state"]["Data"]["payload"])
    exact(payload, ["version", "sealed"])
    sealed = payload["sealed"]
    exact(sealed, ["backend", "key", "iv", "data"])
    namespace_key = install + ".ott." + namespace[4:] + ".v1"
    if (type(payload["version"]) is not int or payload["version"] != 1
            or sealed["backend"] != "host_fixture" or sealed["key"] != namespace_key):
        raise ValueError("refusing non-HostFixture or wrong-namespace record")
    plain = canonical_base64(sealed["data"], 1024 * 1024)
    tag = canonical_base64(sealed["iv"], 32)
    authenticated = frame([DOMAIN, b"envelope", b"1", b"host_fixture", str(root).encode(),
                           install.encode(), namespace_key.encode(), plain])
    if not hmac.compare_digest(hmac.digest(key, authenticated, "sha256"), tag):
        raise ValueError("synthetic fixture envelope authentication failed")
    inner = json_value(plain)
    exact(inner, ["version", "install", "namespace", "value"])
    if (type(inner["version"]) is not int or inner["version"] != 1
            or inner["install"] != install or inner["namespace"] != namespace):
        raise ValueError("wrong inner fixture identity")
    return {"value": inner["value"], "record_revision": record["revision"],
            "record_sha256": hashlib.sha256(raw).hexdigest()}
