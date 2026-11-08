# Local artwork, descriptions and subtitles

The native player reads optional assets from the same authorized folder as a Local video. Video files remain in their original location. This feature does not import videos, write a metadata file, grant access to another folder or follow symbolic links.

## Subtitles beside a video

For `Films/Example.mkv`, recognized adjacent names include `Films/Example.srt`, `Films/Example.en.vtt`, `Films/Example.fr.ass` and `Films/Example.ssa`. Matching preserves the exact basename and parent folder. A language suffix is filename syntax, not a verified language tag. Extra labels such as `forced`, `SDH` and `CC` are not inferred.

Choose a discovered track in the existing subtitle menu. Off disables it; selecting the same track again retries a failed read. Embedded and adjacent tracks share the existing subtitle settings and remote-control inventory. Text subtitles use the existing font, color, outline, background and timing controls. Authored ASS/SSA retains the existing format-specific style restrictions.

Discovery never automatically selects a subtitle. Known rows remain available if discovery is incomplete, while remote enumeration refuses to describe a partial inventory as complete. Embedded and adjacent tracks share the limits of 64 tracks per kind and 128 total. Optional discovery examines at most 4096 immediate-directory names and at most 256 KiB of names and root-relative paths separately. Large or unreadable directories are explicitly incomplete.

A selected subtitle is read through the authorized root with the existing 4 MiB subtitle limit. Off, another selection, loss of the root, a changed video or source suspension retires the request and its content. Late results cannot reappear in a later video. A failed read does not retry every frame; select the track again to retry. After source suspension, a still-selected track can be read again only after the same source has been revalidated.

## Optional metadata

Place `.ottplay/metadata.json` inside the authorized root if you want titles, full descriptions, durations and artwork. This is an optional versioned convention. No file is created or migrated automatically; absence is normal.

```json
{
  "version": 1,
  "items": [
    {
      "relative_path": "Films/Example.mkv",
      "title": "Example",
      "description": "A full description. Additional sentences are retained.",
      "duration_ms": 120000,
      "poster_relative_path": ".ottplay/posters/example.jpg"
    }
  ]
}
```

Keys and artwork paths are exact root-relative paths, never absolute paths or network URLs. Artwork is explicit JPG, JPEG or PNG; the player does not guess a poster from neighboring files. Invalid or missing optional metadata does not make the video unavailable. Titles and descriptions enrich display data without replacing the source or media identity used by Favorites and resume.

The metadata document is bounded to 8 MiB and uses the shared metadata parser limits. Artwork uses the existing bounded image worker and cache. A Local artwork miss displays the normal placeholder instead of falling back to an unrelated network URL.

## Startup and portability

During playback startup, optional reads yield until the current load reports playback evidence. They use the existing metadata worker slot, so they are not a prerequisite for the first file or the background video catalog scan. Browsing without a pending or retained playback can request a preview directly. Explicit subtitle reads have priority over passive hover previews.

These assets require a currently authorized native Local root. A root path string, an old Favorite or a saved access receipt alone does not grant file access. Browser directory handles and server libraries have their own source adapters; this native integration does not claim to provide those adapters. Files on disconnected volumes remain unavailable until that source is available and authorized again.
