#!/usr/bin/env python3
"""Compile the shared-core ARM leaf and link its QuickJS C library; never execute it."""
from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
loader = importlib.util.spec_from_file_location("lg_build", ROOT / "scripts/lg-build.py")
builder = importlib.util.module_from_spec(loader)
loader.loader.exec_module(builder)

PROBE = r'''#include "quickjs.h"
_Static_assert(sizeof(JSValue) == 8, "ARM32 JSValue ABI");
_Static_assert(sizeof(JSMallocFunctions) == 20, "ARM32 allocator ABI");
_Static_assert(sizeof(JSMemoryUsage) == 208, "ARM32 memory usage ABI");
int main(void) {
    JSRuntime *runtime = JS_NewRuntime();
    if (!runtime) return 1;
    JSContext *context = JS_NewContext(runtime);
    if (!context) { JS_FreeRuntime(runtime); return 2; }
    JSValue value = JS_Eval(context, "1+1", 3, "link-probe", JS_EVAL_TYPE_GLOBAL);
    int result = JS_IsException(value) ? 3 : 0;
    JS_FreeValue(context, value);
    JS_FreeContext(context);
    JS_FreeRuntime(runtime);
    return result;
}
'''


def run(argv, env, cwd=ROOT):
    result = subprocess.run([str(arg) for arg in argv], cwd=cwd, env=env,
                            capture_output=True, text=True)
    if result.returncode:
        raise ValueError("Command failed: " + str(argv[0]) + "\n" + result.stdout + result.stderr)
    return result.stdout


def arm_attributes(report, *, archive=False):
    if ("Tag_CPU_arch: v7" not in report or "Tag_FP_arch: VFPv3" not in report
            or (archive and "Tag_Advanced_SIMD_arch" in report) or "Tag_ABI_VFP_args:" in report):
        raise ValueError("QuickJS must use ARMv7/VFPv3 and the soft-float calling convention")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--libclang", type=Path)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--check-application", action="store_true",
                        help="also type-check the production application for the television target")
    args = parser.parse_args(argv)
    args.sdk = args.sdk.expanduser().resolve(strict=True)
    args.target_dir = args.target_dir.expanduser().resolve()
    args.work_dir = args.target_dir
    args.rust_opt_level = "s"
    args.target_dir.mkdir(parents=True, exist_ok=True)
    libclang = builder.find_libclang(args.libclang)
    libclang_digest = builder.sha256(libclang)
    compiler = args.sdk / "bin/arm-webos-linux-gnueabi-gcc"
    readelf = compiler.with_name("arm-webos-linux-gnueabi-readelf")
    if not compiler.is_file() or not readelf.is_file():
        raise ValueError("The verified SDK compiler and readelf are required")
    cargo = shutil.which("cargo") or str(Path.home() / ".cargo/bin/cargo")

    # Reuse the standalone builder's clean environment and cache isolation. The
    # Makefile remains the sole definition of compiler, target and ABI flags.
    with builder.cargo_environment(args) as env:
        env.update(LIBCLANG_PATH=str(libclang), PYTHONDONTWRITEBYTECODE="1")
        configuration = run(["make", "-s", "print-bench-config", "RELEASE=1", "FLAVOR=stable",
                             "WEBOS_SDK=" + str(args.sdk)], env)
        config = dict(line.split("=", 1) for line in configuration.splitlines() if "=" in line)
        for group in ("RUST_ENV", "RUST_C_ENV"):
            for assignment in shlex.split(config[group]):
                key, value = assignment.split("=", 1)
                env[key] = value
        target = config["RUST_TARGET"]
        suffix = target.replace("-", "_")
        command = [cargo, "+" + config["RUST_TOOLCHAIN"], "build", "--locked", "-p", "plx_shared_core",
                   "--target", target, "--target-dir", args.target_dir, "--jobs", "2",
                   "--message-format=json-render-diagnostics"]
        if args.offline:
            command.append("--offline")
        print("Compiling shared-core and QuickJS for " + target, flush=True)
        output = run(command, env, ROOT / "rust-modules")
        (args.target_dir / "shared-core-arm-cargo.jsonl").write_text(output)
        rows = [json.loads(line) for line in output.splitlines() if line.strip()]
        outputs = {Path(row["out_dir"]).resolve() for row in rows
                   if row.get("reason") == "build-script-executed"
                   and row["package_id"].rsplit("#", 1)[-1].split("@", 1)[0] == "rquickjs-sys"}
        outputs = {path for path in outputs if path.is_relative_to(args.target_dir / target)}
        if len(outputs) != 1 or not any(row.get("reason") == "build-finished" and row.get("success")
                                       for row in rows):
            raise ValueError("Expected one successful target QuickJS build")
        out = outputs.pop()
        archive = out / "libquickjs.a"
        header = out / "quickjs.h"
        bindings = out / "bindings.rs"
        for artifact in (archive, header, bindings):
            if not artifact.is_file():
                raise ValueError("Missing compiled QuickJS artifact: " + artifact.name)
        attrs = run([readelf, "-A", archive], env)
        arm_attributes(attrs, archive=True)
        probe = args.target_dir / "shared-core-arm-probe.c"
        binary = args.target_dir / "shared-core-arm-probe"
        probe.write_text(PROBE)
        run([env["CC_" + suffix], *shlex.split(env["CFLAGS_" + suffix]),
             "-std=c11", "-D_GNU_SOURCE", "-O2", "-I" + str(out), probe, archive,
             "-lm", "-lpthread", "-ldl", "-lrt", "-o", binary], env)
        report = run([readelf, "-h", "-A", "-V", "-d", binary], env)
        # SDK startup objects advertise NEON even when the new archive does not.
        # Check that restriction on our archive, and the calling ABI on both.
        arm_attributes(report)
        if "soft-float ABI" not in report or not re.search(r"Machine:\s+ARM\s*$", report, re.MULTILINE):
            raise ValueError("Linked probe is not a soft-float ARM ELF")
        versions = sorted(set(re.findall(r"\bGLIBC_([0-9]+(?:\.[0-9]+)+)\b", report)))
        # The pinned webOS SDK and existing companion use glibc 2.12 as the ceiling.
        if not versions or any(tuple(map(int, value.split("."))) > (2, 12) for value in versions):
            raise ValueError("QuickJS needs a newer libc than the supported webOS SDK")
        if args.check_application:
            # Check the actual production crate with ARM32 C types before merge.
            # A shared-core-only ABI probe cannot catch application FFI type drift.
            app_env = dict(env, PLX_RELEASE="1")
            app_command = [cargo, "+" + config["RUST_TOOLCHAIN"], "check", "--locked",
                           "--release", "--lib", "--target", target,
                           "--target-dir", args.target_dir, "-p", "plxnative-modules",
                           "--no-default-features", "--jobs", "2",
                           "--message-format=json-render-diagnostics"]
            if args.offline:
                app_command.append("--offline")
            print("Checking production application for " + target, flush=True)
            app_output = run(app_command, app_env, ROOT / "rust-modules")
            (args.target_dir / "application-arm-check.jsonl").write_text(app_output)
        if builder.sha256(libclang) != libclang_digest:
            raise ValueError("libclang changed during binding generation")
        (args.target_dir / "shared-core-arm-readelf.txt").write_text(attrs + report)
        receipt = {"schema": 1, "target": target, "rust_toolchain": config["RUST_TOOLCHAIN"],
                   "makefile_sha256": builder.sha256(ROOT / "Makefile"),
                   "cargo_lock_sha256": builder.sha256(ROOT / "rust-modules/Cargo.lock"),
                   "core_manifest_sha256": builder.sha256(ROOT / "vendor/ottplay-core.manifest.json"),
                   "compiler_sha256": builder.sha256(compiler.resolve()),
                   "compiler_backend_sha256": builder.sha256(compiler.with_name(compiler.name + ".br_real")),
                   "libclang_sha256": libclang_digest, "header_sha256": builder.sha256(header),
                   "bindings_sha256": builder.sha256(bindings), "archive_sha256": builder.sha256(archive),
                   "probe_sha256": builder.sha256(binary), "glibc_versions": versions,
                   "device_executed": False,
                   "application_checked": args.check_application,
                   "scope": "shared-core-arm-compile-and-c-link"}
        if args.check_application:
            receipt["application_check_sha256"] = builder.sha256(args.target_dir / "application-arm-check.jsonl")
        builder.write_json(args.target_dir / "shared-core-arm-evidence.json", receipt)
        print("PASS shared-core ARM compile, generated bindings, C ABI and legacy-libc link (not executed)")
        if args.check_application:
            print("PASS production application ARM type-check (not linked or executed)")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError) as error:
        print("shared-core ARM check failed: " + str(error), file=sys.stderr)
        sys.exit(1)
