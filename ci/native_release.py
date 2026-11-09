#!/usr/bin/env python3
"""Create and verify immutable native webOS release plans and qualifications."""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime
from pathlib import Path
from typing import Any


PLAN_FIELDS = {
    "schema", "repository", "channel", "version", "sequence", "tag",
    "source_sha", "run_id", "run_attempt",
}
QUALIFICATION_FIELDS = {
    "schema", "repository", "channel", "version", "rc_tag",
    "source_sha", "ipk_sha256", "physical_evidence",
}
EVIDENCE_FIELDS = {"model", "firmware", "tested_at", "checks"}
SUPPORTED_CHECKS = {
    "install", "launch", "live-playback", "channel-switching",
    "audio-video-sync", "remote-control", "standby-resume",
}
REQUIRED_CHECKS = {
    "install", "launch", "live-playback", "channel-switching", "audio-video-sync",
}
VERSION_RE = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\Z", re.ASCII)
SHA_RE = re.compile(r"[0-9a-f]{40}\Z", re.ASCII)
SHA256_RE = re.compile(r"[0-9a-f]{64}\Z", re.ASCII)
REPOSITORY_RE = re.compile(r"[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}\Z", re.ASCII)
TAG_RE = re.compile(
    r"v((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*))-(beta|rc)\.([1-9][0-9]*)\Z",
    re.ASCII,
)
UTC_TIMESTAMP_RE = re.compile(
    r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{1,6})?Z\Z",
    re.ASCII,
)


class ReleaseError(ValueError):
    """Invalid release plan, qualification, or command-line input."""


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ReleaseError(message)


def _strict_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    _require(type(value) is dict, f"{label} must be a JSON object")
    actual = set(value)
    missing = expected - actual
    unknown = actual - expected
    _require(not missing, f"{label} missing fields: {', '.join(sorted(missing))}")
    _require(not unknown, f"{label} has unknown fields: {', '.join(sorted(unknown))}")
    return value


def _positive_int(value: Any, label: str) -> int:
    _require(type(value) is int and value > 0, f"{label} must be a positive integer")
    return value


def _repository(value: Any) -> str:
    _require(type(value) is str and REPOSITORY_RE.fullmatch(value) is not None,
             "repository must have the form owner/name")
    owner, name = value.split("/", 1)
    _require(owner not in {".", ".."} and name not in {".", ".."},
             "repository has an invalid owner or name")
    return value


def _version(value: Any) -> str:
    _require(type(value) is str and VERSION_RE.fullmatch(value) is not None,
             "version must be numeric X.Y.Z without leading zeroes")
    return value


def _source_sha(value: Any) -> str:
    _require(type(value) is str and SHA_RE.fullmatch(value) is not None,
             "source_sha must be a full lowercase 40-character commit SHA")
    return value


def _sha256(value: Any) -> str:
    _require(type(value) is str and SHA256_RE.fullmatch(value) is not None,
             "ipk_sha256 must be 64 lowercase hexadecimal characters")
    return value


def tag_for(version: str, channel: str, sequence: int) -> str:
    """Return the sole accepted beta/RC tag spelling for a plan."""
    _version(version)
    _require(type(channel) is str and channel in {"beta", "rc"}, "channel must be beta or rc")
    _positive_int(sequence, "sequence")
    return f"v{version}-{channel}.{sequence}"


def create_plan(
    *, repository: str, channel: str, version: str, sequence: int,
    source_sha: str, run_id: int, run_attempt: int,
) -> dict[str, Any]:
    """Create the deterministic, source- and run-bound beta/RC plan."""
    repository = _repository(repository)
    version = _version(version)
    _require(type(channel) is str and channel in {"beta", "rc"}, "channel must be beta or rc")
    sequence = _positive_int(sequence, "sequence")
    source_sha = _source_sha(source_sha)
    run_id = _positive_int(run_id, "run_id")
    run_attempt = _positive_int(run_attempt, "run_attempt")
    return {
        "schema": 1,
        "repository": repository,
        "channel": channel,
        "version": version,
        "sequence": sequence,
        "tag": tag_for(version, channel, sequence),
        "source_sha": source_sha,
        "run_id": run_id,
        "run_attempt": run_attempt,
    }


def validate_plan(plan: Any) -> dict[str, Any]:
    """Validate all fields, the closed schema, and the derived tag."""
    plan = _strict_keys(plan, PLAN_FIELDS, "plan")
    _require(type(plan["schema"]) is int and plan["schema"] == 1, "plan schema must be integer 1")
    expected = create_plan(
        repository=plan["repository"], channel=plan["channel"], version=plan["version"],
        sequence=plan["sequence"], source_sha=plan["source_sha"],
        run_id=plan["run_id"], run_attempt=plan["run_attempt"],
    )
    _require(plan["tag"] == expected["tag"], "plan tag does not match version, channel, and sequence")
    return expected


def verify_plan(plan: Any, expected: dict[str, Any]) -> dict[str, Any]:
    """Validate a plan and compare every supplied expected identity field."""
    valid = validate_plan(plan)
    unknown = set(expected) - PLAN_FIELDS
    _require(not unknown, f"expected identity has unknown fields: {', '.join(sorted(unknown))}")
    _require(bool(expected), "at least one expected identity field is required")
    for key, value in expected.items():
        _require(valid[key] == value, f"plan {key} does not match expected identity")
    return valid


def _text(value: Any, label: str) -> str:
    _require(type(value) is str and value == value.strip() and bool(value),
             f"{label} must be a nonempty trimmed string")
    _require(len(value) <= 160 and not any(ord(char) < 0x20 or ord(char) == 0x7F for char in value),
             f"{label} must be at most 160 characters and contain no control characters")
    return value


def _tested_at(value: Any) -> str:
    _require(type(value) is str and UTC_TIMESTAMP_RE.fullmatch(value) is not None,
             "tested_at must be an RFC 3339 UTC timestamp ending in Z")
    try:
        datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as exc:
        raise ReleaseError("tested_at is not a valid UTC timestamp") from exc
    return value


def validate_qualification(qualification: Any) -> dict[str, Any]:
    """Validate stable qualification identity and the required device evidence."""
    qualification = _strict_keys(qualification, QUALIFICATION_FIELDS, "qualification")
    _require(type(qualification["schema"]) is int and qualification["schema"] == 1,
             "qualification schema must be integer 1")
    repository = _repository(qualification["repository"])
    _require(qualification["channel"] == "stable", "qualification channel must be stable")
    version = _version(qualification["version"])
    rc_tag = qualification["rc_tag"]
    _require(type(rc_tag) is str, "rc_tag must be a string")
    match = TAG_RE.fullmatch(rc_tag)
    _require(match is not None and match.group(2) == "rc" and match.group(1) == version,
             "rc_tag must be v{version}-rc.N for the qualified version")
    source_sha = _source_sha(qualification["source_sha"])
    ipk_sha256 = _sha256(qualification["ipk_sha256"])
    evidence = qualification["physical_evidence"]
    _require(type(evidence) is list and len(evidence) > 0,
             "physical_evidence must contain at least one device record")
    normalized = []
    for index, item in enumerate(evidence):
        label = f"physical_evidence[{index}]"
        item = _strict_keys(item, EVIDENCE_FIELDS, label)
        model = _text(item["model"], f"{label}.model")
        firmware = _text(item["firmware"], f"{label}.firmware")
        tested_at = _tested_at(item["tested_at"])
        checks = item["checks"]
        _require(type(checks) is list and len(checks) > 0,
                 f"{label}.checks must be a nonempty list")
        _require(all(type(check) is str for check in checks), f"{label}.checks entries must be strings")
        _require(len(set(checks)) == len(checks), f"{label}.checks must not contain duplicates")
        unsupported = set(checks) - SUPPORTED_CHECKS
        _require(not unsupported, f"{label}.checks contains unsupported checks: {', '.join(sorted(unsupported))}")
        missing = REQUIRED_CHECKS - set(checks)
        _require(not missing, f"{label}.checks missing required checks: {', '.join(sorted(missing))}")
        normalized.append({"model": model, "firmware": firmware, "tested_at": tested_at,
                           "checks": sorted(checks)})
    return {
        "schema": 1,
        "repository": repository,
        "channel": "stable",
        "version": version,
        "rc_tag": rc_tag,
        "source_sha": source_sha,
        "ipk_sha256": ipk_sha256,
        "physical_evidence": normalized,
    }


def verify_qualification(qualification: Any, expected: dict[str, Any]) -> dict[str, Any]:
    """Validate qualification and match the exact promoted RC/package identity."""
    valid = validate_qualification(qualification)
    allowed = {"repository", "version", "rc_tag", "source_sha", "ipk_sha256"}
    unknown = set(expected) - allowed
    _require(not unknown, f"expected identity has unknown fields: {', '.join(sorted(unknown))}")
    _require(set(expected) == allowed, "expected identity must include repository, version, rc_tag, source_sha, and ipk_sha256")
    for key, value in expected.items():
        _require(valid[key] == value, f"qualification {key} does not match expected identity")
    return valid


def _object_no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ReleaseError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def read_json(path: str | Path) -> Any:
    try:
        with Path(path).open("r", encoding="utf-8") as stream:
            return json.load(stream, object_pairs_hook=_object_no_duplicates,
                             parse_constant=lambda value: (_ for _ in ()).throw(ReleaseError(f"invalid JSON constant: {value}")))
    except UnicodeDecodeError as exc:
        raise ReleaseError("JSON input must be UTF-8") from exc
    except json.JSONDecodeError as exc:
        raise ReleaseError(f"invalid JSON: {exc.msg}") from exc


def write_plan(plan: dict[str, Any], path: str | Path) -> None:
    destination = Path(path)
    payload = json.dumps(validate_plan(plan), indent=2, ensure_ascii=False, sort_keys=True) + "\n"
    try:
        with destination.open("x", encoding="utf-8", newline="\n") as stream:
            stream.write(payload)
    except FileExistsError as exc:
        raise ReleaseError(f"refusing to overwrite existing plan: {destination}") from exc


def _positive_cli_int(value: str) -> int:
    if not re.fullmatch(r"[1-9][0-9]*", value, re.ASCII):
        raise argparse.ArgumentTypeError("must be a positive decimal integer without leading zeroes")
    return int(value)


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)

    plan = commands.add_parser("plan", help="write a source/run-bound beta or RC plan")
    plan.add_argument("--repository", required=True)
    plan.add_argument("--channel", choices=("beta", "rc"), required=True)
    plan.add_argument("--version", required=True)
    plan.add_argument("--sequence", type=_positive_cli_int, required=True)
    plan.add_argument("--source-sha", required=True)
    plan.add_argument("--run-id", type=_positive_cli_int, required=True)
    plan.add_argument("--run-attempt", type=_positive_cli_int, required=True)
    plan.add_argument("--output", required=True)

    verify = commands.add_parser("verify-plan", help="verify a plan against exact workflow identity")
    verify.add_argument("--plan", required=True)
    verify.add_argument("--repository", required=True)
    verify.add_argument("--channel", choices=("beta", "rc"), required=True)
    verify.add_argument("--version", required=True)
    verify.add_argument("--sequence", type=_positive_cli_int, required=True)
    verify.add_argument("--tag", required=True)
    verify.add_argument("--source-sha", required=True)
    verify.add_argument("--run-id", type=_positive_cli_int, required=True)
    verify.add_argument("--run-attempt", type=_positive_cli_int, required=True)

    qualification = commands.add_parser("verify-qualification", help="verify stable qualification evidence")
    qualification.add_argument("--qualification", required=True)
    qualification.add_argument("--repository", required=True)
    qualification.add_argument("--version", required=True)
    qualification.add_argument("--rc-tag", required=True)
    qualification.add_argument("--source-sha", required=True)
    qualification.add_argument("--ipk-sha256", required=True)
    return root


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "plan":
            plan = create_plan(repository=args.repository, channel=args.channel, version=args.version,
                               sequence=args.sequence, source_sha=args.source_sha,
                               run_id=args.run_id, run_attempt=args.run_attempt)
            write_plan(plan, args.output)
            print(f"Created {plan['channel']} plan {plan['tag']} for {plan['source_sha']}")
        elif args.command == "verify-plan":
            expected = {
                "repository": args.repository, "channel": args.channel, "version": args.version,
                "sequence": args.sequence, "tag": args.tag, "source_sha": args.source_sha,
                "run_id": args.run_id, "run_attempt": args.run_attempt,
            }
            verify_plan(read_json(args.plan), expected)
            print("Plan identity verified")
        else:
            expected = {
                "repository": args.repository, "version": args.version, "rc_tag": args.rc_tag,
                "source_sha": args.source_sha, "ipk_sha256": args.ipk_sha256,
            }
            verify_qualification(read_json(args.qualification), expected)
            print("Stable qualification verified")
    except (OSError, ReleaseError) as exc:
        print(f"native_release: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
