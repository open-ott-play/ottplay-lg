#!/usr/bin/env python3
"""Small, synthetic media for the native external-source track picker.

Run with --out OUTSIDE this checkout. Generates 12 seconds of moving H.264,
English 220 Hz / Spanish 880 Hz AAC, two embedded SRT tracks, and a separate
PGS variant. There are no credentials, Plex identities, downloads or TV calls.
The recipe is deterministic; container hashes are receipts for this run, not
a promise of byte-identical output from different FFmpeg versions.
"""
import argparse
import array
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from make_fixtures import pgs_build, write_srt

ROOT = Path(__file__).resolve().parents[2]
DURATION = 12
TEXT_NAME = "external-tracks-text.mkv"
BITMAP_NAME = "external-tracks-bitmap.mkv"
TEXT_TRACKS = (
    (0, "video", "h264", None, None, 0, 0),
    (1, "audio", "aac", "eng", "English 220 Hz", 1, 0),
    (2, "subtitle", "subrip", "eng", "English SDH", 0, 0),
    (3, "audio", "aac", "spa", "Spanish 880 Hz", 0, 0),
    (4, "subtitle", "subrip", "rus", "Russian forced", 0, 1),
)
BITMAP_TRACKS = (
    (0, "video", "h264", None, None, 0, 0),
    (1, "audio", "aac", "eng", "English 220 Hz", 1, 0),
    (2, "subtitle", "hdmv_pgs_subtitle", "eng", "English PGS", 0, 0),
)


def run(args, *, binary=False):
    return subprocess.check_output(args, text=not binary, timeout=60)


def inventory(document):
    """Derive type-relative ordinals from actual container order, never indexes."""
    rows, counts, indexes = [], {}, set()
    for stream in document.get("streams", []):
        index = stream["index"]
        if index in indexes:
            raise ValueError("duplicate container stream index")
        indexes.add(index)
        kind = stream["codec_type"]
        ordinal = counts.get(kind, 0)
        counts[kind] = ordinal + 1
        tags, flags = stream.get("tags", {}), stream.get("disposition", {})
        rows.append({
            "stream_index": index, "kind": kind, "ordinal": ordinal,
            "codec": stream["codec_name"], "language": tags.get("language"),
            "title": tags.get("title"), "default": bool(flags.get("default", 0)),
            "forced": bool(flags.get("forced", 0)),
            "hearing_impaired": bool(flags.get("hearing_impaired", 0)),
            "channels": stream.get("channels"),
            "sample_rate": int(stream.get("sample_rate", 0)),
        })
    return rows


def validate_inventory(document, expected):
    rows = inventory(document)
    if len(rows) != len(expected):
        raise ValueError("wrong number of container streams")
    for row, spec in zip(rows, expected):
        index, kind, codec, lang, title, default, forced = spec
        want = dict(stream_index=index, kind=kind, codec=codec, language=lang,
                    title=title, default=bool(default), forced=bool(forced))
        for key, value in want.items():
            # No imposed video tag/default policy: only the playable track rows
            # are selectable, and FFmpeg may mark its sole video default.
            if kind == "video" and key in ("language", "title", "default", "forced"):
                continue
            if row[key] != value:
                raise ValueError(f"stream {index}: {key}={row[key]!r}, expected {value!r}")
        if kind == "audio" and (row["channels"], row["sample_rate"]) != (1, 48000):
            raise ValueError(f"stream {index}: audio must be mono 48000 Hz")
    return rows


def measured_tone(samples, sample_rate=8000):
    """Positive zero crossings of decoded PCM; a mislabeled/swapped tone fails."""
    if len(samples) < sample_rate // 2 or max(map(abs, samples), default=0) < .01:
        raise ValueError("missing or silent decoded audio")
    crossings = sum(a <= 0 < b for a, b in zip(samples, samples[1:]))
    return crossings * sample_rate / (len(samples) - 1)


def validate_bitmap_frames(document):
    frames = document.get("frames", [])
    times = [round(float(f["pts_time"]), 3) for f in frames]
    if (len(times) != 2 or abs(times[0]) > .1 or abs(times[1] - 8) > .1
            or [f.get("num_rects") for f in frames] != [1, 0]):
        raise ValueError("PGS must decode one visible rectangle at 0s and clear it at 8s")
    return times


def verify(path, bitmap=False):
    doc = json.loads(run(["ffprobe", "-v", "error", "-show_streams", "-show_format",
                          "-of", "json", str(path)]))
    rows = validate_inventory(doc, BITMAP_TRACKS if bitmap else TEXT_TRACKS)
    duration = float(doc["format"]["duration"])
    if not DURATION - .1 <= duration <= DURATION + .2:
        raise ValueError(f"unexpected duration {duration}")
    tones = []
    for ordinal, wanted in enumerate((220,) if bitmap else (220, 880)):
        raw = run(["ffmpeg", "-v", "error", "-ss", "1", "-i", str(path),
                   "-map", f"0:a:{ordinal}", "-t", "1", "-ac", "1", "-ar", "8000",
                   "-f", "f32le", "-"], binary=True)
        samples = array.array("f")
        samples.frombytes(raw)
        if sys.byteorder != "little":
            samples.byteswap()
        hz = measured_tone(samples)
        if abs(hz - wanted) > 2:
            raise ValueError(f"audio ordinal {ordinal}: decoded {hz:.2f} Hz, expected {wanted}")
        tones.append(round(hz, 2))
    captions = []
    if not bitmap:
        if not rows[2]["hearing_impaired"]:
            raise ValueError("English text track lost its hearing-impaired flag")
        for ordinal, label in enumerate(("English", "Русский")):
            text = run(["ffmpeg", "-v", "error", "-i", str(path), "-map",
                        f"0:s:{ordinal}", "-c:s", "srt", "-f", "srt", "-"])
            # Read the actual muxed text, and pin independent expected cue count
            # and coverage rather than trusting the generator's return value.
            if text.count(" --> ") != 3 or any(
                    f"S{ordinal} {label} @ {second}s" not in text for second in (0, 4, 8)):
                raise ValueError(f"subtitle ordinal {ordinal}: missing or wrong cue text")
            if "00:00:12," not in text:
                raise ValueError(f"subtitle ordinal {ordinal}: no cue covering the end")
            captions.append({"ordinal": ordinal, "cues": 3, "last_start_s": 8})
    else:
        frames = json.loads(run(["ffprobe", "-v", "error", "-select_streams", "s",
                                "-show_frames", "-of", "json", str(path)]))
        times = validate_bitmap_frames(frames)
        captions.append({"ordinal": 0, "display_and_clear_s": times,
                         "decoded_rectangles": [1, 0],
                         "note": "host decode; native bitmap rendering needs device acceptance"})
    return {"file": path.name, "bytes": path.stat().st_size,
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "duration_s": duration, "tracks": rows,
            "decoded_tones_hz": tones, "captions": captions}


def build(output):
    output = output.expanduser().resolve()
    if output == ROOT or ROOT in output.parents:
        raise ValueError("generated media must be outside the checkout")
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".external-tracks-", dir=output) as scratch:
        work = Path(scratch)
        en = write_srt(work / "english.srt", 0, "English", DURATION,
                       step=4, hold=4, cover_end=True)
        ru = write_srt(work / "russian.srt", 1, "Русский", DURATION,
                       step=4, hold=4, cover_end=True)
        target = output / TEXT_NAME
        run(["ffmpeg", "-v", "error", "-nostdin", "-y",
             "-f", "lavfi", "-i", f"testsrc2=size=320x180:rate=24:duration={DURATION}",
             "-f", "lavfi", "-i", f"sine=frequency=220:sample_rate=48000:duration={DURATION}",
             "-f", "lavfi", "-i", f"sine=frequency=880:sample_rate=48000:duration={DURATION}",
             "-i", str(en), "-i", str(ru),
             "-map", "0:v", "-map", "1:a", "-map", "3:s", "-map", "2:a", "-map", "4:s",
             "-c:v", "libx264", "-preset", "ultrafast", "-crf", "30", "-threads:v", "1",
             "-pix_fmt", "yuv420p", "-g", "24", "-c:a", "aac", "-b:a", "48k", "-c:s", "srt",
             "-metadata:s:a:0", "language=eng", "-metadata:s:a:0", "title=English 220 Hz",
             "-metadata:s:a:1", "language=spa", "-metadata:s:a:1", "title=Spanish 880 Hz",
             "-metadata:s:s:0", "language=eng", "-metadata:s:s:0", "title=English SDH",
             "-metadata:s:s:1", "language=rus", "-metadata:s:s:1", "title=Russian forced",
             "-disposition:a:0", "default", "-disposition:a:1", "0",
             "-disposition:s:0", "hearing_impaired", "-disposition:s:1", "forced",
             "-t", str(DURATION), str(target)])
        pgs = work / "english.sup"
        # Reuse the suite's synthetic PGS writer: no third-party film or image.
        pgs_build(pgs, DURATION, canvas=(640, 360))
        run(["ffmpeg", "-v", "error", "-nostdin", "-y", "-i", str(target), "-i", str(pgs),
             "-map", "0:v:0", "-map", "0:a:0", "-map", "1:0", "-c", "copy",
             "-metadata:s:s:0", "language=eng", "-metadata:s:s:0", "title=English PGS",
             "-disposition:s:0", "0", "-t", str(DURATION), str(output / BITMAP_NAME)])
    receipt = {"schema": 1, "source": "synthetic lavfi + authored captions + suite PGS writer",
               "ffmpeg": run(["ffmpeg", "-version"]).splitlines()[0],
               "files": [verify(output / TEXT_NAME), verify(output / BITMAP_NAME, bitmap=True)]}
    (output / "external-tracks-receipt.json").write_text(
        json.dumps(receipt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    receipt = build(args.out)
    for item in receipt["files"]:
        print(f"{item['file']}: {len(item['tracks'])} streams, "
              f"{item['bytes']} bytes, tones {item['decoded_tones_hz']} Hz")
    print(f"Receipt: {args.out.expanduser().resolve() / 'external-tracks-receipt.json'}")


if __name__ == "__main__":
    main()
