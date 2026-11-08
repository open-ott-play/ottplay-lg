#!/usr/bin/env python3
"""Exercise the real Cargo recipes with a recorder, without an SDK or compiler."""
import json
from pathlib import Path
import shlex
import unittest

import test_arm_profile as profiles


class RustCEnvironment(unittest.TestCase):
    def setUp(self):
        # Reuse the isolated native Make fixture: source paths are read-only,
        # while all stamp, archive and generated-header writes stay temporary.
        self.fixture = profiles.ControlDiscoveryStamp()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.tree = self.fixture.tree
        self.sdk = self.tree / "fixture-sdk"
        self.record = self.tree / "compiler-environment.json"
        self.cargo = Path(self.fixture.env["HOME"]) / ".cargo/bin/cargo"
        self.fixture.env.update(WEBOS_SDK=str(self.sdk), CC="host-compiler", AR="host-archiver")
        for key in list(self.fixture.env):
            if key.startswith(("CC_", "AR_", "CFLAGS_", "BINDGEN_EXTRA_CLANG_ARGS")):
                self.fixture.env.pop(key)
        self.recorder = (
            "\nimport json, os, pathlib\n"
            f"pathlib.Path({str(self.record)!r}).write_text(json.dumps(dict(os.environ)))\n"
        )

    def test_arm_archive_recipe_passes_sdk_tools_and_target_flags_to_cargo(self):
        self.cargo.write_text(self.cargo.read_text() + self.recorder)
        self.fixture.env.update(CC_armv7_unknown_linux_gnueabi="wrong-target-compiler",
                                AR_armv7_unknown_linux_gnueabi="wrong-target-archiver",
                                CFLAGS_armv7_unknown_linux_gnueabi="-Werror -Iwrong")
        self.fixture.run_make(None)
        env = json.loads(self.record.read_text())
        # The fixture overrides Make's native CC with true; the real recipe must
        # pass that selected compiler rather than discover a host/cross default.
        self.assertEqual(env["CC_armv7_unknown_linux_gnueabi"], "true")
        self.assertEqual(env["AR_armv7_unknown_linux_gnueabi"],
                         str(self.sdk / "bin/arm-webos-linux-gnueabi-ar"))
        flags = shlex.split(env["CFLAGS_armv7_unknown_linux_gnueabi"])
        for value in ("--sysroot=" + str(self.sdk / "arm-webos-linux-gnueabi/sysroot"),
                      "-mcpu=cortex-a9", "-mfpu=vfpv3", "-mfloat-abi=softfp",
                      "-fno-omit-frame-pointer", "-funwind-tables"):
            self.assertIn(value, flags)
        self.assertTrue(any(flag.startswith("-ffile-prefix-map=") for flag in flags))
        self.assertNotIn("-Werror", flags)
        self.assertFalse(any(flag.startswith("-I") for flag in flags))
        binding_flags = shlex.split(env["BINDGEN_EXTRA_CLANG_ARGS_armv7_unknown_linux_gnueabi"])
        self.assertEqual(binding_flags, flags[:4], "Rust bindings must use the compiled C target ABI")

    def test_host_check_does_not_receive_arm_compiler_assignments(self):
        self.cargo.write_text("#!/usr/bin/env python3\n" + self.recorder)
        self.fixture.run_make(None, "check-nodef")
        env = json.loads(self.record.read_text())
        for key in ("CC_armv7_unknown_linux_gnueabi", "AR_armv7_unknown_linux_gnueabi",
                    "CFLAGS_armv7_unknown_linux_gnueabi",
                    "BINDGEN_EXTRA_CLANG_ARGS_armv7_unknown_linux_gnueabi"):
            self.assertNotIn(key, env)

    def test_resolved_build_receipt_names_the_same_target_configuration(self):
        config = profiles.resolved("WEBOS_SDK=" + str(self.sdk))
        values = dict(part.split("=", 1) for part in shlex.split(config["RUST_C_ENV"]))
        self.assertEqual(values["CC_armv7_unknown_linux_gnueabi"], str(profiles.ROOT / "ci/arm-cc.py"))
        self.assertEqual(values["AR_armv7_unknown_linux_gnueabi"],
                         str(self.sdk / "bin/arm-webos-linux-gnueabi-ar"))


if __name__ == "__main__":
    unittest.main()
