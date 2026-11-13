#!/usr/bin/env python3
"""Build the checked-out OTTPlay LG sources; never clone, install or launch a player.

Host compilation is development evidence. ARM package receipts describe their exact
source and build configuration, not device acceptance or complete release qualification.
Build/toolchain outputs remain in --work-dir and the Makefile's ignored build paths.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import ctypes
import fnmatch
import hashlib
import importlib.util
import io
import json
import os
import platform
import posixpath
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
HERE = ROOT / "ci/ottplay-integration"
TARGET = "armv7-unknown-linux-gnueabi"
REPOSITORY = "https://github.com/open-ott-play/ottplay-lg"
PRODUCT_CONFIG = "ci/ottplay-integration/release/product.json"
CARGO_EVIDENCE_LIMIT = 8 * 1024 * 1024
PRIVATE_INPUTS = ("src/config.local.h", "local.env", "pkg/auth.json", "pkg/lab.json",
                  "pkg/ottplay-profile.json", "pkg/telemetry.local.json",
                  "rust-modules/platform/src/webos/jail_repair.rs")
_profile_path = HERE / "control_discovery_config.py"
_profile_spec = importlib.util.spec_from_file_location("control_discovery_config", _profile_path)
control_discovery_config = importlib.util.module_from_spec(_profile_spec)
exec(compile(_profile_path.read_bytes(), str(_profile_path), "exec"), control_discovery_config.__dict__)

def control_discovery_profile(args):
    url = getattr(args, "control_discovery_url", None)
    digest = getattr(args, "control_discovery_sha256", None)
    if (url is None) != (digest is None):
        fail("Control discovery requires both explicit build inputs")
    return control_discovery_config.profile(url, digest)

def fail(message: str):
    raise ValueError(message)

def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()

def write_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")

def run(args, *, cwd=None, env=None, capture=False) -> str:
    result = subprocess.run(
        [str(x) for x in args], cwd=cwd, env=env, check=True, text=True, stdout=subprocess.PIPE if capture else None
    )
    return result.stdout.strip() if capture else ""

def check_cargo_configs(source: Path, tree: str):
    """Admit only regular config blobs from the already validated prepared tree."""
    source = source.absolute()
    if source.resolve() != source:
        fail("Native source ancestry must not traverse symlinks")
    cwd = source / "rust-modules"
    for directory in (cwd, *cwd.parents):
        for name in ("config", "config.toml"):
            path = directory / ".cargo" / name
            if not path.exists() and not path.is_symlink():
                continue
            if not path.is_relative_to(source):
                fail("External ancestor Cargo configuration is not permitted: " + str(path))
            if not path.is_file() or path.resolve() != path:
                fail("Repository Cargo configuration must be a regular non-symlink file")
            relative = path.relative_to(source).as_posix()
            entry = run(["git", "-C", source, "ls-tree", tree, "--", relative], capture=True)
            expected = run(["git", "-C", source, "hash-object", "--no-filters", path], capture=True)
            if entry not in ("100644 blob " + expected + "\t" + relative,
                             "100755 blob " + expected + "\t" + relative):
                fail("Cargo configuration differs from the prepared source tree: " + relative)

@contextmanager
def cargo_environment(args):
    """Reuse caches and their locks, never the caller's Cargo configuration or credentials."""
    env = build_env(args)
    cache = Path(env.get("CARGO_HOME") or Path.home() / ".cargo").expanduser().resolve()
    home = args.work_dir / "cargo-home"
    if home.is_symlink() or home.resolve() == cache:
        fail("Isolated Cargo home must be separate from the source cache")
    cache.mkdir(parents=True, exist_ok=True)
    home.mkdir(parents=True, exist_ok=True)
    for name in ("config", "config.toml", "credentials", "credentials.toml"):
        path = home / name
        if path.exists() or path.is_symlink():
            fail("Isolated Cargo home contains configuration or credentials: " + name)
    # Shared locks serialize access even when another Cargo process uses the original home.
    # Global usage tracking is private; do not alias a SQLite database and its journal paths.
    # A stable path matters: Cargo fingerprints the registry source path, so a
    # newly named temporary home forces every dependency to rebuild on every run.
    for name in ("registry", "git", ".package-cache", ".package-cache-mutate"):
        target = cache / name
        directory = name in ("registry", "git")
        if directory:
            target.mkdir(exist_ok=True)
        else:
            target.touch(exist_ok=True)
            if not target.is_file():
                fail("Cargo cache lock must be a regular file")
        alias = home / name
        if alias.exists() or alias.is_symlink():
            if not alias.is_symlink() or alias.resolve() != target.resolve():
                fail("Isolated Cargo cache alias changed: " + name)
        else:
            alias.symlink_to(target.resolve(), target_is_directory=directory)
    env["CARGO_HOME"] = str(home)
    yield env

def toolchain(source: Path) -> str:
    text = (source / "rust-modules/rust-toolchain.toml").read_text()
    match = re.search(r'^channel\s*=\s*"([0-9.]+)"', text, re.MULTILINE)
    if not match:
        fail("Missing exact Rust toolchain pin")
    return match.group(1)

def build_env(args) -> dict:
    profile = control_discovery_profile(args)
    optimization = getattr(args, "rust_opt_level", "2")
    if optimization not in ("2", "s"):
        fail("Native Rust optimization must be 2 or s")
    env = dict(os.environ)
    for key in list(env):
        if (key.startswith(("PLX_", "SENTRY_", "POSTHOG_", "OTTPLAY_", "CARGO_PROFILE_", "CARGO_TARGET_"))
                or key == "BINDGEN_EXTRA_CLANG_ARGS" or key.startswith("BINDGEN_EXTRA_CLANG_ARGS_")
                or re.fullmatch(r"(?:(?:HOST|TARGET)_)?(?:CC|CXX|AR|RANLIB|CFLAGS|CXXFLAGS|ARFLAGS)(?:[_-].*)?", key)
                or key in {
            "RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_BUILD_TARGET_DIR", "CARGO_TARGET_DIR", "CARGO_BUILD_RUSTC",
            "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
            "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "RUSTUP_TOOLCHAIN",
            "CARGO_ENCODED_RUSTFLAGS",
            "MAKEFLAGS",
            "GNUMAKEFLAGS",
            "MAKEOVERRIDES",
            "MAKEFILES",
            "MFLAGS",
            "ARM_PROFILE", "DEBUG", "HOST", "HOSTSIM", "CAPLINTS", "WERROR",
            "CFLAGS", "CPPFLAGS", "CXXFLAGS", "LDFLAGS", "PKG_CONFIG_PATH",
            "SIM_TDIR", "HOST_THREADS", "RELEASE", "FLAVOR", "STORAGE_TDIR", "FFMPEG_PREFIX", "SENTRY_NATIVE_PREFIX", "LIBASS_UNSTRIPPED",
            "LAB",
            "SYMBOLS",
            "RUST_FEATFLAGS",
            "RUST_DEBUGINFO", "RUST_CAPLINTS", "RUST_CODEGEN", "RUST_ENV", "RUST_REMAP", "RC",
            "RUST_TOOLCHAIN",
            "RUST_TARGET",
            "RUST_TDIR",
            "OTT_PRODUCT_CONFIG",
            "REQUIRE_PACKAGE", "CROSS_COMPILE", "CRATE_CC_NO_DEFAULTS", "LIBCLANG_PATH",
            "LIBCLANG_STATIC_PATH", "LLVM_CONFIG_PATH", "CLANG_PATH",
        }):
            env.pop(key)
    env.update(
        {
            "WEBOS_SDK": str(args.sdk),
            "RELEASE": "1",
            "FLAVOR": "stable",
            # Link-time archives must remain beneath the recorded native source.
            "PLX_BUILD_CACHE": str(source_dir(args) / "vendor/build-cache"),
            "PLX_DEPS_CACHE": str(source_dir(args) / "vendor/downloads"),
            "PLX_CARGO_SEED": "off",
            "PLX_DISK_BUDGET_DONE": "1",
            "CARGO_INCREMENTAL": "0",
            "CARGO_BUILD_JOBS": str(getattr(args, "jobs", 2)),
            "JOBS": str(getattr(args, "jobs", 2)),
            "CARGO_PROFILE_RELEASE_OPT_LEVEL": optimization,
            "CARGO_TERM_VERBOSE": "true",
        }
    )
    if profile["configured"]:
        env[control_discovery_config.URL_ENV] = profile["url"]
        env[control_discovery_config.SHA256_ENV] = profile["sha256"]
    return env

def native_compile_evidence(args, spec, source, configuration):
    """Validate the actual Cargo result before the native linker can consume it."""
    target = source / "rust-modules/target-release"
    raw_path = target / ".lib-artifacts.json"
    archive = target / TARGET / "release/libplxnative_modules.a"
    for path in (raw_path, archive):
        if not path.is_file() or path.resolve() != path.absolute():
            fail("Native compilation evidence must remain inside its source tree")
    with raw_path.open("rb") as stream:
        raw = stream.read(CARGO_EVIDENCE_LIMIT + 1)
    if len(raw) > CARGO_EVIDENCE_LIMIT:
        fail("Native Cargo evidence exceeds the 8 MiB release limit")
    try:
        records = [json.loads(line) for line in raw.splitlines() if line.strip()]
    except ValueError:
        fail("Malformed native Cargo evidence")
    if not records or any(not isinstance(row, dict) for row in records):
        fail("Missing native Cargo evidence")
    finished = [row for row in records if row.get("reason") == "build-finished"]
    if len(finished) != 1 or finished[0].get("success") is not True or records[-1] != finished[0]:
        fail("Native Cargo evidence does not end in a successful build")
    artifacts = [row for row in records if row.get("reason") == "compiler-artifact"
                 and isinstance(row.get("target"), dict)
                 and row["target"].get("name") == "plxnative_modules"]
    if len(artifacts) != 1:
        fail("Expected exactly one native application Cargo artifact")
    artifact = artifacts[0]
    expected_profile = {"opt_level": getattr(args, "rust_opt_level", "2"), "debuginfo": 0,
                        "debug_assertions": False, "overflow_checks": False, "test": False}
    actual_profile = artifact.get("profile", {})
    if (artifact.get("manifest_path") != str(source / "rust-modules/Cargo.toml")
            or artifact["target"].get("src_path") != str(source / "rust-modules/src/lib.rs")
            or artifact["target"].get("kind") != ["staticlib"]
            or artifact["target"].get("crate_types") != ["staticlib"]
            or artifact.get("features") != []
            or any(type(actual_profile.get(key)) is not type(value) or actual_profile.get(key) != value
                   for key, value in expected_profile.items())
            or artifact.get("filenames") != [str(archive)]):
        fail("Native Cargo artifact differs from the selected release configuration")
    name = output_name(spec)
    cargo_file, config_file = name + "-cargo-artifacts.jsonl", name + "-build-config.txt"
    for evidence in (cargo_file, config_file):
        if (args.work_dir / evidence).is_symlink():
            fail("Native compilation evidence destination must not be a symlink")
    (args.work_dir / cargo_file).write_bytes(raw)
    (args.work_dir / config_file).write_text(configuration + "\n")
    return {"cargo": {"file": cargo_file, "sha256": sha256(args.work_dir / cargo_file),
                       "artifact": artifact},
            "make": {"file": config_file, "sha256": sha256(args.work_dir / config_file)},
            "archive": {"path": archive.relative_to(source).as_posix(), "sha256": sha256(archive)}}

def sdk_host() -> str:
    system, machine = platform.system().lower(), platform.machine().lower()
    if system == "darwin" and machine in {"arm64", "x86_64"}:
        return system + "-" + machine
    if system == "linux" and machine in {"aarch64", "arm64"}:
        return "linux-aarch64"
    fail("Pinned SDK supports macOS arm64/x86_64 and Linux aarch64; use an ARM Linux CI runner")

def sdk_extract_filter(member, destination, sdk_directory):
    """Keep target absolute symlinks inside the extracted target sysroot."""
    name = PurePosixPath(safe_name(member.name))
    if member.issym() and PurePosixPath(member.linkname).is_absolute():
        sysroot = PurePosixPath(sdk_directory) / "arm-webos-linux-gnueabi/sysroot"
        if not name.is_relative_to(sysroot):
            fail("Absolute SDK link is outside its target sysroot: " + member.name)
        target = sysroot / safe_name(member.linkname.lstrip("/"))
        root = Path(destination).resolve()
        boundary = root.joinpath(*sysroot.parts)
        if (
            boundary.resolve() != boundary
            or not root.joinpath(*name.parent.parts).resolve().is_relative_to(boundary)
            or not root.joinpath(*target.parts).resolve().is_relative_to(boundary)
        ):
            fail("SDK symlink target or ancestor leaves its target sysroot: " + member.name)
        member = member.replace(linkname=posixpath.relpath(str(target), str(name.parent)))
    return tarfile.data_filter(member, destination)

def bootstrap(args, spec):
    source = check_source(args, spec)
    find_libclang(getattr(args, "libclang", None))
    env = build_env(args)
    compiler = args.sdk / "bin/arm-webos-linux-gnueabi-gcc"
    if not compiler.is_file():
        if args.sdk.exists():
            fail("SDK directory exists without its compiler; choose an empty SDK destination")
        host = sdk_host()
        dependencies = json.loads((source / "ci/dependencies.json").read_text())
        sdk = next(row for row in dependencies if row["id"] == "webos-ndk")
        asset = sdk["assets"][host]
        if sdk["version"] != spec["sdk"]["version"] or asset["sha256"] != spec["sdk"]["assets"][host]["sha256"]:
            fail("Pinned upstream SDK does not match reviewed integration manifest")
        args.sdk.parent.mkdir(parents=True, exist_ok=True)
        archive = Path(
            run(
                [
                    sys.executable,
                    source / "ci/deps.py",
                    "fetch",
                    "webos-ndk",
                    "--asset",
                    host,
                    "--into",
                    args.work_dir / "downloads",
                ],
                cwd=source,
                env=env,
                capture=True,
            ).splitlines()[-1]
        )
        if sha256(archive) != asset["sha256"]:
            fail("SDK checksum mismatch")
        # The archive has been authenticated before extraction. Refuse path traversal
        # even for pinned inputs, while preserving the SDK's intentional symlinks.
        with tempfile.TemporaryDirectory(prefix="sdk-extract-", dir=args.sdk.parent) as staging:
            with tarfile.open(archive) as tar:
                for entry in tar.getmembers():
                    safe_name(entry.name)
                tar.extractall(
                    staging,
                    filter=lambda member, destination: sdk_extract_filter(
                        member, destination, spec["sdk"]["directory"]
                    ),
                )
            canonical = Path(staging) / spec["sdk"]["directory"]
            canonical.rename(args.sdk)
        run(["sh", "./relocate-sdk.sh"], cwd=args.sdk, env=env)
    run(
        [
            "rustup",
            "toolchain",
            "install",
            "--no-self-update",
            toolchain(source),
            "--profile",
            "minimal",
            "--component",
            "rust-src",
            "--target",
            TARGET,
        ],
        env=env,
    )
    run([compiler, "--version"], env=env)

def verify_companion(files, prefix, data_archive):
    binary_name = prefix + "bin/ottplay-companion"
    metadata_prefix = prefix + "companion/"
    if binary_name not in files and not any(name.startswith(metadata_prefix) for name in files):
        return None
    required = [binary_name] + [metadata_prefix + name for name in ("manifest.json", "receipt.json", "source.json")]
    if any(name not in files for name in required):
        fail("Incomplete native companion payload")
    binary = files[binary_name]
    manifest = json.loads(files[metadata_prefix + "manifest.json"])
    receipt = json.loads(files[metadata_prefix + "receipt.json"])
    source = json.loads(files[metadata_prefix + "source.json"])
    if (
        not elf_arm(binary)
        or manifest.get("schema") != 1
        or receipt.get("schema") != 1
        or manifest.get("package") != "ottplay-companion"
        or manifest.get("target") != TARGET
        or manifest.get("install_path") != "bin/ottplay-companion"
        or receipt.get("package") != manifest["package"]
        or receipt.get("target") != TARGET
        or receipt.get("elf", {}).get("sha256") != hashlib.sha256(binary).hexdigest()
        or receipt.get("elf", {}).get("bytes") != len(binary)
        or receipt.get("manifest_sha256") != hashlib.sha256(files[metadata_prefix + "manifest.json"]).hexdigest()
    ):
        fail("Native companion identity or binary receipt mismatch")
    if (
        source.get("schema") != 1
        or source.get("local_source_archive_sha256") != receipt.get("local_source_archive_sha256")
        or source.get("complete_corresponding_source") is not False
        or source.get("release_source_requirements") != manifest.get("release_source_requirements")
        or receipt.get("device_executed") is not False
        or source.get("local_inputs_sha256") != receipt.get("local_inputs_sha256")
    ):
        fail("Native companion source or validation receipt mismatch")
    for value in (
        receipt.get("manifest_sha256"),
        receipt.get("source_receipt_sha256"),
        receipt.get("local_source_archive_sha256"),
        source.get("local_inputs_sha256"),
        receipt.get("local_inputs_sha256"),
        receipt.get("link_map_sha256"),
        receipt.get("link_trace_sha256"),
        receipt.get("link_receipt_sha256"),
    ):
        if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value):
            fail("Native companion receipt is missing a source or linker digest")
    with tarfile.open(fileobj=io.BytesIO(data_archive), mode="r:gz") as archive:
        member = next(item for item in archive if item.name.removeprefix("./") == binary_name)
        if member.mode != 0o755:
            fail("Native companion must be executable with mode 0755")
    return {
        "elf_sha256": hashlib.sha256(binary).hexdigest(),
        "receipt_sha256": hashlib.sha256(files[metadata_prefix + "receipt.json"]).hexdigest(),
        "manifest_sha256": receipt["manifest_sha256"],
        "source_receipt_sha256": receipt["source_receipt_sha256"],
        "local_source_archive_sha256": receipt["local_source_archive_sha256"],
        "local_inputs_sha256": source["local_inputs_sha256"],
    }

def safe_name(value: str) -> str:
    path = PurePosixPath(value)
    if not value or path.is_absolute() or ".." in path.parts or "\\" in value:
        fail("Unsafe archive member: " + value)
    return str(path)

def ar_members(data: bytes) -> dict[str, bytes]:
    if not data.startswith(b"!<arch>\n"):
        fail("Not an IPK/ar archive")
    result, offset = {}, 8
    while offset < len(data):
        header = data[offset : offset + 60]
        if len(header) != 60 or header[58:] != b"`\n":
            fail("Malformed IPK member header")
        name = safe_name(header[:16].decode("ascii").strip().rstrip("/"))
        size = int(header[48:58].decode("ascii").strip())
        if name in result or size < 0 or offset + 60 + size > len(data):
            fail("Duplicate or truncated IPK member")
        result[name] = data[offset + 60 : offset + 60 + size]
        offset += 60 + size + size % 2
    if offset != len(data) or set(result) != {"debian-binary", "control.tar.gz", "data.tar.gz"}:
        fail("Unexpected IPK structure")
    if result["debian-binary"] != b"2.0\n":
        fail("Unsupported IPK format")
    return result

def tar_files(data: bytes, allowed=None) -> dict[str, bytes]:
    result = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        for member in tar:
            name = safe_name(member.name)
            if allowed is not None and not allowed(name):
                fail("Unexpected package path: " + name)
            if member.isdir():
                continue
            if not member.isfile() or name in result or member.size > 512 * 1024 * 1024:
                fail("Unsupported/duplicate IPK payload member: " + name)
            result[name] = tar.extractfile(member).read()
    return result

def elf_arm(data: bytes) -> bool:
    return len(data) >= 52 and data[:6] == b"\x7fELF\x01\x01" and int.from_bytes(data[18:20], "little") == 40

def verify_ipk(path: Path, spec) -> dict:
    members = ar_members(path.read_bytes())
    app = spec["application"]
    prefix = "usr/palm/applications/" + app["id"] + "/"
    service_id = app["id"] + ".storage"
    service_prefix = "usr/palm/services/" + service_id + "/"
    package_prefix = "usr/palm/packages/" + app["id"] + "/"
    directories = {
        ".",
        "usr",
        "usr/palm",
        "usr/palm/applications",
        prefix.rstrip("/"),
        "usr/palm/services",
        service_prefix.rstrip("/"),
        "usr/palm/packages",
        package_prefix.rstrip("/"),
    }
    auxiliary = {
        service_prefix + "plxnative-storage",
        service_prefix + "services.json",
        package_prefix + "packageinfo.json",
    }
    files = tar_files(
        members["data.tar.gz"], lambda name: name in directories or name in auxiliary or name.startswith(prefix)
    )
    controls = tar_files(members["control.tar.gz"], lambda name: name in {".", "control"})
    if set(controls) != {"control"}:
        fail("Only package control metadata is permitted; installer hooks are forbidden")
    control = {}
    for line in controls["control"].decode().splitlines():
        if not line:
            continue
        field, separator, value = line.partition(":")
        field = field.lower()
        if not separator or field in control:
            fail("Malformed or duplicate IPK control field")
        control[field] = value.strip()
    descriptor = json.loads(files[prefix + "appinfo.json"])
    if any(descriptor.get(key) != app[key] for key in ("id", "title", "version")) or descriptor.get("type") != "native":
        fail("Packaged native identity mismatch")
    if (
        control.get("package") != app["id"]
        or control.get("version") != app["version"]
        or control.get("architecture") != "arm"
        or control.get("webos-package-format-version") != "2"
    ):
        fail("IPK control identity mismatch")
    if control.get("license") != "GPL-3.0-or-later":
        fail("GPL native package license missing")
    license_bytes = files.get(prefix + "licenses/LICENSE", b"")
    if hashlib.sha256(license_bytes).hexdigest() != sha256(ROOT / "LICENSE"):
        fail("Complete native GPL license text missing")
    if prefix + "THIRD-PARTY-NOTICES.md" not in files:
        fail("Native dependency notices missing")
    private_names = {"ottplay-profile.json", "ottplay-control.json", "ott-state", "ott-state.json"}
    if any(private_names.intersection(name.split("/")) for name in files):
        fail("Private profiles, control credentials and state must never be distributed in the IPK")
    if descriptor.get("main") != "plxnative":
        fail("Unexpected native application entry point")
    binary = files[prefix + descriptor["main"]]
    if not elf_arm(binary):
        fail("Native application must be a little-endian ELF32 ARM executable")
    storage = service_prefix + "plxnative-storage"
    if not elf_arm(files[storage]):
        fail("Native storage service missing or wrong architecture")
    service = json.loads(files[service_prefix + "services.json"])
    if (
        service.get("id") != service_id
        or service.get("engine") != "native"
        or service.get("executable") != "plxnative-storage"
        or service.get("services") != [{"name": service_id, "commands": []}]
    ):
        fail("Storage service identity or activation scope mismatch")
    package_info = json.loads(files[package_prefix + "packageinfo.json"])
    if (
        any(package_info.get(key) != app[key] for key in ("id", "version"))
        or package_info.get("app") != app["id"]
        or package_info.get("services") != [service_id]
        or package_info.get("package_format_version") != 2
    ):
        fail("Package descriptor app/service ownership mismatch")
    if package_info.get("requiredPermissions") != ["database.operation", "securitykey.operation"]:
        fail("Unexpected native service permission scope")
    if not any(name.startswith(prefix + "libavformat") for name in files):
        fail("Bundled native demuxer missing")
    if any(name.startswith("usr/palm/applications/") and not name.startswith(prefix) for name in files):
        fail("Package would install another application")
    result = {
        "application": app,
        "size": path.stat().st_size,
        "sha256": sha256(path),
        "architecture": "arm",
        "type": "native",
        "rootRequired": False,
        "payload_bytes": sum(len(data) for data in files.values()),
    }
    companion = verify_companion(files, prefix, members["data.tar.gz"])
    if companion is not None:
        result["companion"] = companion
    return result


def manifest() -> dict:
    """SDK provenance is pinned; product identity comes from this repository."""
    spec = json.loads((HERE / "upstream.json").read_text())
    app = json.loads((ROOT / "pkg/appinfo.json").read_text())
    if (app.get("id") != "com.openottplay.player.native" or app.get("type") != "native"
            or not re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", app.get("version", ""))):
        fail("Unexpected native product identity")
    spec["application"] = {key: app[key] for key in ("id", "title", "version")}
    return spec


def source_dir(args) -> Path:
    return ROOT


def source_receipt(args) -> Path:
    return args.work_dir / "source.json"


def output_name(spec) -> str:
    return "ottplay-lg-" + spec["application"]["version"]


def reject_private_inputs():
    for relative in PRIVATE_INPUTS:
        path = ROOT / relative
        if path.exists() or path.is_symlink():
            fail("Remove private or excluded build input before using this builder: " + relative)


def source_record() -> dict:
    """An ARM receipt binds a committed clean checkout, including build recipes."""
    reject_private_inputs()
    if run(["git", "rev-parse", "--show-toplevel"], cwd=ROOT, capture=True) != str(ROOT):
        fail("Run from the primary ottplay-lg repository, not an enclosing repository")
    revision = run(["git", "rev-parse", "--verify", "HEAD"], cwd=ROOT, capture=True)
    if run(["git", "status", "--porcelain", "--untracked-files=all"], cwd=ROOT, capture=True):
        fail("ARM packaging requires a committed clean source tree; commit reviewed source first")
    tree = run(["git", "rev-parse", "HEAD^{tree}"], cwd=ROOT, capture=True)
    check_cargo_configs(ROOT, tree)
    config_path = ROOT / PRODUCT_CONFIG
    config = json.loads(config_path.read_text())
    documents = {}
    for key in ("notes", "audit"):
        relative = safe_name(config[key])
        path = ROOT / relative
        if path.resolve() != path.absolute() or not path.is_file():
            fail("Product release document must be a regular file")
        documents[key] = {"path": relative, "sha256": sha256(path)}
    return {"schema": 1, "repository": REPOSITORY, "revision": revision, "tree": tree,
            "product_release": {"path": PRODUCT_CONFIG, "sha256": sha256(config_path),
                                "documents": documents, "audit_evidence": "authored_only"}}


def prepare(args, spec):
    record = source_record()
    args.work_dir.mkdir(parents=True, exist_ok=True)
    receipt = source_receipt(args)
    if receipt.exists() and json.loads(receipt.read_text()) != record:
        fail("Build directory belongs to another source revision; choose a fresh --work-dir")
    write_json(receipt, record)
    return record


def check_source(args, spec):
    if not source_receipt(args).is_file():
        fail("Run prepare before building")
    if json.loads(source_receipt(args).read_text()) != source_record():
        fail("Source changed after preparation; use a fresh build directory")
    return ROOT


def companion_builder(args):
    source = getattr(args, "companion_source", None)
    bundle = getattr(args, "companion_bundle", None)
    if (source is None) != (bundle is None):
        fail("A companion requires both --companion-bundle and --companion-source")
    if source is None:
        return None
    source = source.resolve(strict=True)
    policy = json.loads((ROOT / "ci/companion-source.json").read_text())
    for relative, digest in policy["verifier_files"].items():
        path = source / safe_name(relative)
        if path.resolve() != path.absolute() or not path.is_file() or sha256(path) != digest:
            fail("Companion verifier does not match the reviewed source: " + relative)
    path = source / "native/webos/companion/build.py"
    loader = importlib.util.spec_from_file_location("lg_verified_companion", path)
    module = importlib.util.module_from_spec(loader)
    bytecode_policy = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        loader.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = bytecode_policy
    return module


def probe_libclang(path):
    """Confirm the selected host library can create an index; no compilation."""
    library = ctypes.CDLL(str(path))
    library.clang_createIndex.argtypes = [ctypes.c_int, ctypes.c_int]
    library.clang_createIndex.restype = ctypes.c_void_p
    library.clang_disposeIndex.argtypes = [ctypes.c_void_p]
    library.clang_disposeIndex.restype = None
    index = library.clang_createIndex(0, 0)
    if not index:
        fail("Selected libclang could not create an index")
    library.clang_disposeIndex(index)


def find_libclang(explicit=None):
    """Select one discoverable library in a deterministic order, without env overrides."""
    if platform.system() == "Darwin":
        patterns = ("libclang.dylib",)
        directories = (Path("/opt/homebrew/opt/llvm/lib"), Path("/usr/local/opt/llvm/lib"))
    elif platform.system() == "Linux":
        patterns = ("libclang.so", "libclang-*.so", "libclang.so.*", "libclang-*.so.*")
        directories = tuple(Path(f"/usr/lib/llvm-{version}/lib") for version in range(22, 13, -1))
    else:
        fail("Native ARM binding generation supports macOS and Linux hosts")
    paths = (Path(explicit).expanduser(),) if explicit is not None else directories
    for path in paths:
        candidates = [path] if path.is_file() else sorted({candidate for pattern in patterns for candidate in path.glob(pattern)})
        for candidate in candidates:
            if not candidate.is_file():
                continue
            if "-cpp." in candidate.name or not any(fnmatch.fnmatchcase(candidate.name, pattern) for pattern in patterns):
                if explicit is not None and path.is_file():
                    fail("Selected libclang filename is not discoverable by clang-sys")
                continue
            # Keep the valid distro symlink name: clang-sys validates its basename.
            candidate = candidate.absolute()
            try:
                probe_libclang(candidate.resolve(strict=True))
            except (OSError, AttributeError) as error:
                fail("Selected libclang is not usable: " + str(error))
            return candidate
    fail("Host libclang is required for ARM bindings; install libclang-18-dev on Ubuntu or llvm with Homebrew, or pass --libclang /path/to/library-or-directory")


def build(args, spec):
    source = check_source(args, spec)
    module = companion_builder(args)
    compiler = args.sdk / "bin/arm-webos-linux-gnueabi-gcc"
    if not compiler.is_file():
        fail("SDK is missing; run bootstrap or supply --sdk")
    libclang = find_libclang(getattr(args, "libclang", None))
    libclang_digest = sha256(libclang)
    expected_companion = None
    readelf = compiler.with_name("arm-webos-linux-gnueabi-readelf")
    if module is not None:
        module.checked_bundle(args.companion_bundle, readelf)
        expected_companion = sha256(args.companion_bundle / "receipt.json")
    with cargo_environment(args) as environment:
        environment["LIBCLANG_PATH"] = str(libclang)
        make = ["make", "-j" + str(args.jobs), "RELEASE=1", "FLAVOR=stable",
                "OTT_PRODUCT_CONFIG=" + PRODUCT_CONFIG, "ARM_PROFILE=release",
                "RUST_TDIR=target-release", "STORAGE_TDIR=target-release-storage",
                "RUST_TARGET=" + TARGET, "RUST_TOOLCHAIN=" + toolchain(source)]
        environment["REQUIRE_PACKAGE"] = "1"
        (args.work_dir / "built.json").unlink(missing_ok=True)
        configuration = run(make + ["-s", "print-bench-config"], cwd=source, env=environment, capture=True)
        settings = dict(line.split("=", 1) for line in configuration.splitlines() if "=" in line)
        archive = "rust-modules/target-release/" + TARGET + "/release/libplxnative_modules.a"
        expected = {"RUST_TDIR": "target-release", "RUST_TARGET": TARGET,
                    "ARM_PROFILE": "release", "ARM_PROFILE_FLAG": "--release", "RUST_LIB": archive,
                    "RUST_FEATFLAGS": "--no-default-features", "RUST_TOOLCHAIN": toolchain(source)}
        if any(settings.get(key) != value for key, value in expected.items()):
            fail("Native Make configuration differs from the selected stable build")
        run(make + ["-W", "rust-modules/Cargo.toml", archive, "pkg/plxnative-storage"], cwd=source, env=environment)
        evidence = native_compile_evidence(args, spec, source, configuration)
        run(make + ["-W", archive, "ipk-stage"], cwd=source, env=environment)
        if (sha256(source / archive) != evidence["archive"]["sha256"]
                or sha256(source / "rust-modules/target-release/.lib-artifacts.json") != evidence["cargo"]["sha256"]):
            fail("Compilation evidence changed during final link")
        stage = source / "ipkroot/data/usr/palm/applications" / spec["application"]["id"]
        if not stage.is_dir() or stage.is_symlink():
            fail("Native application staging directory is missing or unsafe")
        run([sys.executable, source / "ci/check-build-budgets.py", "--binary", stage / "plxnative"], cwd=source, env=environment)
        run(["bash", source / "ci/check-elf.sh", source / "pkg/plxnative"], cwd=source, env=environment)
        if module is not None:
            module.stage(argparse.Namespace(bundle=args.companion_bundle, readelf=readelf,
                                            destination=stage, link_evidence=source / "pkg/link-evidence/stable"))
        run(make + ["ipk-pack"], cwd=source, env=environment)
        check_source(args, spec)
        if sha256(libclang) != libclang_digest:
            fail("Selected libclang changed during the ARM build")
        app = spec["application"]
        package_path = source / "pkg" / (app["id"] + "_" + app["version"] + "_arm.ipk")
        verified = verify_ipk(package_path, spec)
        if verified.get("companion", {}).get("receipt_sha256") != expected_companion:
            fail("Packaged companion differs from the explicitly verified bundle")
        receipt = {"source": source_record(), "ipk_sha256": verified["sha256"], "compilation": evidence,
                   "sdk": {"version": spec["sdk"]["version"],
                           "compiler_sha256": sha256(compiler.resolve()),
                           "backend_sha256": sha256(compiler.with_name("arm-webos-linux-gnueabi-gcc.br_real")),
                           "linked_runtime_source_attestation": "not_performed"},
                   "binding_generator": {"library": libclang.name, "sha256": libclang_digest,
                                         "host": platform.system().lower() + "-" + platform.machine().lower()},
                   "configuration": {"RELEASE": "1", "FLAVOR": "stable", "target": TARGET,
                                     "rust_toolchain": toolchain(source), "rust_opt_level": args.rust_opt_level,
                                     "control_discovery": control_discovery_profile(args)},
                   "companion": verified.get("companion"), "device_validation": "not_performed",
                   "complete_corresponding_source": False, "release_qualified": False}
        write_json(args.work_dir / "built.json", receipt)


def package(args, spec):
    check_source(args, spec)
    receipt = json.loads((args.work_dir / "built.json").read_text())
    app = spec["application"]
    binary = ROOT / "pkg" / (app["id"] + "_" + app["version"] + "_arm.ipk")
    verified = verify_ipk(binary, spec)
    if (receipt.get("source") != source_record() or receipt.get("ipk_sha256") != verified["sha256"]
            or receipt.get("companion") != verified.get("companion")):
        fail("Package no longer matches the recorded source/build")
    args.output.mkdir(parents=True, exist_ok=True)
    for key in ("cargo", "make"):
        row = receipt["compilation"][key]
        if Path(row["file"]).name != row["file"] or sha256(args.work_dir / row["file"]) != row["sha256"]:
            fail("Compilation evidence changed before packaging")
        destination = args.output / row["file"]
        if destination.is_symlink():
            fail("Package output must not follow symlinks")
        if destination != args.work_dir / row["file"]:
            shutil.copy2(args.work_dir / row["file"], destination)
    for suffix in (".ipk", ".json", ".sha256"):
        if (args.output / (output_name(spec) + suffix)).is_symlink():
            fail("Package output must not follow symlinks")
    destination = args.output / (output_name(spec) + ".ipk")
    shutil.copy2(binary, destination)
    write_json(args.output / (output_name(spec) + ".json"), {**verified, "build": receipt})
    (args.output / (output_name(spec) + ".sha256")).write_text(verified["sha256"] + "  " + destination.name + "\n")
    print("Candidate package: " + str(destination))
    print("Not release-qualified: matching runtime/dependency sources and device acceptance remain separate.")


def host(args, spec):
    reject_private_inputs()
    args.work_dir.mkdir(parents=True, exist_ok=True)
    if args.command == "bootstrap-host":
        run(["rustup", "toolchain", "install", "--no-self-update", toolchain(ROOT), "--profile", "minimal",
             "--component", "clippy,rustfmt,rust-src"])
        return
    with cargo_environment(args) as environment:
        environment.update(RELEASE="", FLAVOR="debug", PLX_TEST_JOBS=str(args.jobs), HOST_THREADS="0",
                           CARGO_BUILD_JOBS=str(args.jobs), CARGO_TARGET_DIR=str(args.work_dir / "target"),
                           PYTHONDONTWRITEBYTECODE="1")
        environment.pop("CARGO_PROFILE_RELEASE_OPT_LEVEL", None)
        environment.pop("CARGO_TERM_VERBOSE", None)
        make = ["make", "-j" + str(args.jobs), "OTT_PRODUCT_CONFIG=" + PRODUCT_CONFIG]
        if args.command == "host":
            target = {"darwin": "sim-macos", "linux": "sim-linux"}.get(platform.system().lower())
            if target is None:
                fail("Host renderer supports macOS and Linux")
            run(make + ["SIM_TDIR=" + str(args.work_dir / "target-sim"), target], cwd=ROOT, env=environment)
        else:
            check(args, environment)


def check_commands(args, environment):
    """Owned source contracts, then native behavior and shipping-feature checks.

    Imported workflow/site/history tests describe another repository's publication
    infrastructure. The standalone gate names its product checks explicitly.
    """
    python = sys.executable
    contracts = [
        [python, "scripts/test_lg_build.py"],
        [python, "ci/test_cargo_test_parallel.py"],
        [python, "ci/gen-third-party-notices.py", "--check", "--verify-graph"],
        [python, "ci/test_package_product_config.py"],
        [python, "ci/test_rust_c_toolchain.py"],
        [python, "ci/check-package.py", "--tracked", "--product-config", PRODUCT_CONFIG],
        [python, "ci/deps.py", "check"],
        [python, "ci/check-module-layers.py"],
        [python, "ci/check-localization.py"],
        ["node", "scripts/check-shared-core.cjs"],
    ]
    cargo = ["cargo", "+" + toolchain(ROOT)]
    # Keep the existing per-binary runtime/TMPDIR isolation and machine-wide
    # build slots. Explicit -p names make a missing test executable fail closed.
    metadata = json.loads(run(cargo + ["metadata", "--locked", "--no-deps", "--format-version=1",
                                       "--manifest-path", ROOT / "rust-modules/Cargo.toml"],
                              cwd=ROOT, env=environment, capture=True))
    libraries = sorted(package["name"] for package in metadata["packages"]
                       if any({"lib", "rlib", "staticlib", "cdylib"}.intersection(target["kind"])
                              for target in package["targets"]))
    if not libraries or "plxnative-modules" not in libraries or "plx_shared_core" not in libraries:
        fail("Native test workspace is missing the application or shared-core library")
    packages = [value for name in libraries for value in ("-p", name)]
    lock = [python, str(ROOT / "tools/check-lock.py"), "--"]
    runner = [python, str(ROOT / "tools/cargo-test-parallel.py"), "--jobs", str(args.jobs), "--"]
    units = cargo + ["test", "--locked", "--lib", *packages]
    # Resolve locale data before starting unrelated media binaries: an ICU data
    # incompatibility otherwise fans out into many failures and can strand a
    # consumer waiting for a locale-dependent worker. The complete suite below
    # is retained for both normal and hostsim configurations.
    locale_preflight = (runner[:-1] + ["--filter", "i18n::tests", "--require-match", "--"]
                        + cargo + ["test", "--locked", "-p", "plx_platform", "--lib"])
    native = [lock + locale_preflight,
              lock + runner + units,
              lock + runner + units + ["--features", "hostsim"],
              lock + cargo + ["test", "--locked", "-p", "plx_shared_core", "--test", "seek"],
              lock + cargo + ["test", "--locked", "-p", "plxnative-storage", "--bin", "plxnative-storage"],
              lock + cargo + ["check", "--locked", "--lib", "--no-default-features"]]
    return [(ROOT, command) for command in contracts] + [(ROOT / "rust-modules", command) for command in native]


def check(args, environment):
    for directory, command in check_commands(args, environment):
        print("Standalone check:", " ".join(str(value) for value in command), flush=True)
        run(command, cwd=directory, env=environment)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("command", choices=("prepare", "bootstrap", "bootstrap-host", "host", "check",
                                            "build", "package", "verify", "all"))
    parser.add_argument("--work-dir", type=Path, default=ROOT / "build/lg")
    parser.add_argument("--sdk", type=Path, help="Explicit verified webOS SDK directory")
    parser.add_argument("--libclang", type=Path, help="Host libclang library or directory for ARM binding generation")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--package", type=Path, help="Candidate IPK to inspect with verify")
    parser.add_argument("--jobs", type=int, choices=(1, 2), default=2)
    parser.add_argument("--rust-opt-level", choices=("2", "s"), default="s")
    parser.add_argument("--companion-bundle", type=Path, help="Optional prebuilt bundle with matching source/link evidence")
    parser.add_argument("--companion-source", type=Path, help="Explicit source checkout matching that companion bundle")
    parser.add_argument("--control-discovery-url")
    parser.add_argument("--control-discovery-sha256")
    args = parser.parse_args(argv)
    control_discovery_profile(args)
    if (args.control_discovery_url is not None or args.control_discovery_sha256 is not None) and args.command not in ("build", "all"):
        fail("Discovery configuration is only accepted by build/all")
    if (args.companion_bundle is None) != (args.companion_source is None):
        fail("A companion requires both --companion-bundle and --companion-source")
    if args.companion_bundle is not None and args.command not in ("build", "all"):
        fail("Companion inputs are only accepted by build/all")
    if args.package is not None and args.command != "verify":
        fail("--package is only accepted by verify")
    if args.libclang is not None and args.command not in ("bootstrap", "build", "all"):
        fail("--libclang is only accepted by bootstrap/build/all")
    args.work_dir = args.work_dir.expanduser().resolve()
    args.output = (args.output or args.work_dir / "dist").expanduser().resolve()
    spec = manifest()
    args.sdk = (args.sdk or args.work_dir / "toolchain" / spec["sdk"]["directory"]).expanduser().resolve()
    for name in ("companion_bundle", "companion_source"):
        value = getattr(args, name)
        if value is not None:
            setattr(args, name, value.expanduser().resolve(strict=True))
    if args.command in ("bootstrap-host", "host", "check"):
        host(args, spec)
    elif args.command == "verify":
        binary = args.package or args.output / (output_name(spec) + ".ipk")
        print(json.dumps({**verify_ipk(binary, spec), "scope": "package-structure-only",
                          "device_validation": "not_performed", "release_qualified": False}, indent=2))
    elif args.command == "all":
        for command in (prepare, bootstrap, build, package):
            command(args, spec)
    else:
        if args.command in ("bootstrap", "build"):
            prepare(args, spec)
        {"prepare": prepare, "bootstrap": bootstrap, "build": build, "package": package}[args.command](args, spec)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, tarfile.TarError, subprocess.CalledProcessError) as error:
        print("OTTPlay LG build failed: " + str(error), file=sys.stderr)
        sys.exit(1)
