# Embedded MP2 audio in native HLS

Multiplexed MPEG-TS HLS can contain H.264 video with MPEG Layer II (MP2) audio.
The native player decodes an inspected embedded MP2 track to stereo 48 kHz,
16-bit PCM before feeding the existing television audio pipeline. Video remains
H.264; the stream's reported source codec remains MP2.

Admission requires a bundled MP2 decoder, known mono/stereo source audio at
32, 44.1 or 48 kHz, and measured PCM support on the television. Unknown device
capabilities are not treated as support. The packaged FFmpeg component list
includes the MPEG-audio parser so MPEG-TS inspection can distinguish Layer II
from Layer III correctly.

The decoder and audio timestamp mapping remain continuous across completely
consumed, contiguous segments. Seeking, a discontinuity, a sequence/source
change or an interrupted feed retires that history. PCM duration is derived
from decoded sample counts rather than compressed packet size.

This path applies only to embedded MP2 in external MPEG-TS HLS. It does not
enable MP2 in separate HLS AUDIO renditions, ordinary file/HTTP playback or Plex,
and it does not expand the accepted HLS video codecs, containers or encryption.
Existing AAC handling keeps its own framing and timing path.

Tests use synthetic localhost H.264/MP2 streams and the pinned host FFmpeg ABI
to exercise inspection, capability refusal, PCM route declaration, decoded
samples and timeline changes. Host tests establish those boundaries; successful
playback and audio/video synchronization on a particular TV require a separate
device observation. A failed playlist request occurs before codec handling and
still needs its own network/source diagnosis.

Playlist acquisition distinguishes a request deadline, cancellation, an HTTP
rejection and a network transport failure. Deadline messages identify the initial
or selected playlist, or a stalled response body. Local diagnostics contain only
the fixed stage, nesting depth, elapsed milliseconds and numeric transport/status
codes; source URLs, headers and response bodies are excluded. Acquisition keeps
the existing 15-second request limit, cancellation and size bounds, with no added
retry. A failed cold start alone does not establish that the timeout is too short.
