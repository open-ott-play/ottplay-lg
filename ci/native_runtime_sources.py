#!/usr/bin/env python3
"""Collect source coverage for actual incorporated SDK runtime linker inputs.

No source is inferred from a library name alone: input bytes must match the
pinned SDK archive; source archives and the build/patch recipes are pinned too.
Shared firmware libraries are recorded separately and must not be in the IPK.
"""
from __future__ import annotations
import argparse
import hashlib
import importlib.util
import json
import os
import re
from pathlib import Path, PurePosixPath
import shutil
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
TARGET = "arm-webos-linux-gnueabi"
MAX_SOURCE = 1024 ** 3
STATIC_COMPONENTS = {
    "Scrt1.o": "glibc", "crt1.o": "glibc", "crti.o": "glibc", "crtn.o": "glibc",
    "libc_nonshared.a": "glibc", "libpthread_nonshared.a": "glibc",
    "crtbegin.o": "gcc", "crtbeginS.o": "gcc", "crtbeginT.o": "gcc",
    "crtend.o": "gcc", "crtendS.o": "gcc", "libgcc.a": "gcc", "libgcc_eh.a": "gcc",
}
FIRMWARE = {"libc.so", "libc.so.6", "libm.so", "libm.so.6", "libdl.so", "libdl.so.2",
            "libpthread.so", "libpthread.so.0", "librt.so", "librt.so.1", "libgcc_s.so",
            "libgcc_s.so.1", "libresolv.so", "libresolv.so.2", "libutil.so", "libutil.so.1", "ld-linux.so.3"}


def sha(path: Path, algorithm="sha256"):
    digest = hashlib.new(algorithm)
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""): digest.update(chunk)
    return digest.hexdigest()


def regular(path: Path):
    if path.is_symlink() or not path.is_file(): raise ValueError("Expected a regular source/evidence file: " + path.name)
    return path


def load_policy():
    return json.loads((ROOT / "ci/native-runtime-sources.json").read_text())



def approved_sdk_hashes(policy):
    deps = json.loads((ROOT / "ci/dependencies.json").read_text())
    ndk = next(dep for dep in deps if dep["id"] == "webos-ndk")
    hashes = {name: asset["sha256"] for name, asset in ndk["assets"].items()}
    if ndk["version"] != policy.get("sdk_version") or hashes != policy.get("sdk_archives"):
        raise ValueError("Runtime source provenance must be reviewed for the changed SDK distribution")
    return set(hashes.values())


def source_inputs(directory: Path, policy: dict):
    files = {}
    for name, spec in policy["sources"].items():
        if Path(name).name != name: raise ValueError("Unsafe runtime source name")
        path = regular(directory / name)
        if not 0 < path.stat().st_size <= MAX_SOURCE or sha(path) != spec["sha256"]:
            raise ValueError("Runtime source checksum mismatch: " + name)
        if "sha512" in spec and sha(path, "sha512") != spec["sha512"]:
            raise ValueError("Runtime upstream checksum mismatch: " + name)
        files[name] = path
    return files


def fetch_sources(directory: Path):
    directory.mkdir(parents=True, exist_ok=True)
    policy = load_policy()
    for name, spec in policy["sources"].items():
        path = directory / name
        if path.exists(): continue
        with tempfile.NamedTemporaryFile(dir=directory, delete=False) as stream:
            temporary = Path(stream.name)
            try:
                with urllib.request.urlopen(spec["url"], timeout=60) as response:
                    size = 0
                    while chunk := response.read(1024 * 1024):
                        size += len(chunk)
                        if size > MAX_SOURCE: raise ValueError("Runtime source exceeds size limit")
                        stream.write(chunk)
                stream.flush()
                if sha(temporary) != spec["sha256"]: raise ValueError("Downloaded runtime source checksum mismatch")
                temporary.rename(path)
            finally:
                temporary.unlink(missing_ok=True)
    return source_inputs(directory, policy)


def sdk_archive_inputs(archive: Path, wanted: set[str]):
    """Hash requested original SDK members without extracting arbitrary paths."""
    observed = {}
    with tarfile.open(archive, "r:*") as source:
        for member in source:
            parts = PurePosixPath(member.name).parts
            if len(parts) < 2: continue
            relative = str(PurePosixPath(*parts[1:]))
            if relative not in wanted: continue
            if relative in observed or not member.isfile() or member.size > MAX_SOURCE:
                raise ValueError("SDK input is not a unique regular archive member: " + relative)
            content = source.extractfile(member)
            digest = hashlib.sha256()
            while chunk := content.read(1024 * 1024): digest.update(chunk)
            observed[relative] = digest.hexdigest()
    if set(observed) != wanted: raise ValueError("Linked runtime input absent from pinned SDK archive")
    return observed


def sdk_relative(path: str, sdk: Path):
    candidate = Path(os.path.normpath(path))
    root = sdk.resolve(strict=True)
    resolved = candidate.resolve(strict=False)
    try:
        relative = resolved.relative_to(root)
    except ValueError:
        if candidate.is_relative_to(sdk.absolute()):
            raise ValueError("SDK input escapes the supplied SDK")
        return None
    resolved.resolve(strict=True)
    return relative.as_posix()


def trace_inputs(trace: str, sdk: Path, aliases=()):
    paths = {}
    for line in trace.splitlines():
        value = line.strip()
        if not value or not value.startswith("/"): continue
        # GNU ld may print archive(member); the archive is the source unit.
        value = value.split("(", 1)[0]
        if re.fullmatch(r".+\.(?:o|a|so(?:\.[0-9]+)*)", Path(value).name) is None:
            continue
        relative = next((relative for root in (sdk, *aliases)
                         if (relative := sdk_relative(value, root)) is not None), None)
        if relative is None: continue
        name = Path(relative).name
        if name in STATIC_COMPONENTS:
            paths[relative] = {"component": STATIC_COMPONENTS[name], "kind": "incorporated"}
        elif name in FIRMWARE:
            paths[relative] = {"component": "firmware-runtime", "kind": "dynamic-not-distributed"}
        elif name.endswith((".a", ".o")):
            raise ValueError("Unmapped SDK object/archive input: " + name)
        elif ".so" in name:
            paths[relative] = {"component": "firmware-runtime", "kind": "dynamic-not-distributed"}
        if relative in paths:
            paths[relative]["sha256"] = sha(regular(Path(value).resolve(strict=True)))
    if not any(row["kind"] == "incorporated" for row in paths.values()):
        raise ValueError("No SDK runtime objects in final linker trace")
    return paths


def packaged_elfs(package: Path):
    spec = importlib.util.spec_from_file_location("runtime_native_package", ROOT / "scripts/lg-build.py")
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    members = native.ar_members(regular(package).read_bytes())
    files = native.tar_files(members["data.tar.gz"])
    result = {}
    for name, content in files.items():
        basename = PurePosixPath(name).name
        if basename in FIRMWARE: raise ValueError("Firmware runtime library is redistributed in IPK: " + basename)
        if content.startswith(b"\x7fELF"):
            if basename in result: raise ValueError("Ambiguous packaged ELF basename")
            result[basename] = hashlib.sha256(content).hexdigest()
    return result


def collect(package: Path, evidence: Path, sdk: Path, sdk_archive: Path, sources: Path,
            output: Path, *, policy=None, allowed_sdk_hashes=None, sdk_aliases=()):
    policy = load_policy() if policy is None else policy
    files = source_inputs(sources, policy)
    sdk_hash = sha(regular(sdk_archive))
    if allowed_sdk_hashes is None:
        allowed_sdk_hashes = approved_sdk_hashes(policy)
    if sdk_hash not in allowed_sdk_hashes: raise ValueError("SDK archive is not an approved pinned distribution")
    elfs = packaged_elfs(package)
    records, wanted, inputs, tools = {}, set(), {}, {}
    for binary, elf_hash in sorted(elfs.items()):
        base = evidence / binary
        record_path = regular(Path(str(base) + ".link.json"))
        record = json.loads(record_path.read_text())
        trace_path = regular(Path(str(base) + ".link.trace"))
        map_path = regular(Path(str(base) + ".link.map"))
        if record.get("elf_sha256") != elf_hash or record.get("trace_sha256") != sha(trace_path) or record.get("map_sha256") != sha(map_path):
            raise ValueError("Final packaged ELF/link evidence mismatch: " + binary)
        if record.get("archive_excluded") is not True: raise ValueError("Missing SDK archive exclusion evidence")
        compiler_paths = {"compiler_sha256": sdk / "bin" / (TARGET + "-gcc")}
        if "compiler_backend_sha256" in record:
            compiler_paths["compiler_backend_sha256"] = sdk / "bin" / (TARGET + "-gcc.br_real")
        for field, compiler in compiler_paths.items():
            relative = sdk_relative(str(compiler), sdk)
            if relative is None or record.get(field) != sha(regular(sdk / relative)):
                raise ValueError("Final link compiler differs from pinned SDK: " + binary)
            tools[relative] = sha(sdk / relative)
            wanted.add(relative)
        entries = trace_inputs(trace_path.read_text(), sdk, sdk_aliases)
        for relative, row in entries.items():
            wanted.add(relative)
            if row["sha256"] != sha(regular(sdk / relative)):
                raise ValueError("Linked SDK alias input differs from pinned SDK installation: " + relative)
            if relative in inputs and inputs[relative] != row:
                raise ValueError("Different linked SDK bytes share one runtime identity")
            inputs[relative] = row
        records[binary] = {"elf_sha256": elf_hash, "link_record_sha256": sha(record_path), "trace_sha256": sha(trace_path),
                           "map_sha256": sha(map_path), "runtime_inputs": sorted(entries)}
    if not records: raise ValueError("IPK contains no ELF inputs")
    if any(Path(path).name in elfs or row["sha256"] in elfs.values()
           for path, row in inputs.items() if row["kind"] == "dynamic-not-distributed"):
        raise ValueError("SDK shared library is copied into the final package")
    archived = sdk_archive_inputs(sdk_archive, wanted)
    for relative, expected in archived.items():
        actual = inputs[relative]["sha256"] if relative in inputs else tools[relative]
        if actual != expected: raise ValueError("Installed SDK input differs from pinned SDK archive: " + relative)
    output.mkdir(parents=True, exist_ok=False)
    for name, path in files.items(): shutil.copyfile(path, output / name)
    source_inventory = {name: {"sha256": sha(output / name), "bytes": (output / name).stat().st_size} for name in files}
    record = {"schema": 1, "scope": "incorporated-sdk-runtime-source-coverage",
              "package_sha256": sha(package), "sdk_archive_sha256": sdk_hash,
              "toolchain_revision": policy["toolchain_revision"], "buildroot_revision": policy["buildroot_revision"],
              "glibc_revision": policy["glibc_revision"], "gcc_version": policy["gcc_version"],
              "sources": source_inventory, "runtime_inputs": inputs, "toolchain_inputs": tools, "binaries": records,
              "sdk_runtime_source_coverage": "PASS", "complete_corresponding_source": False,
              "enclosing_source_closure": "REQUIRES_LOCKED_APPLICATION_AND_COMPANION_SOURCES"}
    (output / "runtime-source-coverage.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    return record


def verify(directory: Path, package: Path, *, expected_sdk_hash: str, policy=None):
    record = json.loads(regular(directory / "runtime-source-coverage.json").read_text())
    if policy is None and expected_sdk_hash not in approved_sdk_hashes(load_policy()):
        raise ValueError("Runtime coverage refers to an unapproved SDK archive")
    if record.get("schema") != 1 or record.get("package_sha256") != sha(regular(package)) or record.get("sdk_archive_sha256") != expected_sdk_hash:
        raise ValueError("Runtime source coverage identity mismatch")
    if record.get("sdk_runtime_source_coverage") != "PASS" or not record.get("binaries") or not record.get("runtime_inputs"):
        raise ValueError("Runtime source coverage is incomplete")
    policy = load_policy() if policy is None else policy
    files = source_inputs(directory, policy)
    for field in ("toolchain_revision", "buildroot_revision", "glibc_revision", "gcc_version"):
        if record.get(field) != policy[field]: raise ValueError("Runtime recipe identity mismatch")
    if set(record["sources"]) != set(files): raise ValueError("Runtime source inventory mismatch")
    for name, path in files.items():
        if record["sources"][name] != {"sha256": sha(path), "bytes": path.stat().st_size}:
            raise ValueError("Runtime source asset differs from coverage receipt")
    if {name: row["elf_sha256"] for name, row in record["binaries"].items()} != packaged_elfs(package):
        raise ValueError("Runtime source coverage targets different packaged ELF bytes")
    return record




def verify_complete_companion(directory: Path, package: Path, *, policy=None):
    """Independently verify the combined source payload without extraction."""
    explicit_policy = policy is not None
    policy = load_policy() if policy is None else policy
    closure_path = regular(directory / "source-closure.json")
    closure = json.loads(closure_path.read_text())
    manifest_path = regular(directory / "SOURCE-MANIFEST.json")
    source_archive = regular(directory / "companion-source.tar.gz")
    manifest = json.loads(manifest_path.read_text())
    if (closure.get("complete_corresponding_source") is not True
            or closure.get("sdk_runtime_source_coverage") != "PASS"
            or closure.get("locked_source_coverage") != "PASS"
            or closure.get("source_manifest_sha256") != sha(manifest_path)
            or closure.get("source_archive", {}).get("sha256") != sha(source_archive)):
        raise ValueError("Combined source closure identity or coverage mismatch")
    if (manifest.get("scope") != "companion-locked-and-incorporated-sdk-runtime-sources"
            or manifest.get("complete_corresponding_source") is not True
            or manifest.get("sdk_runtime_source_coverage") != "PASS"):
        raise ValueError("Combined source manifest does not describe complete companion sources")
    expected = set(policy["sources"]) | {"companion-locked-source.tar.gz", "companion-locked-source-manifest.json",
               "companion-locked-source-closure.json", "runtime-source-coverage.json", "README.txt"}
    entries = manifest.get("source_sets", {})
    if set(entries) != expected: raise ValueError("Combined source-set inventory mismatch")
    for name, spec in policy["sources"].items():
        if entries[name].get("sha256") != spec["sha256"]: raise ValueError("Combined source archive is not a pinned runtime source")
    found, retained, total = set(), {}, 0
    with tarfile.open(source_archive, "r:gz") as archive:
        for member in archive:
            name = member.name
            if name not in expected | {"SOURCE-MANIFEST.json"} or name in found or not member.isfile():
                raise ValueError("Unexpected, duplicate or non-regular combined source member")
            if member.size < 0 or member.size > MAX_SOURCE: raise ValueError("Combined source member exceeds size limit")
            total += member.size
            if total > MAX_SOURCE: raise ValueError("Combined source payload exceeds size limit")
            found.add(name)
            digest = hashlib.sha256()
            content = archive.extractfile(member)
            keep = name in {"runtime-source-coverage.json", "companion-locked-source-closure.json"}
            if keep and member.size > 4 * 1024 * 1024: raise ValueError("Combined source receipt exceeds size limit")
            data = bytearray()
            while chunk := content.read(1024 * 1024):
                digest.update(chunk)
                if keep: data.extend(chunk)
            if name == "SOURCE-MANIFEST.json":
                if digest.hexdigest() != sha(manifest_path): raise ValueError("Archived source manifest differs from published manifest")
            elif entries[name] != {"sha256": digest.hexdigest(), "bytes": member.size}:
                raise ValueError("Combined source member differs from its source manifest")
            if keep: retained[name] = json.loads(data)
    if found != expected | {"SOURCE-MANIFEST.json"}: raise ValueError("Combined source payload is missing members")
    precursor = retained["companion-locked-source-closure.json"]
    coverage = retained["runtime-source-coverage.json"]
    if (precursor.get("locked_source_coverage") != "PASS"
            or precursor.get("source_archive", {}).get("sha256") != entries["companion-locked-source.tar.gz"]["sha256"]
            or precursor.get("source_manifest_sha256") != entries["companion-locked-source-manifest.json"]["sha256"]
            or closure.get("precursor_closure_sha256") != entries["companion-locked-source-closure.json"]["sha256"]
            or closure.get("runtime_source_coverage_sha256") != entries["runtime-source-coverage.json"]["sha256"]):
        raise ValueError("Combined source sets do not match their retained original evidence")
    actual = packaged_elfs(package)
    for field in ("elf_sha256", "local_inputs_sha256", "local_source_archive_sha256", "source_receipt_sha256"):
        if not isinstance(closure.get(field), str) or not re.fullmatch(r"[0-9a-f]{64}", closure[field]):
            raise ValueError("Combined source closure is missing a helper source identity digest")
        if precursor.get(field) != closure.get(field): raise ValueError("Combined source closure changed the helper source identity")
    if not actual.get("ottplay-companion") or actual["ottplay-companion"] != closure.get("elf_sha256"):
        raise ValueError("Combined source closure describes a different companion ELF")
    if (coverage.get("sdk_runtime_source_coverage") != "PASS" or coverage.get("package_sha256") != sha(package)
            or {name: row["elf_sha256"] for name, row in coverage.get("binaries", {}).items()} != actual):
        raise ValueError("Combined runtime sources describe a different final package")
    for field in ("toolchain_revision", "buildroot_revision", "glibc_revision", "gcc_version"):
        if coverage.get(field) != policy[field]: raise ValueError("Combined runtime recipe differs from pinned source policy")
    if not explicit_policy:
        if coverage.get("sdk_archive_sha256") not in approved_sdk_hashes(policy):
            raise ValueError("Combined runtime sources refer to an unapproved SDK archive")
    if not coverage.get("runtime_inputs"):
        raise ValueError("Combined runtime sources lack incorporated SDK input evidence")
    for name in policy["sources"]:
        if coverage.get("sources", {}).get(name) != entries[name]: raise ValueError("Combined runtime source hash differs from coverage record")
    return closure


def complete_companion(supplement: Path, runtime_directory: Path, package: Path, output: Path, *, policy=None):
    """Combine actual locked helper sources with its verified SDK source closure.

    The original collector records remain inside the archive; no pending evidence
    is edited in place. The new closure identifies both constituent source sets.
    """
    closure_path = regular(supplement / "source-closure.json")
    closure = json.loads(closure_path.read_text())
    manifest = regular(supplement / "SOURCE-MANIFEST.json")
    archive = regular(supplement / "companion-source.tar.gz")
    if (closure.get("locked_source_coverage") != "PASS"
            or closure.get("source_archive", {}).get("sha256") != sha(archive)
            or closure.get("source_manifest_sha256") != sha(manifest)):
        raise ValueError("Locked companion source supplement is incomplete or changed")
    coverage_path = regular(runtime_directory / "runtime-source-coverage.json")
    raw_coverage = json.loads(coverage_path.read_text())
    coverage = verify(runtime_directory, package, expected_sdk_hash=raw_coverage.get("sdk_archive_sha256"), policy=policy)
    helper = coverage["binaries"].get("ottplay-companion")
    if not helper or helper.get("elf_sha256") != closure.get("elf_sha256"):
        raise ValueError("Runtime coverage does not describe the same packaged companion")
    payload = {"companion-locked-source.tar.gz": archive, "companion-locked-source-manifest.json": manifest,
               "companion-locked-source-closure.json": closure_path,
               "runtime-source-coverage.json": coverage_path}
    payload.update({name: runtime_directory / name for name in coverage["sources"]})
    combined_manifest = {"schema": 1, "package": "ottplay-companion",
        "scope": "companion-locked-and-incorporated-sdk-runtime-sources",
        "elf_sha256": closure["elf_sha256"], "local_inputs_sha256": closure["local_inputs_sha256"],
        "complete_corresponding_source": True, "sdk_runtime_source_coverage": "PASS",
        "source_sets": {name: {"sha256": sha(path), "bytes": path.stat().st_size} for name, path in payload.items()},
        "sdk_archive_sha256": coverage["sdk_archive_sha256"], "rebuild_validation": "NOT_RUN"}
    output.mkdir(parents=True, exist_ok=False)
    combined_path = output / "SOURCE-MANIFEST.json"
    combined_path.write_text(json.dumps(combined_manifest, indent=2, sort_keys=True) + "\n")
    payload["SOURCE-MANIFEST.json"] = combined_path
    readme = output / "README.txt"
    readme.write_text("This archive combines two verified source sets.\n"
                      "Extract companion-locked-source.tar.gz and follow its README for the helper build.\n"
                      "The original source manifest and pending SDK declaration are retained as precursor evidence.\n"
                      "The additional GCC/glibc source archives and complete native-toolchain/Buildroot recipe snapshots\n"
                      "cover actual SDK runtime objects listed in runtime-source-coverage.json.\n"
                      "Do not copy firmware shared libraries into the application.\n"
                      "A source coverage result does not claim a reproducible or device-tested rebuild.\n")
    payload["README.txt"] = readme
    combined_manifest["source_sets"]["README.txt"] = {"sha256": sha(readme), "bytes": readme.stat().st_size}
    combined_path.write_text(json.dumps(combined_manifest, indent=2, sort_keys=True) + "\n")
    target = output / "companion-source.tar.gz"
    total = 0
    with tarfile.open(target, "w:gz") as tar:
        for name, path in sorted(payload.items()):
            size = regular(path).stat().st_size
            total += size
            if size > MAX_SOURCE or total > MAX_SOURCE: raise ValueError("Combined source supplement exceeds size limit")
            info = tarfile.TarInfo(name)
            info.size = size
            info.mode = 0o644
            info.mtime = 0
            with path.open("rb") as stream: tar.addfile(info, stream)
    combined = {**closure,
        "source_archive": {"file": target.name, "sha256": sha(target), "bytes": target.stat().st_size},
        "source_manifest_sha256": sha(combined_path),
        "complete_corresponding_source": True, "sdk_runtime_source_coverage": "PASS",
        "runtime_source_coverage_sha256": sha(coverage_path), "precursor_closure_sha256": sha(closure_path),
        "rebuild_validation": "NOT_RUN"}
    (output / "source-closure.json").write_text(json.dumps(combined, indent=2, sort_keys=True) + "\n")
    verify_complete_companion(output, package, policy=policy)
    return combined


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("fetch", "collect", "verify", "complete-companion"))
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--package", type=Path)
    parser.add_argument("--companion-supplement", type=Path)
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--sdk", type=Path)
    parser.add_argument("--sdk-archive", type=Path)
    parser.add_argument("--sdk-alias", action="append", default=[], type=Path,
                        help="explicit original SDK root in retained linker evidence; each input is reverified")
    parser.add_argument("--sdk-sha256")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.operation == "fetch": fetch_sources(args.source_dir)
    elif args.operation == "complete-companion": complete_companion(args.companion_supplement, args.source_dir, args.package, args.output)
    elif args.operation == "verify": verify(args.source_dir, args.package, expected_sdk_hash=args.sdk_sha256)
    else: collect(args.package, args.evidence, args.sdk, args.sdk_archive, args.source_dir, args.output, sdk_aliases=args.sdk_alias)


if __name__ == "__main__": main()
