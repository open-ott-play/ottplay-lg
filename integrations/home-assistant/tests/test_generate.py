# SPDX-License-Identifier: GPL-3.0-only
"""Offline policy/rendering checks. These do not execute Home Assistant or Jinja."""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / "generate.py"
spec = importlib.util.spec_from_file_location("ha_generator", MODULE)
g = importlib.util.module_from_spec(spec)
spec.loader.exec_module(g)


class StartupPolicy(unittest.TestCase):
    def allow(self, **overrides):
        args = dict(previous="off", current="on", off_seconds=60, minimum_off=60,
                    live_state="on", source="HDMI 1", sources=["HDMI 1", "OTTPlay"],
                    desired="OTTPlay", app_id="", excluded="")
        args.update(overrides)
        return g.startup_allows(**args)

    def test_only_real_sustained_off_to_on_can_launch(self):
        self.assertTrue(self.allow())
        for previous, current in [("unavailable", "on"), ("unknown", "on"), ("on", "on"), ("off", "off")]:
            self.assertFalse(self.allow(previous=previous, current=current))
        self.assertFalse(self.allow(off_seconds=59.999))
        self.assertFalse(self.allow(live_state="off"))
        self.assertFalse(self.allow(minimum_off=0))
        self.assertFalse(self.allow(minimum_off=601))
        self.assertFalse(self.allow(desired="", sources=[""]))

    def test_missing_source_case_mismatch_current_and_protection(self):
        self.assertFalse(self.allow(sources=None))
        self.assertFalse(self.allow(sources=["ottplay"]))
        self.assertFalse(self.allow(source="OTTPlay"))
        self.assertFalse(self.allow(app_id="com.example.camera", excluded="com.example.camera"))
        self.assertTrue(self.allow(app_id="com.example.other", excluded="com.example.camera"))

    def test_blueprint_is_one_shot_not_polling_or_startup_or_app_chasing(self):
        b = g.startup_blueprint()
        self.assertEqual(len(b["triggers"]), 1)
        t = b["triggers"][0]
        self.assertEqual((t["trigger"], t["from"], t["to"]), ("state", "off", "on"))
        self.assertEqual(t["for"]["seconds"].value, "settle_seconds")
        self.assertEqual(len(b["actions"]), 1)
        self.assertEqual(b["actions"][0]["action"], "media_player.select_source")
        self.assertEqual(b["mode"], "single")
        checked_in = (MODULE.parent / "ott_startup.yaml").read_text()
        self.assertEqual("\n".join(checked_in.splitlines()[2:]) + "\n", g.yaml_text(b) + "\n")
        condition = b["conditions"][0]["value_template"]
        for clause in ["trigger.from_state is defined", "trigger.to_state is defined",
                       "source_name in", "minimum_off", "excluded_id", "is_state(tv_entity, 'on')"]:
            self.assertIn(clause, condition)
        # After initial selection, app/source updates are not off-to-on transitions.
        self.assertFalse(self.allow(previous="on", current="on", source="Netflix"))


class RenewalPolicy(unittest.TestCase):
    def setUp(self):
        self.settings = dict(id="example_tv", name="Example TV", renewal_mode="managed")
        self.ok = {"status": 200, "content": {"result": "success", "errorMsg": "168:00:00"}}

    def test_independent_check_required_not_just_reset_success_or_http200(self):
        self.assertTrue(g.verification_succeeds(self.ok, self.ok))
        failures = [{}, None, {"status": 503}, {"status": 200, "content": "not-json"},
                    {"status": 200, "content": []}, {"status": 200, "content": {"result": "success", "errorMsg": "GNL"}},
                    {"status": 200, "content": {"result": "error", "errorMsg": "999:00:00"}}]
        for response in failures:
            with self.subTest(response=response):
                self.assertFalse(g.verification_succeeds(self.ok, response))
                if not (isinstance(response, dict) and isinstance(response.get("content"), dict) and response["content"].get("result") == "success"):
                    self.assertFalse(g.verification_succeeds(response, self.ok))
        for remaining in ["167:59:59", "168:60:00", "168:00:60", "-168:00:00", "168h", "168:0:00"]:
            check = copy.deepcopy(self.ok)
            check["content"]["errorMsg"] = remaining
            self.assertFalse(g.verification_succeeds(self.ok, check))
        self.assertTrue(g.verification_succeeds({"status": 200, "content": {"result": "success", "errorMsg": "GNL"}}, self.ok))
        check = {"status": 200, "content": json.dumps(self.ok["content"])}
        self.assertTrue(g.verification_succeeds(self.ok, check))

    def test_managed_opt_in_and_existing_renewers_get_no_requests(self):
        self.assertIsNone(g.renewal_package({"id": "tv"}))  # default external
        for mode in ["external", "manual"]:
            self.assertIsNone(g.renewal_package({"id": "tv", "renewal_mode": mode}))
        package = g.renewal_package(self.settings)
        auto = package["automation"][0]
        self.assertEqual(auto["mode"], "single")
        self.assertEqual(auto["conditions"][0]["state"], "on")
        # No initial:true: HA restores explicit consent; a first-ever helper starts off.
        self.assertNotIn("initial", next(iter(package["input_boolean"].values())))
        requests = [a for a in auto["actions"] if a.get("action", "").startswith("rest_command.")]
        self.assertEqual(len(requests), 2)
        self.assertEqual([a["response_variable"] for a in requests], ["renewed", "checked"])
        self.assertTrue(all(a["continue_on_error"] for a in requests))
        self.assertEqual(auto["actions"][1], auto["conditions"][0])  # opt-out rechecked after delay
        for command in package["rest_command"].values():
            self.assertIsInstance(command["url"], g.Tag)
            self.assertEqual(command["url"].kind, "secret")
            self.assertTrue(command["verify_ssl"])
            self.assertEqual(command["timeout"], 20)
        final = auto["actions"][-1]
        self.assertEqual(final["else"][0]["action"], "persistent_notification.create")
        self.assertEqual(final["then"][0]["action"], "persistent_notification.dismiss")
        text = g.yaml_text(package)
        self.assertNotIn("sessionToken=", text)
        self.assertNotIn("media_player.", text)
        def keys(value):
            if isinstance(value, dict):
                return set(value).union(*(keys(v) for v in value.values()))
            if isinstance(value, list):
                return set().union(*(keys(v) for v in value))
            return set()
        self.assertFalse({"repeat", "while", "until"} & keys(auto["actions"]))
        self.assertIn("renewed if renewed is mapping else {}", text)
        self.assertIn("checked if checked is mapping else {}", text)

    def test_settings_reject_unexpected_secrets_injection_bad_time_or_bounds(self):
        for change in [{"id": "Bad-ID"}, {"id": "x\nrest_command:"}, {"id": "x" * 33},
                       {"renewal_mode": "enabled"}, {"daily_time": "24:00:00"},
                       {"daily_time": "12:00:00\nfoo:"}, {"minimum_hours": 167},
                       {"minimum_hours": True}, {"minimum_hours": 1000},
                       {"name": "new\nkey"}, {"token": "synthetic-secret"}]:
            with self.subTest(change=change):
                with self.assertRaises(g.ConfigurationError):
                    g.validate_settings(dict(self.settings, **change))

    def test_distinct_tv_ids_have_disjoint_command_helper_notification_names(self):
        a = g.renewal_package(dict(self.settings, id="tv_a"))
        b = g.renewal_package(dict(self.settings, id="tv_b"))
        self.assertFalse(set(a["rest_command"]) & set(b["rest_command"]))
        self.assertFalse(set(a["input_boolean"]) & set(b["input_boolean"]))
        self.assertNotEqual(a["automation"][0]["id"], b["automation"][0]["id"])


class OfflineGeneration(unittest.TestCase):
    def test_url_validator_is_closed_exact_origin_pair_and_never_fetches(self):
        urls = {a: "https://developer.lge.com/secure/" + e + "?sessionToken=SYNTHETIC" for a, e in [("renew", "ResetDevModeSession.dev"), ("check", "CheckDevModeSession.dev")]}
        g.validate_url_pair(urls)
        for changed in [urls["renew"].replace("https:", "http:"),
                        urls["renew"].replace("developer", "develo\nper"),
                        urls["renew"].replace("developer.lge.com", "developer.lge.com.example"),
                        urls["renew"].replace("developer.lge.com", "user@developer.lge.com"),
                        urls["renew"] + "#ignored", urls["renew"] + "&sessionToken=OTHER",
                        urls["renew"].replace("SYNTHETIC", "OTHER"), urls["renew"].replace("sessionToken", "sessiontoken")]:
            with self.assertRaises(ValueError):
                g.validate_url_pair(dict(urls, renew=changed))

    def test_generation_is_secret_free_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            settings = root / "settings.json"
            settings.write_text(json.dumps({"id": "test_tv", "renewal_mode": "managed"}))
            out = root / "output"
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(g.main(["generate", str(settings), str(out)]), 0)
                before = {p.name: p.read_bytes() for p in out.iterdir()}
                self.assertEqual(g.main(["generate", str(settings), str(out)]), 2)
                self.assertEqual(before, {p.name: p.read_bytes() for p in out.iterdir()})
            self.assertIn("!input tv", (out / "ott_startup.yaml").read_text())
            package = (out / "ott_lg_test_tv.yaml").read_text()
            self.assertIn("!secret ott_lg_test_tv_renew_url", package)
            self.assertNotIn("SYNTHETIC", package)
            self.assertIn("REPLACE_PRIVATELY", (out / "secrets.example.yaml").read_text())
            self.assertEqual(package, g.yaml_text(g.renewal_package({"id": "test_tv", "renewal_mode": "managed"})) + "\n")


if __name__ == "__main__":
    unittest.main()
