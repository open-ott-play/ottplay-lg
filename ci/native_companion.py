#!/usr/bin/env python3
"""Build and stage the source-pinned app-owned webOS companion."""
import argparse
import importlib.util
import json
import os
import sys
import subprocess
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]


def native_builder():
    spec = importlib.util.spec_from_file_location("native_package_builder", ROOT / "scripts/lg-build.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("build", "stage"))
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--sdk-archive", type=Path)
    parser.add_argument("--target-dir", type=Path)
    args = parser.parse_args()
    policy = json.loads((ROOT / "ci/companion-source.json").read_text())
    if args.operation == "build":
        if args.source.exists():
            raise ValueError("Companion source checkout must be fresh")
        subprocess.run(["git", "init", str(args.source)], check=True)
        subprocess.run(["git", "-C", str(args.source), "fetch", "--depth=1", policy["repository"], policy["revision"]], check=True)
        subprocess.run(["git", "-C", str(args.source), "checkout", "--detach", "FETCH_HEAD"], check=True)
    actual = subprocess.check_output(["git", "-C", str(args.source), "rev-parse", "HEAD"], text=True).strip()
    if actual != policy["revision"] or subprocess.check_output(["git", "-C", str(args.source), "status", "--porcelain", "--untracked-files=no"]):
        raise ValueError("Companion source does not match its committed pin")
    native = native_builder()
    module = native.companion_builder(SimpleNamespace(companion_source=args.source, companion_bundle=args.bundle))
    readelf = args.sdk / "bin/arm-webos-linux-gnueabi-readelf"
    if args.operation == "build":
        if args.sdk_archive is None or args.target_dir is None:
            raise ValueError("Companion build requires SDK archive and target directory")
        prepared_path = args.bundle.parent / "companion-prepared.json"
        subprocess.run([sys.executable, str(args.source / "native/webos/companion/prepare.py"),
                        "--cache-dir", str(args.bundle.parent / "companion-cache"),
                        "--target-dir", str(args.target_dir), "--output", str(prepared_path)], check=True)
        prepared = json.loads(prepared_path.read_text())
        environment = {**os.environ, **prepared["environment"]}
        subprocess.run([sys.executable, str(args.source / "native/webos/companion/build.py"), "build",
                        "--sdk", str(args.sdk), "--sdk-archive", str(args.sdk_archive),
                        "--target-dir", str(args.target_dir), "--output", str(args.bundle),
                        "--libclang", prepared["libclang"], "--jobs", "2"], env=environment, check=True)
        module.checked_bundle(args.bundle, readelf)
        subprocess.run([sys.executable, str(args.source / "native/webos/companion/sources.py"),
                        "--bundle", str(args.bundle), "--output", str(args.bundle.parent / "companion-corresponding-source"),
                        "--readelf", str(readelf), "--rust-sysroot", prepared["rust_sysroot"],
                        "--rust-source-archive", prepared["rust_source_archive"]], env=environment, check=True)
    else:
        app_id = json.loads((ROOT / "pkg/appinfo.json").read_text())["id"]
        stage = ROOT / "ipkroot/data/usr/palm/applications" / app_id
        if not stage.is_dir() or stage.is_symlink():
            raise ValueError("Native staging directory is unavailable")
        module.stage(SimpleNamespace(bundle=args.bundle, readelf=readelf, destination=stage,
                                     link_evidence=ROOT / "pkg/link-evidence/stable"))


if __name__ == "__main__":
    main()
