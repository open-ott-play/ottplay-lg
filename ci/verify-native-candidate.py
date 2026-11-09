#!/usr/bin/env python3
"""Verify downloaded candidate bytes against their immutable provenance receipt."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify(artifact_dir: Path, *, source_sha: str, run_id: str, run_attempt: str,
           version: str) -> tuple[Path, dict]:
    if not re.fullmatch(r"[0-9a-f]{40}", source_sha):
        raise ValueError("source SHA must be a full lowercase 40-character Git SHA")
    if not re.fullmatch(r"[0-9]+", run_id) or not re.fullmatch(r"[1-9][0-9]*", run_attempt):
        raise ValueError("candidate run id/attempt must be positive integers")
    if not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", version):
        raise ValueError("release version must be a numeric X.Y.Z")

    artifact_dir = artifact_dir.resolve(strict=True)
    receipt_path = artifact_dir / "candidate-provenance.json"
    receipt_sidecar = artifact_dir / "candidate-provenance.json.sha256"
    receipt = json.loads(receipt_path.read_text())
    expected = f"{sha256(receipt_path)}  {receipt_path.name}"
    if receipt_sidecar.read_text().strip() != expected:
        raise ValueError("provenance receipt checksum does not match")
    if receipt.get("schema") != 1 or receipt.get("artifact_kind") != "native-webos-stable-candidate":
        raise ValueError("unsupported or unexpected candidate receipt")
    if receipt.get("source", {}).get("commit") != source_sha:
        raise ValueError("candidate receipt source SHA does not match the requested SHA")
    if receipt.get("candidate") != {"run_id": run_id, "run_attempt": run_attempt}:
        raise ValueError("candidate receipt run identity does not match the requested run")

    package = receipt.get("package", {})
    filename = f"com.openottplay.player.native_{version}_arm.ipk"
    if (package.get("filename") != filename or package.get("version") != version
            or package.get("app_id") != "com.openottplay.player.native"):
        raise ValueError("candidate receipt package identity/version is inconsistent")
    ipk = artifact_dir / filename
    if not ipk.is_file() or ipk.stat().st_size != package.get("size_bytes"):
        raise ValueError("candidate IPK is missing or its size disagrees with the receipt")
    digest = sha256(ipk)
    if digest != package.get("sha256"):
        raise ValueError("candidate IPK SHA-256 disagrees with the receipt")
    package_sidecar = artifact_dir / (filename + ".sha256")
    if package_sidecar.read_text().strip() != f"{digest}  {filename}":
        raise ValueError("candidate package checksum sidecar disagrees")
    release_sidecar = artifact_dir / f"ottplay-lg-{version}.sha256"
    if release_sidecar.read_text().strip() != f"{digest}  {filename}":
        raise ValueError("release checksum asset disagrees with candidate bytes")

    evidence = artifact_dir / "link-evidence"
    if not evidence.is_dir() or not any(evidence.glob("*.link.json")):
        raise ValueError("candidate link evidence is missing")
    receipt_evidence = receipt.get("link_evidence_sha256", {})
    for relative, expected_digest in receipt_evidence.items():
        evidence_file = evidence / relative
        if not evidence_file.is_file() or sha256(evidence_file) != expected_digest:
            raise ValueError(f"link evidence checksum mismatch: {relative}")

    return ipk, receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-dir", required=True, type=Path)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--run-attempt", required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    try:
        ipk, receipt = verify(args.artifact_dir, source_sha=args.source_sha, run_id=args.run_id,
                              run_attempt=args.run_attempt, version=args.version)
    except (OSError, ValueError, json.JSONDecodeError, KeyError) as error:
        parser.exit(1, f"candidate verification failed: {error}\n")
    print(f"verified {ipk.name} sha256={receipt['package']['sha256']}")


if __name__ == "__main__":
    main()
