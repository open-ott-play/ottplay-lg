#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Generate local HA configuration only. No network, TV, or HA actions."""
import argparse
import json
import re
import sys
from pathlib import Path
from urllib.parse import parse_qs, urlsplit


class ConfigurationError(ValueError):
    pass


class Tag:
    def __init__(self, kind, value):
        self.kind, self.value = kind, value


def yaml_text(value, indent=0):
    """Small YAML emitter: JSON-quoted scalars plus the two explicit HA tags."""
    pad = " " * indent
    if isinstance(value, Tag):
        if value.kind not in ("input", "secret") or not re.fullmatch(r"[a-z][a-z0-9_]*", value.value):
            raise ConfigurationError("Invalid HA tag")
        return f"!{value.kind} {value.value}"
    if isinstance(value, dict) and value:
        rows = []
        for key, child in value.items():
            nested = isinstance(child, (dict, list)) and bool(child)
            rows.append(pad + json.dumps(key) + ":" + ("\n" + yaml_text(child, indent + 2) if nested else " " + yaml_text(child)))
        return "\n".join(rows)
    if isinstance(value, list) and value:
        return "\n".join(pad + "-" + ("\n" + yaml_text(child, indent + 2) if isinstance(child, (dict, list)) and child else " " + yaml_text(child)) for child in value)
    return json.dumps(value, ensure_ascii=False, allow_nan=False)


STARTUP_CONDITION = """{{ trigger is defined and trigger.from_state is defined
   and trigger.to_state is defined and trigger.from_state is not none
   and trigger.to_state is not none
   and trigger.from_state.state == 'off' and trigger.to_state.state == 'on'
   and 30 <= (minimum_off | int(0)) <= 600
   and 1 <= (settle | int(0)) <= 30
   and source_name is string and (source_name | length) > 0
   and (as_timestamp(trigger.to_state.last_changed, 0)
        - as_timestamp(trigger.from_state.last_changed, 0)) >= minimum_off | int(60)
   and is_state(tv_entity, 'on')
   and state_attr(tv_entity, 'source') != source_name
   and source_name in (state_attr(tv_entity, 'source_list') or [])
   and (not excluded_id or state_attr(tv_entity, 'app_id') != excluded_id) }}"""


def startup_blueprint():
    inp = lambda name: Tag("input", name)
    return {
        "blueprint": {
            "name": "OTTPlay once after LG TV startup",
            "description": "Launch an installed OTTPlay source once after a sustained off-to-on transition. No power-on, retries, or app chasing. Network outages may resemble power-off; disable this automation if that is unsuitable.",
            "domain": "automation",
            "homeassistant": {"min_version": "2024.10.0"},
            "input": {
                "tv": {"name": "LG television", "selector": {"entity": {"filter": [{"domain": "media_player", "integration": "webostv"}]}}},
                "source_name": {"name": "Installed OTTPlay source name", "description": "Exact case-sensitive name in this television's source_list; installation is separate.", "default": "OTTPlay", "selector": {"text": {}}},
                "settle_seconds": {"name": "Stable on interval", "default": 3, "selector": {"number": {"min": 1, "max": 30, "unit_of_measurement": "s", "mode": "box"}}},
                "minimum_off_seconds": {"name": "Minimum preceding off interval", "default": 60, "selector": {"number": {"min": 30, "max": 600, "unit_of_measurement": "s", "mode": "box"}}},
                "excluded_app_id": {"name": "Protected app ID", "description": "Optional app which must not be interrupted, for example a camera overlay.", "default": "", "selector": {"text": {}}},
            },
        },
        "variables": {"settle": inp("settle_seconds"), "minimum_off": inp("minimum_off_seconds"), "tv_entity": inp("tv"), "source_name": inp("source_name"), "excluded_id": inp("excluded_app_id")},
        "triggers": [{"trigger": "state", "entity_id": inp("tv"), "from": "off", "to": "on", "for": {"seconds": inp("settle_seconds")}}],
        "conditions": [{"condition": "template", "value_template": STARTUP_CONDITION}],
        "actions": [{"action": "media_player.select_source", "target": {"entity_id": inp("tv")}, "data": {"source": inp("source_name")}}],
        "mode": "single", "max_exceeded": "silent",
    }


def startup_allows(*, previous, current, off_seconds, minimum_off, live_state, source, sources, desired, app_id="", excluded=""):
    """Executable policy model; HA's trigger separately requires stable-on time."""
    return (previous == "off" and current == "on" and 30 <= minimum_off <= 600
            and isinstance(desired, str) and bool(desired) and off_seconds >= minimum_off
            and live_state == "on" and source != desired and desired in (sources or [])
            and (not excluded or app_id != excluded))


def validate_settings(value):
    fields = {"id", "name", "renewal_mode", "daily_time", "minimum_hours"}
    if not isinstance(value, dict) or set(value) - fields:
        raise ConfigurationError("Settings must contain only documented fields")
    ident = value.get("id")
    name = value.get("name", "OTTPlay LG TV")
    mode = value.get("renewal_mode", "external")
    daily = value.get("daily_time", "12:00:00")
    hours = value.get("minimum_hours", 168)
    if not isinstance(ident, str) or not re.fullmatch(r"[a-z][a-z0-9_]{0,31}", ident):
        raise ConfigurationError("id must be 1..32 lower-case letters, digits or underscores, starting with a letter")
    if not isinstance(name, str) or not 1 <= len(name) <= 80 or any(ord(c) < 32 for c in name):
        raise ConfigurationError("name must be 1..80 printable characters")
    if mode not in ("managed", "external", "manual"):
        raise ConfigurationError("renewal_mode must be managed, external or manual")
    if not isinstance(daily, str) or not re.fullmatch(r"(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]", daily):
        raise ConfigurationError("daily_time must be HH:MM:SS")
    if type(hours) is not int or not 168 <= hours <= 999:
        raise ConfigurationError("minimum_hours must be an integer from168 to999")
    return {"id": ident, "name": name, "renewal_mode": mode, "daily_time": daily, "minimum_hours": hours}


def renewal_package(settings):
    s = validate_settings(settings)
    if s["renewal_mode"] != "managed":
        return None
    p = "ott_lg_" + s["id"]
    enabled = {"condition": "state", "entity_id": f"input_boolean.{p}_renew_enabled", "state": "on"}
    body = lambda response: "{{ " + response + ".get('content', {}) if " + response + ".get('content', {}) is mapping else (" + response + ".get('content', '{}') | from_json({}) if " + response + ".get('content', {}) is string else {}) }}"
    return {
        "input_boolean": {p + "_renew_enabled": {"name": s["name"] + " Developer Mode renewal", "icon": "mdi:timer-refresh-outline"}},
        "rest_command": {p + "_" + action: {"url": Tag("secret", p + "_" + action + "_url"), "method": "get", "timeout": 20, "verify_ssl": True} for action in ("renew", "check")},
        "automation": [{
            "id": p + "_developer_mode", "alias": s["name"] + " verified Developer Mode renewal",
            "description": "Optional session maintenance; enable only when this TV has no existing renewer. Independent check must confirm sufficient remaining time. Does not contact or launch the TV.",
            "triggers": [{"trigger": "time", "at": s["daily_time"]}, {"trigger": "homeassistant", "event": "start"}],
            "conditions": [enabled],
            "actions": [
                {"delay": "00:01:00"}, enabled,
                {"variables": {"renewed": {}, "checked": {}}},
                {"action": f"rest_command.{p}_renew", "response_variable": "renewed", "continue_on_error": True},
                {"delay": "00:00:05"},
                {"action": f"rest_command.{p}_check", "response_variable": "checked", "continue_on_error": True},
                {"variables": {"renewed": "{{ renewed if renewed is mapping else {} }}", "checked": "{{ checked if checked is mapping else {} }}"}},
                {"variables": {"renew_body": body("renewed"), "check_body": body("checked")}},
                {"variables": {"remaining": "{{ check_body.get('errorMsg', '') | string | trim if check_body is mapping else '' }}"}},
                {"variables": {"remaining_hours": "{% if remaining | regex_match('^[0-9]+:[0-5][0-9]:[0-5][0-9]$') %}{% set parts = remaining.split(':') %}{{ (parts[0] | int) + (parts[1] | int) / 60 + (parts[2] | int) / 3600 }}{% else %}{{ -1 }}{% endif %}"}},
                {"variables": {"verified": "{{ renewed.get('status', 0) == 200 and checked.get('status', 0) == 200 and renew_body is mapping and check_body is mapping and renew_body.get('result') == 'success' and check_body.get('result') == 'success' and (remaining_hours | float(-1)) >= " + str(s["minimum_hours"]) + " }}"}},
                {"if": [{"condition": "template", "value_template": "{{ verified }}"}],
                 "then": [{"action": "persistent_notification.dismiss", "data": {"notification_id": p + "_developer_mode"}}],
                 "else": [{"action": "persistent_notification.create", "data": {"notification_id": p + "_developer_mode", "title": s["name"] + " Developer Mode needs attention", "message": "Renewal could not verify sufficient server-side time remaining. Check the private automation trace and the TV's Remain Session. Use EXTEND before expiry; refresh private LG URLs if necessary. Do not publish URLs, tokens or raw responses."}}]},
            ], "mode": "single", "max_exceeded": "silent",
        }],
    }


def verification_succeeds(renewed, checked, minimum_hours=168):
    """Fail-closed response policy model, including transport/content failures."""
    def response(value):
        if not isinstance(value, dict) or value.get("status") != 200:
            return None
        body = value.get("content")
        if isinstance(body, str):
            try:
                body = json.loads(body)
            except (ValueError, TypeError):
                return None
        return body if isinstance(body, dict) and body.get("result") == "success" else None
    renew, check = response(renewed), response(checked)
    if renew is None or check is None:
        return False
    match = re.fullmatch(r"([0-9]+):([0-5][0-9]):([0-5][0-9])", str(check.get("errorMsg", "")).strip())
    if not match:
        return False
    h, m, s = map(int, match.groups())
    return h + m / 60 + s / 3600 >= minimum_hours


def validate_url_pair(value):
    """Offline validation only; do not include secrets or URLs in error output."""
    if not isinstance(value, dict) or set(value) != {"renew", "check"}:
        raise ConfigurationError("Expected exactly renew and check fields")
    tokens = []
    for action, endpoint in (("renew", "ResetDevModeSession.dev"), ("check", "CheckDevModeSession.dev")):
        url = value[action]
        if not isinstance(url, str) or len(url) > 4096 or any(ord(c) < 33 or ord(c) == 127 for c in url):
            raise ConfigurationError("Invalid private URL pair")
        parsed = urlsplit(url)
        if (parsed.scheme != "https" or parsed.netloc != "developer.lge.com" or parsed.path != "/secure/" + endpoint or parsed.fragment):
            raise ConfigurationError("Private URLs must use the exact LG HTTPS endpoint")
        query = parse_qs(parsed.query, keep_blank_values=True)
        if set(query) != {"sessionToken"} or len(query["sessionToken"]) != 1 or not query["sessionToken"][0] or any(c.isspace() for c in query["sessionToken"][0]):
            raise ConfigurationError("Invalid private session token field")
        tokens.append(query["sessionToken"][0])
    if tokens[0] != tokens[1]:
        raise ConfigurationError("Private URL pair must identify one session")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    gen = sub.add_parser("generate", help="Generate a new directory; never install or execute it")
    gen.add_argument("settings", type=Path)
    gen.add_argument("output", type=Path)
    sub.add_parser("validate-private-urls", help="Validate renew/check JSON from stdin without printing it or making requests")
    args = parser.parse_args(argv)
    try:
        if args.command == "validate-private-urls":
            validate_url_pair(json.load(sys.stdin))
            print("Private URL pair format is valid; no request made")
            return 0
        settings = validate_settings(json.loads(args.settings.read_text()))
        package = renewal_package(settings)
        # Refuse existing output: no user configuration or secrets overwritten.
        args.output.mkdir(parents=True, exist_ok=False)
        (args.output / "ott_startup.yaml").write_text(yaml_text(startup_blueprint()) + "\n")
        if package is not None:
            p = "ott_lg_" + settings["id"]
            (args.output / (p + ".yaml")).write_text(yaml_text(package) + "\n")
            (args.output / "secrets.example.yaml").write_text("# Fill privately in HA secrets.yaml; placeholders are not credentials.\n" + "\n".join(f'{p}_{a}_url: "https://developer.lge.com/secure/{endpoint}?sessionToken=REPLACE_PRIVATELY"' for a, endpoint in (("renew", "ResetDevModeSession.dev"), ("check", "CheckDevModeSession.dev"))) + "\n")
        else:
            (args.output / "renewal-not-managed.txt").write_text("Renewal mode: " + settings["renewal_mode"] + ". No renewal commands or automation generated. Keep your existing renewer or use LG EXTEND manually.\n")
        print("Configuration generated locally; nothing installed or contacted")
        return 0
    except (ConfigurationError, ValueError, OSError) as exc:
        # Parsing failures may contain credentials: intentionally return only a closed message.
        print("Configuration rejected or output unavailable; check documented settings locally", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
