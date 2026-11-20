#!/usr/bin/env python3
"""Generate independent qualification inputs; never used by the player."""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess


def encoder(ffmpeg, index, size, rate, gop, duration):
    return [ffmpeg, "-nostdin", "-v", "error", "-y", "-filter_threads", "1",
            "-filter_complex_threads", "1", "-f", "lavfi", "-i",
            f"testsrc2=size={size}:rate={rate}:duration={duration},hue=h={index * 37},lutyuv=y=val*0.7+{index * 4}",
            "-f", "lavfi", "-i", f"sine=frequency={300 + index * 37}:sample_rate=48000:duration={duration}",
            "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-pix_fmt", "yuv420p",
            "-threads", "1", "-g", str(gop), "-c:a", "aac", "-ac", "2", "-shortest"]


def record(path, command, root):
    return {"name": path.relative_to(root).as_posix(), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "bytes": path.stat().st_size, "command": command}


def hls(directory, command, root):
    directory.mkdir()
    command = [*command, "-hls_time", "2", "-hls_list_size", "0", "-hls_playlist_type", "vod",
               "-hls_segment_filename", str(directory / "segment%03d.ts"), str(directory / "media.m3u8")]
    subprocess.run(command, check=True, timeout=90)
    return [record(path, command, root) for path in sorted(directory.iterdir())]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=pathlib.Path)
    parser.add_argument("--ffmpeg", default=shutil.which("ffmpeg"))
    parser.add_argument("--grid9-resolution", choices=("1280x720", "1920x1080"),
                        help="also generate nine independent HD live-transport fixtures")
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
        # The four rolling broadcasts exercise film, PAL and fractional/double-rate TV.
        rate, gop = {2: ("24000/1001", 24), 6: ("30000/1001", 30),
                     7: ("25", 25), 8: ("50", 50)}.get(index, ("10", 10))
        command = [*encoder(args.ffmpeg, index, "96x64", rate, gop, 24), "-movflags", "+faststart", str(path)]
        subprocess.run(command, check=True, timeout=45)
        records.append(record(path, command, args.destination))
        if index in (2, 6, 7, 8):
            remux = [args.ffmpeg, "-nostdin", "-v", "error", "-threads", "1", "-i", str(path), "-c", "copy"]
            hls_objects.extend(hls(args.destination / f"slot{index}-hls", remux, args.destination))
        if args.grid9_resolution:
            hd = encoder(args.ffmpeg, index, args.grid9_resolution, "25", 50, 30)
            hls_objects.extend(hls(args.destination / f"grid9-{index}-hls", hd, args.destination))
    manifest = {"schema": 1, "video_count": 9, "videos": records, "hls_objects": hls_objects}
    (args.destination / "MANIFEST.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
