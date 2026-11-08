#!/usr/bin/env python3
"""Render native Unicode 17 tables from exact local, pinned project inputs.

No network, host Unicode database, locale, or third-party module is used.
Data license: licenses/Unicode-17.0.0-data.txt. The source generator is
scripts/generate-localization-unicode.py at the pinned FOSS commit below.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re

SOURCE = "e7f18d5786f5853e7e32f66644090ab0a82f8713"
HASHES = {
    "data": "b39e7a8866c25331d7f85509750b094bdd4eb041b89df532453c0a2623d47300",
    "normalization": "9d34cd57682dd78678e7788dabdc423a718f3a3f745ef14f1f6f88e2f655aeaa",
    "case_fold": "ff8d8fefbf123574205085d6714c36149eb946d717a0c585c27f0f4ef58c4183",
}
ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"


def checked(path, name):
    raw = Path(path).read_bytes()
    if hashlib.sha256(raw).hexdigest() != HASHES[name]:
        raise ValueError(name + " input differs from pinned source")
    return raw.decode("utf-8")


def values(source, name):
    match = re.search(r"export var " + name + r"\s*=\s*(.*?);", source, re.S)
    if not match:
        raise ValueError("missing data field: " + name)
    encoded = "".join(json.loads(part) for part in re.findall(r'"[^"\\]*"', match[1]))
    result, number, factor = [], 0, 1
    for char in encoded:
        part = ALPHABET.index(char)
        number += (part & 31) * factor
        if part < 32:
            result.append(number)
            number, factor = 0, 1
        else:
            factor *= 32
    if factor != 1:
        raise ValueError("incomplete varint")
    return result


def mappings(source, name, bits):
    data = values(source, name)
    at, point, previous, result = 0, 0, [0] * (1 << bits), []
    while at < len(data):
        header = data[at]
        at += 1
        point += header >> bits
        row = []
        for index in range((header & ((1 << bits) - 1)) + 1):
            offset = data[at]
            at += 1
            previous[index] += -(offset + 1) // 2 if offset & 1 else offset // 2
            row.append(previous[index])
        result.append((point, row))
    return result


def digest(value):
    return hashlib.sha256(json.dumps(value, separators=(",", ":")).encode()).hexdigest()


def scalar(point):
    return 0 < point <= 0x10FFFF and not 0xD800 <= point <= 0xDFFF


def render(data, normalization, case_fold):
    normalization = json.loads(normalization)
    if normalization["version"] != "17.0.0":
        raise ValueError("wrong Unicode version")
    canonical = mappings(data, "unicodeCanonicalData", 1)
    folding = mappings(data, "unicodeCaseFoldData", 2)
    compositions, point = [], 0
    for delta in values(data, "unicodeCompositionData"):
        point += delta
        compositions.append(point)
    ccc, end = [], 0
    raw = values(data, "unicodeCombiningData")
    if len(raw) % 3:
        raise ValueError("invalid combining data")
    for offset in range(0, len(raw), 3):
        start = end + raw[offset]
        end = start + raw[offset + 1]
        ccc.append((start, end, raw[offset + 2]))
    if digest(canonical) != normalization["canonicalDecompositionSha256"]:
        raise ValueError("canonical decompositions differ")
    if digest(compositions) != normalization["compositionSha256"]:
        raise ValueError("canonical compositions differ")
    if sum(end - start + 1 for start, end, _ in ccc) != normalization["combiningClassCount"]:
        raise ValueError("combining class count differs")
    reference = []
    for line in case_fold.splitlines():
        row = [part.strip() for part in line.split("#")[0].split(";")]
        if len(row) >= 3 and row[1] in ("C", "F"):
            reference.append((int(row[0], 16), [int(point, 16) for point in row[2].split()]))
    if folding != sorted(reference) or len(folding) != 1585:
        raise ValueError("full default C+F mapping differs")
    for table in (canonical, folding):
        if any(a[0] >= b[0] for a, b in zip(table, table[1:])):
            raise ValueError("mapping keys are not unique and sorted")
        if any(not scalar(point) for key, row in table for point in [key, *row]):
            raise ValueError("non-scalar mapping")
    decompositions = dict(canonical)
    def depth(point, active=frozenset()):
        if point in active:
            raise ValueError("cyclic decomposition")
        return 1 + max((depth(child, active | {point}) for child in decompositions.get(point, ())), default=0)
    maximum_depth = max(map(depth, decompositions))
    pairs = sorted((decompositions[point][0], decompositions[point][1], point) for point in compositions)
    if any(a[:2] == b[:2] for a, b in zip(pairs, pairs[1:])):
        raise ValueError("duplicate composition pair")
    if any(not 0 < rank <= 255 or start > end for start, end, rank in ccc):
        raise ValueError("invalid combining class range")
    if any(left[1] >= right[0] for left, right in zip(ccc, ccc[1:])):
        raise ValueError("overlapping combining class ranges")
    tables = [
        ("CANONICAL", "(u32, u32, u32)", [(key, *row, *([0] * (2 - len(row)))) for key, row in canonical]),
        ("CASE_FOLD", "(u32, u32, u32, u32)", [(key, *row, *([0] * (3 - len(row)))) for key, row in folding]),
        ("COMPOSITION", "(u32, u32, u32)", pairs),
        ("COMBINING", "(u32, u32, u8)", ccc),
    ]
    lines = ["// Generated by tools/generate-unicode-data.py; do not edit.",
             "// Unicode 17.0.0; Unicode-3.0 license: licenses/Unicode-17.0.0-data.txt.",
             "// Pinned source commit: " + SOURCE,
             "// Source data SHA256: " + HASHES["data"],
             "// Canonical decomposition maximum depth: " + str(maximum_depth), ""]
    for name, kind, records in tables:
        lines += ["#[rustfmt::skip]", f"pub(super) const {name}: &[{kind}] = &["]
        lines += ["    (" + ", ".join(f"0x{point:X}" for point in row) + ")," for row in records]
        lines += ["];", ""]
    return "\n".join(lines), {name:len(rows) for name, _, rows in tables}, maximum_depth


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", required=True)
    parser.add_argument("--normalization", required=True)
    parser.add_argument("--case-fold", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output, counts, depth_value = render(checked(args.data, "data"), checked(args.normalization, "normalization"), checked(args.case_fold, "case_fold"))
    Path(args.output).write_text(output, encoding="utf-8")
    print(json.dumps({"counts": counts, "maximum_decomposition_depth": depth_value, "output_sha256":hashlib.sha256(output.encode()).hexdigest()}, sort_keys=True))


if __name__ == "__main__":
    main()
