#!/usr/bin/env python3
"""Generate small, independent qualification inputs; never used by the player."""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=pathlib.Path)
    parser.add_argument("--ffmpeg", default=shutil.which("ffmpeg"))
    args = parser.parse_args()
    if not args.ffmpeg:
        parser.error("fixture encoder ffmpeg is required")
    args.destination.mkdir(parents=True, exist_ok=True)
    if any(args.destination.iterdir()):
        parser.error("fixture destination must be empty")
    records = []
    hls_objects = []
    for index in range(9):
        path = args.destination / f"slot{index}.mp4"
        command = [args.ffmpeg, "-nostdin", "-v", "error", "-y", "-filter_threads", "1",
                   "-filter_complex_threads", "1", "-f", "lavfi", "-i",
                   f"testsrc2=size=96x64:rate=10:duration=24,hue=h={index * 37},lutyuv=y=val*0.7+{index * 4}",
                   "-f", "lavfi", "-i", f"sine=frequency={300 + index * 37}:sample_rate=48000:duration=24",
                   "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-pix_fmt", "yuv420p",
                   "-threads", "1", "-g", "10", "-c:a", "aac", "-ac", "2", "-movflags", "+faststart",
                   "-shortest", str(path)]
        subprocess.run(command, check=True, timeout=45)
        records.append({"name": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                        "bytes": path.stat().st_size, "command": command})
        if index in (2, 6, 7):
            hls = args.destination / f"slot{index}-hls"
            hls.mkdir()
            hls_command = [args.ffmpeg, "-nostdin", "-v", "error", "-threads", "1", "-i", str(path),
                           "-c", "copy", "-hls_time", "2", "-hls_list_size", "0", "-hls_playlist_type", "vod",
                           "-hls_segment_filename", str(hls / "segment%03d.ts"), str(hls / "media.m3u8")]
            subprocess.run(hls_command, check=True, timeout=45)
            for obj in sorted(hls.iterdir()):
                hls_objects.append({"name": obj.relative_to(args.destination).as_posix(),
                                    "sha256": hashlib.sha256(obj.read_bytes()).hexdigest(),
                                    "bytes": obj.stat().st_size, "command": hls_command})
    manifest = {"schema": 1, "video_count": 9, "videos": records, "hls_objects": hls_objects}
    (args.destination / "MANIFEST.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
