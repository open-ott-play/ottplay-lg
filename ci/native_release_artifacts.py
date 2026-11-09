#!/usr/bin/env python3
"""Verify downloaded native webOS RC assets before stable qualification."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import shutil
import stat
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
VERIFY_SCRIPT = Path(__file__).with_name("verify-native-candidate.py")
PLAN_SCRIPT = Path(__file__).with_name("native_release.py")
VERSION_RE = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z", re.ASCII)
SHA_RE = re.compile(r"[0-9a-f]{40}\Z", re.ASCII)
SHA256_RE = re.compile(r"[0-9a-f]{64}\Z", re.ASCII)
POSITIVE_RE = re.compile(r"[1-9][0-9]*\Z", re.ASCII)
MAX_EVIDENCE_MEMBERS = 10_000
MAX_EVIDENCE_FILE_BYTES = 64 * 1024 * 1024
MAX_EVIDENCE_TOTAL_BYTES = 512 * 1024 * 1024

RECEIPT_FIELDS = {
    "schema", "artifact_kind", "candidate", "source", "package", "toolchain",
    "link_evidence_sha256", "checks",
}
RECEIPT_CANDIDATE_FIELDS = {"run_id", "run_attempt"}
RECEIPT_SOURCE_FIELDS = {"repository", "commit", "tree"}
RECEIPT_PACKAGE_FIELDS = {
    "filename", "app_id", "version", "sha256", "size_bytes", "build_config", "build_config_sha256",
}
RECEIPT_TOOLCHAIN_FIELDS = {
    "runner", "webos_ndk_version", "webos_ndk_asset", "webos_ndk_archive_sha256",
    "webos_gcc_sha256", "webos_gcc_backend_sha256", "rust_toolchain_pin", "rustc_version",
    "cargo_version", "cargo_lock_sha256", "dependency_manifest_sha256", "makefile_sha256",
}


class ArtifactError(ValueError):
    """Downloaded release assets are malformed or do not match each other."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ArtifactError(message)


def _strict_object(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    _require(type(value) is dict, f"{label} must be a JSON object")
    missing = expected - set(value)
    extra = set(value) - expected
    _require(not missing, f"{label} missing fields: {', '.join(sorted(missing))}")
    _require(not extra, f"{label} has unknown fields: {', '.join(sorted(extra))}")
    return value


def _json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ArtifactError(f"duplicate JSON field: {key}")
        value[key] = item
    return value


def _read_json(path: Path) -> Any:
    try:
        with path.open("r", encoding="utf-8") as stream:
            return json.load(stream, object_pairs_hook=_json_object,
                             parse_constant=lambda constant: (_ for _ in ()).throw(
                                 ArtifactError(f"invalid JSON constant: {constant}")))
    except UnicodeDecodeError as exc:
        raise ArtifactError(f"{path.name} must be UTF-8 JSON") from exc
    except json.JSONDecodeError as exc:
        raise ArtifactError(f"{path.name} contains invalid JSON: {exc.msg}") from exc


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _regular_file(path: Path, label: str) -> None:
    try:
        mode = path.lstat().st_mode
    except OSError as exc:
        raise ArtifactError(f"{label} is missing") from exc
    _require(stat.S_ISREG(mode), f"{label} must be a regular non-symlink file")


COMPANION_SOURCE_ASSETS = {"companion-source.tar.gz", "companion-source-closure.json", "companion-source-manifest.json"}
MAX_COMPANION_SOURCE_BYTES = 1024 * 1024 * 1024


def companion_inventory(root: Path) -> set[str]:
    receipt = _read_json(root / "candidate-provenance.json")
    _require(type(receipt) is dict, "candidate receipt must be an object")
    return COMPANION_SOURCE_ASSETS if "companion" in receipt else set()


def expected_inventory(version: str) -> set[str]:
    _require(type(version) is str and VERSION_RE.fullmatch(version) is not None,
             "version must be numeric X.Y.Z without leading zeroes")
    package = f"com.openottplay.player.native_{version}_arm.ipk"
    return {
        package,
        package + ".sha256",
        f"ottplay-lg-{version}.sha256",
        "candidate-provenance.json",
        "candidate-provenance.json.sha256",
        "native-release-plan.json",
        "link-evidence.tar.gz",
    }


def _validate_receipt(receipt: Any, version: str) -> dict[str, Any]:
    extra = {"companion"} if type(receipt) is dict and "companion" in receipt else set()
    receipt = _strict_object(receipt, RECEIPT_FIELDS | extra, "candidate receipt")
    if extra:
        companion = _strict_object(receipt["companion"], {"metadata", "source_assets", "source_revision"}, "companion")
        _require(type(companion["source_revision"]) is str and SHA_RE.fullmatch(companion["source_revision"]) is not None,
                 "companion source revision must be a full SHA")
        _strict_object(companion["source_assets"], COMPANION_SOURCE_ASSETS, "companion source assets")
        for digest in companion["source_assets"].values():
            _require(type(digest) is str and SHA256_RE.fullmatch(digest) is not None, "invalid companion source asset digest")
        _require(type(companion["metadata"]) is dict, "companion metadata must be an object")
    _require(type(receipt["schema"]) is int and receipt["schema"] == 1,
             "candidate receipt schema must be integer 1")
    _require(receipt["artifact_kind"] == "native-webos-stable-candidate",
             "candidate receipt artifact kind is unsupported")

    candidate = _strict_object(receipt["candidate"], RECEIPT_CANDIDATE_FIELDS, "candidate receipt candidate")
    _require(type(candidate["run_id"]) is str and POSITIVE_RE.fullmatch(candidate["run_id"]) is not None,
             "candidate receipt run_id must be a canonical positive integer string")
    _require(type(candidate["run_attempt"]) is str and POSITIVE_RE.fullmatch(candidate["run_attempt"]) is not None,
             "candidate receipt run_attempt must be a canonical positive integer string")

    source = _strict_object(receipt["source"], RECEIPT_SOURCE_FIELDS, "candidate receipt source")
    _require(type(source["repository"]) is str and source["repository"].strip() == source["repository"]
             and bool(source["repository"]), "candidate receipt source repository is invalid")
    _require(type(source["commit"]) is str and SHA_RE.fullmatch(source["commit"]) is not None,
             "candidate receipt source commit is invalid")
    _require(type(source["tree"]) is str and SHA_RE.fullmatch(source["tree"]) is not None,
             "candidate receipt source tree is invalid")

    package = _strict_object(receipt["package"], RECEIPT_PACKAGE_FIELDS, "candidate receipt package")
    filename = f"com.openottplay.player.native_{version}_arm.ipk"
    _require(package["filename"] == filename and package["version"] == version
             and package["app_id"] == "com.openottplay.player.native",
             "candidate receipt package identity/version is inconsistent")
    _require(type(package["sha256"]) is str and SHA256_RE.fullmatch(package["sha256"]) is not None,
             "candidate receipt package SHA-256 is invalid")
    _require(type(package["size_bytes"]) is int and package["size_bytes"] > 0,
             "candidate receipt package size_bytes must be a positive integer")
    _require(type(package["build_config"]) is str and bool(package["build_config"]),
             "candidate receipt build_config is missing")
    _require(type(package["build_config_sha256"]) is str
             and SHA256_RE.fullmatch(package["build_config_sha256"]) is not None,
             "candidate receipt build_config SHA-256 is invalid")

    toolchain = _strict_object(receipt["toolchain"], RECEIPT_TOOLCHAIN_FIELDS, "candidate receipt toolchain")
    for key, value in toolchain.items():
        _require(type(value) is str and bool(value.strip()), f"candidate receipt toolchain {key} is missing")
    _require(toolchain["runner"] == "ubuntu-24.04-arm", "candidate receipt runner is unsupported")
    for key in ("webos_ndk_archive_sha256", "webos_gcc_sha256", "webos_gcc_backend_sha256",
                "cargo_lock_sha256", "dependency_manifest_sha256", "makefile_sha256"):
        _require(SHA256_RE.fullmatch(toolchain[key]) is not None,
                 f"candidate receipt toolchain {key} is invalid")

    evidence = receipt["link_evidence_sha256"]
    _require(type(evidence) is dict and bool(evidence), "candidate receipt link evidence inventory is empty")
    for relative, digest in evidence.items():
        _validate_evidence_path(relative)
        _require(type(digest) is str and SHA256_RE.fullmatch(digest) is not None,
                 f"candidate receipt link evidence digest is invalid: {relative}")
    checks = receipt["checks"]
    _require(type(checks) is list and bool(checks) and all(type(item) is str and bool(item.strip()) for item in checks),
             "candidate receipt checks must be a nonempty list of strings")
    _require(len(set(checks)) == len(checks), "candidate receipt checks must not contain duplicates")
    return receipt


def _validate_evidence_path(relative: Any) -> tuple[str, ...]:
    _require(type(relative) is str and bool(relative) and "\\" not in relative and "\x00" not in relative,
             "link evidence path is malformed")
    _require(not relative.startswith("/"), "link evidence path must be relative")
    parts = relative.split("/")
    _require(all(part not in {"", ".", ".."} for part in parts),
             "link evidence path has an empty, dot, or traversal component")
    path = PurePosixPath(relative)
    _require(not path.is_absolute() and path.parts == tuple(parts), "link evidence path is not canonical")
    return tuple(parts)


def _inventory_root(artifact_dir: Path, version: str) -> dict[str, Path]:
    root = artifact_dir.resolve(strict=True)
    _require(root.is_dir(), "artifact-dir must be a directory")
    entries = list(root.iterdir())
    names = {entry.name for entry in entries}
    expected = expected_inventory(version) | companion_inventory(root)
    missing = expected - names
    extra = names - expected
    _require(not missing, f"artifact inventory missing assets: {', '.join(sorted(missing))}")
    _require(not extra, f"artifact inventory has unexpected assets: {', '.join(sorted(extra))}")
    result = {}
    for name in expected:
        path = root / name
        _regular_file(path, f"artifact {name}")
        result[name] = path
    return result


def _candidate_root(artifact_dir: Path, version: str) -> tuple[Path, dict[str, Path]]:
    root = artifact_dir.resolve(strict=True)
    _require(root.is_dir(), "candidate-artifact-dir must be a directory")
    inventory = (expected_inventory(version) | companion_inventory(root)) - {"link-evidence.tar.gz"}
    expected = inventory | {"link-evidence"}
    entries = list(root.iterdir())
    names = {entry.name for entry in entries}
    missing = expected - names
    extra = names - expected
    _require(not missing, f"candidate artifact missing assets: {', '.join(sorted(missing))}")
    _require(not extra, f"candidate artifact has unexpected assets: {', '.join(sorted(extra))}")
    evidence = root / "link-evidence"
    try:
        evidence_mode = evidence.lstat().st_mode
    except OSError as exc:
        raise ArtifactError("candidate link-evidence directory is missing") from exc
    _require(stat.S_ISDIR(evidence_mode), "candidate link-evidence must be a regular directory, not a link")
    files = {}
    for name in inventory:
        path = root / name
        _regular_file(path, f"candidate artifact {name}")
        files[name] = path
    return root, files


def _verify_candidate_evidence(evidence_dir: Path, expected_hashes: dict[str, str]) -> None:
    expected_files = set(expected_hashes)
    found_files: set[str] = set()
    found_directories = {""}
    stack = [(evidence_dir, "")]
    while stack:
        directory, relative_parent = stack.pop()
        try:
            entries = list(directory.iterdir())
        except OSError as exc:
            raise ArtifactError(f"cannot inspect candidate link evidence: {exc}") from exc
        for path in entries:
            relative = f"{relative_parent}/{path.name}" if relative_parent else path.name
            try:
                mode = path.lstat().st_mode
            except OSError as exc:
                raise ArtifactError(f"cannot inspect candidate evidence entry {relative}: {exc}") from exc
            if stat.S_ISDIR(mode):
                found_directories.add(relative)
                stack.append((path, relative))
            elif stat.S_ISREG(mode):
                found_files.add(relative)
            else:
                raise ArtifactError(f"candidate evidence entry is not a regular file or directory: {relative}")

    missing = expected_files - found_files
    extra = found_files - expected_files
    _require(not missing and not extra,
             "candidate evidence inventory does not match receipt"
             + (f" (missing: {', '.join(sorted(missing))})" if missing else "")
             + (f" (extra: {', '.join(sorted(extra))})" if extra else ""))
    expected_directories = {""}
    for relative in expected_files:
        parts = relative.split("/")
        expected_directories.update("/".join(parts[:index]) for index in range(1, len(parts)))
    _require(found_directories == expected_directories,
             "candidate evidence directory inventory does not match receipt")
    for relative, expected_digest in expected_hashes.items():
        path = evidence_dir / Path(*_validate_evidence_path(relative))
        _require(sha256(path) == expected_digest, f"candidate evidence checksum mismatch: {relative}")


def _same_bytes(left: Path, right: Path, label: str) -> None:
    _regular_file(left, f"candidate {label}")
    _regular_file(right, f"release {label}")
    _require(left.stat().st_size == right.stat().st_size,
             f"candidate {label} differs from downloaded release asset")
    with left.open("rb") as a, right.open("rb") as b:
        while True:
            left_block = a.read(1024 * 1024)
            right_block = b.read(1024 * 1024)
            _require(left_block == right_block, f"candidate {label} differs from downloaded release asset")
            if not left_block:
                break


def _verify_receipt_sidecar(receipt_path: Path, sidecar_path: Path) -> None:
    expected = f"{sha256(receipt_path)}  {receipt_path.name}\n"
    _require(sidecar_path.read_text(encoding="ascii") == expected,
             "provenance receipt checksum does not match")


def _extract_evidence(archive_path: Path, destination: Path, expected_files: set[str]) -> None:
    destination.mkdir(mode=0o700)
    seen: set[str] = set()
    files: set[str] = set()
    directories: set[str] = {"link-evidence"}
    total_size = 0
    try:
        with tarfile.open(archive_path, mode="r:gz") as archive:
            members = archive.getmembers()
            _require(0 < len(members) <= MAX_EVIDENCE_MEMBERS,
                     "link evidence archive has an invalid member count")
            for member in members:
                raw = member.name
                _require(type(raw) is str and "\\" not in raw and "\x00" not in raw,
                         "link evidence archive contains a malformed member name")
                directory_name = member.isdir()
                canonical = raw[:-1] if directory_name and raw.endswith("/") else raw
                parts = _validate_evidence_path(canonical)
                _require(parts[0] == "link-evidence", "link evidence archive has an unexpected root")
                _require(len(parts) >= 1, "link evidence archive contains an empty path")
                _require(canonical not in seen, f"link evidence archive repeats member: {canonical}")
                seen.add(canonical)
                if directory_name:
                    _require(canonical == "link-evidence" or len(parts) > 1,
                             "link evidence archive has an invalid directory")
                    directories.add(canonical)
                else:
                    _require(member.isreg(), f"link evidence member is not a regular file: {canonical}")
                    _require(len(parts) > 1, "link evidence root cannot be a file")
                    _require(member.size > 0 and member.size <= MAX_EVIDENCE_FILE_BYTES,
                             f"link evidence member has invalid size: {canonical}")
                    total_size += member.size
                    _require(total_size <= MAX_EVIDENCE_TOTAL_BYTES,
                             "link evidence archive exceeds the extraction size limit")
                    files.add("/".join(parts[1:]))

            _require(files == expected_files,
                     "link evidence archive inventory does not match receipt digests"
                     + (f" (missing: {', '.join(sorted(expected_files - files))})" if expected_files - files else "")
                     + (f" (extra: {', '.join(sorted(files - expected_files))})" if files - expected_files else ""))

            expected_directories = {"link-evidence"}
            for file_name in expected_files:
                parts = ("link-evidence", *file_name.split("/"))
                expected_directories.update("/".join(parts[:index]) for index in range(1, len(parts)))
            extra_directories = directories - expected_directories
            _require(not extra_directories,
                     "link evidence archive has unexpected directories: " + ", ".join(sorted(extra_directories)))

            # Reject file/directory conflicts before writing any archive data.
            for file_name in files:
                file_parts = ("link-evidence", *file_name.split("/"))
                for index in range(1, len(file_parts)):
                    parent = "/".join(file_parts[:index])
                    _require(parent not in files, f"link evidence file is also a directory: {parent}")
            for directory in directories:
                for file_name in files:
                    _require(directory != "link-evidence/" + file_name,
                             f"link evidence path is both file and directory: {directory}")

            for directory in directories:
                (destination / Path(*directory.split("/"))).mkdir(parents=True, exist_ok=True, mode=0o700)
            for member in members:
                if member.isdir():
                    continue
                canonical = member.name
                target = destination / Path(*canonical.split("/"))
                target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                _require(not target.exists(), f"link evidence target already exists: {canonical}")
                source = archive.extractfile(member)
                _require(source is not None, f"cannot read link evidence member: {canonical}")
                written = 0
                with source, target.open("xb") as output:
                    while True:
                        block = source.read(1024 * 1024)
                        if not block:
                            break
                        written += len(block)
                        _require(written <= member.size and written <= MAX_EVIDENCE_FILE_BYTES,
                                 f"link evidence member exceeded its declared size: {canonical}")
                        output.write(block)
                _require(written == member.size, f"link evidence member was truncated: {canonical}")
    except (tarfile.TarError, OSError) as exc:
        if isinstance(exc, ArtifactError):
            raise
        raise ArtifactError(f"cannot safely read link evidence archive: {exc}") from exc


def _load_candidate_verifier():
    spec = importlib.util.spec_from_file_location("native_candidate_verifier", VERIFY_SCRIPT)
    _require(spec is not None and spec.loader is not None, "native candidate verifier is unavailable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _load_plan_helper():
    spec = importlib.util.spec_from_file_location("native_release_plan", PLAN_SCRIPT)
    _require(spec is not None and spec.loader is not None, "native release plan helper is unavailable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def verify_rc(
    artifact_dir: Path, *, version: str, tag: str, repository: str,
    candidate_artifact_dir: Path | None = None,
) -> dict[str, Any]:
    """Validate a downloaded RC asset set and return its exact identity."""
    _require(type(version) is str and VERSION_RE.fullmatch(version) is not None,
             "version must be numeric X.Y.Z without leading zeroes")
    assets = _inventory_root(artifact_dir, version)
    receipt_path = assets["candidate-provenance.json"]
    _verify_receipt_sidecar(receipt_path, assets["candidate-provenance.json.sha256"])
    receipt = _validate_receipt(_read_json(receipt_path), version)
    if "companion" in receipt:
        for name, digest in receipt["companion"]["source_assets"].items():
            _require(0 < assets[name].stat().st_size <= MAX_COMPANION_SOURCE_BYTES,
                     "companion source asset exceeds its size bound")
            _require(sha256(assets[name]) == digest, "companion source asset digest mismatch")
    plan = _load_plan_helper().validate_plan(_read_json(assets["native-release-plan.json"]))
    expected_tag = f"v{version}-rc.{plan['sequence']}"
    _require(plan["channel"] == "rc", "RC artifact release plan channel must be rc")
    _require(plan["repository"] == repository, "release plan repository does not match expected repository")
    _require(plan["version"] == version, "release plan version does not match requested version")
    _require(plan["tag"] == tag == expected_tag, "release plan tag does not match requested RC tag/version/sequence")
    _require(plan["source_sha"] == receipt["source"]["commit"],
             "release plan source SHA does not match candidate receipt")
    _require(plan["run_id"] == int(receipt["candidate"]["run_id"]),
             "release plan run id does not match candidate receipt")
    _require(plan["run_attempt"] == int(receipt["candidate"]["run_attempt"]),
             "release plan run attempt does not match candidate receipt")

    temp_root = Path(tempfile.mkdtemp(prefix="native-rc-evidence-"))
    try:
        for name in (set(assets) - {"link-evidence.tar.gz", "native-release-plan.json"}):
            shutil.copyfile(assets[name], temp_root / name)
        extracted = temp_root / "extracted"
        _extract_evidence(assets["link-evidence.tar.gz"], extracted,
                          set(receipt["link_evidence_sha256"]))
        shutil.move(str(extracted / "link-evidence"), str(temp_root / "link-evidence"))
        # The existing verifier owns checksum, receipt-to-package, and evidence digest validation.
        verifier = _load_candidate_verifier()
        package, verified_receipt = verifier.verify(
            temp_root, source_sha=receipt["source"]["commit"],
            run_id=receipt["candidate"]["run_id"], run_attempt=receipt["candidate"]["run_attempt"],
            version=version,
        )
        _require(verified_receipt == receipt, "candidate verifier parsed a different provenance receipt")

        if candidate_artifact_dir is not None:
            candidate_root, candidate_assets = _candidate_root(candidate_artifact_dir, version)
            identity_files = set(assets) - {"link-evidence.tar.gz"}
            for name in identity_files:
                _same_bytes(candidate_assets[name], assets[name], name)
            _verify_candidate_evidence(candidate_root / "link-evidence", receipt["link_evidence_sha256"])
            # Independently verify the original immutable artifact, rather than trusting
            # release assets that may have been replaced together with their checksums.
            candidate_package, candidate_receipt = verifier.verify(
                candidate_root, source_sha=receipt["source"]["commit"],
                run_id=receipt["candidate"]["run_id"], run_attempt=receipt["candidate"]["run_attempt"],
                version=version,
            )
            _require(candidate_receipt == receipt, "original candidate receipt differs from release identity")
            _require(candidate_package.name == package.name, "original candidate package identity changed")
        return {
            "source_sha": receipt["source"]["commit"],
            "run_id": int(receipt["candidate"]["run_id"]),
            "run_attempt": int(receipt["candidate"]["run_attempt"]),
            "version": version,
            "ipk_sha256": receipt["package"]["sha256"],
        }
    finally:
        shutil.rmtree(temp_root, ignore_errors=True)


def write_identity(identity: dict[str, Any], output: Path) -> None:
    expected = {"source_sha", "run_id", "run_attempt", "version", "ipk_sha256"}
    _strict_object(identity, expected, "verified identity")
    payload = json.dumps(identity, sort_keys=True, indent=2) + "\n"
    try:
        with output.open("x", encoding="utf-8", newline="\n") as stream:
            stream.write(payload)
    except FileExistsError as exc:
        raise ArtifactError(f"refusing to overwrite identity output: {output}") from exc


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)
    verify = commands.add_parser("verify-rc", help="verify downloaded RC package and provenance assets")
    verify.add_argument("--artifact-dir", required=True, type=Path)
    verify.add_argument("--version", required=True)
    verify.add_argument("--tag", required=True)
    verify.add_argument("--repository", required=True)
    verify.add_argument("--candidate-artifact-dir", type=Path,
                        help="original immutable artifact downloaded by run ID; pass this for release qualification")
    verify.add_argument("--output-json", required=True, type=Path)
    return root


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        identity = verify_rc(args.artifact_dir, version=args.version, tag=args.tag, repository=args.repository,
                             candidate_artifact_dir=args.candidate_artifact_dir)
        write_identity(identity, args.output_json)
    except (OSError, ArtifactError, ValueError, KeyError, tarfile.TarError) as exc:
        print(f"native_release_artifacts: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(identity, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
