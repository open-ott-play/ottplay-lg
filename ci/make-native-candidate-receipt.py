#!/usr/bin/env python3
"""Package a verified stable IPK with exact source and toolchain provenance."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "ci"))
import deps  # noqa: E402
import flavor  # noqa: E402


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def valid_source_sha(value: str) -> bool:
    return re.fullmatch(r"[0-9a-f]{40}", value) is not None


def expected_ipk_name() -> str:
    app = flavor.appinfo_for("stable")
    return f"{app['id']}_{app['version']}_arm.ipk"


def validate_release_stamp(path: Path) -> str:
    stamp = path.read_text().strip()
    if not re.fullmatch(
        r"features:--no-default-features(?:\+tel:[0-9a-f]+)?(?:\+control:(?:none|[0-9a-f]{64}))?",
        stamp,
    ):
        raise ValueError("pkg/.build-config does not prove an ordinary RELEASE=1 stable build")
    return stamp


def run(argv: list[str], *, cwd: Path = ROOT) -> str:
    return subprocess.check_output(argv, cwd=cwd, text=True).strip()


def create_receipt(source_sha: str, run_id: str, run_attempt: str,
                   sdk: Path, ipk: Path, output_dir: Path, companion_sources: Path | None = None) -> dict:
    if not valid_source_sha(source_sha):
        raise ValueError("source SHA must be a full lowercase 40-character Git SHA")
    actual_sha = run(["git", "rev-parse", "HEAD"])
    if actual_sha != source_sha:
        raise ValueError(f"checked-out source is {actual_sha}, expected {source_sha}")
    if run(["git", "status", "--porcelain", "--untracked-files=no"]):
        raise ValueError("tracked source tree has modifications; refusing to attest it as a clean commit")

    ipk = ipk.resolve(strict=True)
    if ipk.name != expected_ipk_name():
        raise ValueError(f"unexpected stable IPK name: {ipk.name!r}; expected {expected_ipk_name()!r}")
    if not ipk.is_file() or ipk.stat().st_size == 0:
        raise ValueError("candidate IPK is missing or empty")

    sdk = sdk.resolve(strict=True)
    sdk_manifest = deps.entry("webos-ndk")
    sdk_asset = sdk_manifest["assets"].get("linux-aarch64")
    if not sdk_asset:
        raise ValueError("pinned webOS NDK has no linux-aarch64 asset")
    compiler = sdk / "bin/arm-webos-linux-gnueabi-gcc"
    backend = compiler.with_name(compiler.name + ".br_real")
    if not compiler.is_file() or not backend.is_file():
        raise ValueError("verified webOS SDK compiler/backend is missing")

    rust_pin = (ROOT / "rust-modules/rust-toolchain.toml").read_text()
    match = re.search(r'^channel\s*=\s*"([^\"]+)"\s*$', rust_pin, re.MULTILINE)
    if not match:
        raise ValueError("Rust toolchain pin is missing from rust-modules/rust-toolchain.toml")
    rust_channel = match.group(1)
    rustc = run(["rustup", "run", rust_channel, "rustc", "-Vv"])
    cargo = run(["rustup", "run", rust_channel, "cargo", "-V"])

    stamp_path = ROOT / "pkg/.build-config"
    stamp = validate_release_stamp(stamp_path)
    appinfo = flavor.appinfo_for("stable")
    source_tree = run(["git", "rev-parse", f"{source_sha}^{{tree}}"])

    output_dir.mkdir(parents=True, exist_ok=True)
    copied_ipk = output_dir / ipk.name
    shutil.copyfile(ipk, copied_ipk)
    package_digest = sha256(copied_ipk)
    (output_dir / (ipk.name + ".sha256")).write_text(f"{package_digest}  {ipk.name}\n")
    (output_dir / f"ottplay-lg-{appinfo['version']}.sha256").write_text(
        f"{package_digest}  {ipk.name}\n")

    evidence_dir = ROOT / "pkg/link-evidence/stable"
    evidence = {}
    if not evidence_dir.is_dir():
        raise ValueError("stable link evidence is missing after the packaging checks")
    copied_evidence = output_dir / "link-evidence"
    shutil.copytree(evidence_dir, copied_evidence, dirs_exist_ok=True)
    for item in sorted(p for p in evidence_dir.rglob("*") if p.is_file()):
        evidence[item.relative_to(evidence_dir).as_posix()] = sha256(item)

    receipt = {
        "schema": 1,
        "artifact_kind": "native-webos-stable-candidate",
        "candidate": {"run_id": run_id, "run_attempt": run_attempt},
        "source": {"repository": run(["git", "config", "--get", "remote.origin.url"]),
                   "commit": source_sha, "tree": source_tree},
        "package": {"filename": copied_ipk.name, "app_id": appinfo["id"],
                    "version": appinfo["version"], "sha256": package_digest,
                    "size_bytes": copied_ipk.stat().st_size,
                    "build_config": stamp, "build_config_sha256": sha256(stamp_path)},
        "toolchain": {
            "runner": "ubuntu-24.04-arm",
            "webos_ndk_version": sdk_manifest["version"],
            "webos_ndk_asset": sdk_asset["name"],
            "webos_ndk_archive_sha256": sdk_asset["sha256"],
            "webos_gcc_sha256": sha256(compiler),
            "webos_gcc_backend_sha256": sha256(backend),
            "rust_toolchain_pin": rust_channel,
            "rustc_version": rustc,
            "cargo_version": cargo,
            "cargo_lock_sha256": sha256(ROOT / "rust-modules/Cargo.lock"),
            "dependency_manifest_sha256": sha256(ROOT / "ci/dependencies.json"),
            "makefile_sha256": sha256(ROOT / "Makefile"),
        },
        "link_evidence_sha256": evidence,
        "checks": ["ci/deps.py check", "ci/check-package.py --selftest",
                   "make FLAVOR=stable RELEASE=1 "
                   "OTT_PRODUCT_CONFIG=ci/ottplay-integration/release/product.json "
                   "ipk-stage and ipk-pack (includes check-link-evidence.py, "
                   "check-package.py and check-packaged-elf.py)"],
    }
    if companion_sources is not None:
        import native_companion
        native = native_companion.native_builder()
        companion = native.verify_ipk(ipk, native.manifest()).get("companion")
        if not companion:
            raise ValueError("Candidate requires a packaged companion")
        source_assets = {}
        names = {"companion-source.tar.gz": "companion-source.tar.gz",
                 "companion-source-closure.json": "source-closure.json",
                 "companion-source-manifest.json": "SOURCE-MANIFEST.json"}
        for target, original in names.items():
            path = companion_sources / original
            if not path.is_file() or path.is_symlink():
                raise ValueError("Missing companion source asset: " + original)
            shutil.copyfile(path, output_dir / target)
            source_assets[target] = sha256(output_dir / target)
        closure = json.loads((output_dir / "companion-source-closure.json").read_text())
        if (closure.get("elf_sha256") != companion["elf_sha256"]
                or closure.get("locked_source_coverage") != "PASS"
                or closure.get("source_manifest_sha256") != source_assets["companion-source-manifest.json"]
                or closure.get("local_inputs_sha256") != companion["local_inputs_sha256"]
                or closure.get("local_source_archive_sha256") != companion["local_source_archive_sha256"]
                or closure.get("source_receipt_sha256") != companion["source_receipt_sha256"]
                or closure.get("source_archive", {}).get("sha256") != source_assets["companion-source.tar.gz"]):
            raise ValueError("Companion source supplement does not match packaged helper")
        receipt["companion"] = {"metadata": companion, "source_assets": source_assets,
                                 "source_revision": json.loads((ROOT / "ci/companion-source.json").read_text())["revision"]}
    receipt_path = output_dir / "candidate-provenance.json"
    receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    (output_dir / "candidate-provenance.json.sha256").write_text(
        f"{sha256(receipt_path)}  {receipt_path.name}\n")
    return receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--run-attempt", required=True)
    parser.add_argument("--sdk", required=True, type=Path)
    parser.add_argument("--ipk", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--companion-sources", type=Path)
    args = parser.parse_args()
    try:
        receipt = create_receipt(args.source_sha, args.run_id, args.run_attempt,
                                 args.sdk, args.ipk, args.output_dir, args.companion_sources)
    except (OSError, ValueError, subprocess.CalledProcessError, KeyError) as error:
        parser.exit(1, f"candidate receipt failed: {error}\n")
    print(f"candidate {receipt['package']['filename']} sha256={receipt['package']['sha256']}")


if __name__ == "__main__":
    main()
