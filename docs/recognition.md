# Damage recognition evaluation

The first reliability improvement is an auditable pipeline: correct capture lifecycle, temporal voting, strict whole-number acceptance, and labeled replay scoring. The current implementation retains template OCR. A learned model needs representative training data and a separate evaluation set before it can be considered a better replacement.

## Replay contract

Run `cargo run --manifest-path src-tauri/Cargo.toml --release --example replay -- <manifest.json> --check`.

```json
{
  "mode": "events",
  "frames": [
    { "path": "frame000.png", "timestamp_ms": 0 },
    { "path": "frame001.png", "timestamp_ms": 50 },
    { "path": "frame002.png", "timestamp_ms": 100 }
  ],
  "expected_hits": [
    { "value": 4046, "element": "pyro", "start_ms": 0,
      "end_ms": 350, "x": 500, "y": 230, "radius": 40 }
  ]
}
```

Paths are relative to the manifest. Timestamps must strictly increase and represent recording time, not decoding time. Coordinates identify the horizontal center and top of the number in the game crop. Element values are lowercase. Each label needs a positive radius and a valid inclusive time window. Missing files and malformed manifests are errors.

Use `events` for a consecutive video sequence: one expected label per actual damage event, with a time/position window covering confirmation. Use `frames` for independent screenshots: one label per visible number per screenshot, normally with equal start/end timestamps. Frame mode bypasses temporal confirmation and must not be interpreted as event accuracy. Legacy video diagnostics currently assume 100 ms between extracted frames; new manifests should preserve actual extraction timestamps.

The command prints JSON with observations, matched/missed labels, false hits, duplicates, precision, recall, total-damage error and mean confirmation delay. Matching is one-to-one and requires exact value, element, time window and position tolerance. A wrong value counts as both a false hit and a missed label. Duplicates are a subset of false hits. Empty denominators produce `null`. Mean/p95 processing time excludes PNG decoding, capture and UI; it is not live FPS. `--check` exits 1 for any miss or false hit, 2 for input/runtime errors.

## Checked-in regressions

The full-frame images in `src-tauri/tests/fixtures` cover orange Pyro text over attack effects, two Physical hits in a recording named Geo, scenery without damage, and an Electro 2066 event across three frames with a gap. Labels describe visible pixels rather than inferring element from the recording filename. These are development examples, including images related to the existing templates, not a held-out accuracy estimate. See the fixture README for provenance.

Touching glyphs use a bounded dynamic-programming search over projection valleys. Template confidence plus a per-glyph cost discourages splitting rounded digits into several narrow fragments. The original projection splitter remains a fallback when no valid complete path is found. Electro segmentation separates purple text from reddish scenery inside the broad element hue range. These changes improve the checked-in examples without lowering the final number confidence thresholds.

Tracker unit tests separately cover drift in any direction, the pop-in shrink, fragments of a tracked number, a number returning after occlusion, simultaneous equal hits, consecutive hits with similar values, nearby reordered hits, outlier readings, no-frame gaps, static text, invalid confidence, expiry and reset. State tests check that reset/pause invalidate in-flight results. Crop tests cover negative monitor origins and intersections.

## Next recognizer decision

Collect complete, manually reviewed combat and no-damage sequences across elements, resolutions, UI scales, effects, overlapping hits and camera movement. Split by recording/session before tuning. Count every visible number; retain difficult and negative examples. Record ambiguous or unreadable labels separately instead of inventing values.

Compare the existing recognizer with a compact whole-number OCR candidate on identical recordings and the same temporal tracker. Report event precision/recall, duplicate rate, damage error, confirmation delay, processing latency and memory. Choose acceptance thresholds before evaluating the held-out recordings. Ship a model only if these measurements justify its added runtime and maintenance cost.

Windows Graphics Capture can be evaluated separately through the capture outcome contract. Live checks should include game startup/minimize/restore, reset during recognition, moving the game across displays, display/device changes, and comparing totals against a manually counted recording. Screenshot replays cannot establish these device behaviors.

## Live diagnosis

`pnpm run dev` uses Cargo's dev profile. The app's own pixel loops must be optimized,
not just dependency crates. A local 1080p measurement on the Electro fixture went
from about 325 ms to 63 ms per frame after enabling dev optimization. These are
single local measurements, not a hardware-independent FPS guarantee. Recording
timestamps let offline replays pass even when processing is too slow for live use.

The HUD and terminal now expose received frames/FPS, capture dimensions, raw OCR
candidate count, confirmed hits and mean processing time. Candidates with no
confirmed hits point to insufficient temporal evidence; no candidates during
visible damage point to segmentation/OCR. No received frames point to capture.

To inspect the same capture pipeline locally:

```powershell
cargo run --manifest-path src-tauri/Cargo.toml --example capture_probe -- scratch/live-capture
```

Keep the game visible and fight during the 15-second sample. This command saves at
most 15 game-crop PNGs and a capture summary locally after sampling, and prints OCR
readings. It does not upload images. Image timestamps are sampling metadata, not
manually verified damage labels; annotate before treating them as an accuracy test.

The first live combat probe processed 166 frames in 15 seconds, producing 167 OCR
readings and 12 confirmed hits after dev optimization. It captured a 1920×1080
desktop crop. This proves the pipeline received and confirmed live observations,
not that every attack was counted correctly.

That sample also exposed impact-animation glyphs above the old 82-pixel limit at
1080p. The detector now allows larger glyphs with a bounded pixel budget that
scales by area. Relative Physical-outline contrast uses luminance so saturated
blue effects do not appear as bright as white text merely because a blue channel
is 255. Large vertically broken glyphs may be rejoined when alignment, gap size
and template confidence support a complete digit. The captured `physical-impact`
regression verifies the full Physical 1215 despite overlapping orange text.

## Recording benchmark

`video_bench` feeds a gameplay recording through the same engine as the live loop and
prints confirmed hits and per-frame timing:

```bash
ffmpeg -v error -i clip.mp4 -f rawvideo -pix_fmt bgra - | cargo run --manifest-path src-tauri/Cargo.toml --release --example video_bench -- 1920 1080 60 --realtime
```

Pass the recording's own width, height and frame rate. `--realtime` skips the frames
that would have gone by while the previous one was processed, which is what live
capture experiences. `--detections` prints every single-frame reading and `--trace`
prints each confirmed track with its readings, tracks dropped without a hit, and
repeats that were suppressed. Run it from Git Bash or `cmd`: Windows PowerShell 5
re-encodes binary pipes.

Local measurements on seven 1080p60 element recordings and three 720p30 combat
recordings (one machine, not a hardware-independent guarantee):

| | Before | After |
|---|---|---|
| 1080p processing per frame | about 86 ms | 6–7 ms |
| 720p processing per frame | not measured | 4–5 ms |
| Frames seen at live speed (Pyro clip, 1266 frames) | 207 | 1261 |
| Hits confirmed at live speed (Pyro clip) | 7 | 27 |

The speed came from labeling pixels through a lookup table across cores, building
connected components from row runs instead of a per-pixel flood fill, and bit-packed
template scoring.

The missed hits came mostly from the tracker, not the reader: it confirmed only
numbers that drifted upward, while numbers follow the camera in every direction.
A 25364 read correctly in 45 consecutive frames was never counted. Fragments of a
partly hidden number (a "91" from 19191) were counted as hits of their own.

These recordings have no hand-made labels. The confirmed hits of the Pyro, Cryo,
Hydro, Geo and Physical clips were compared by eye against video frames at each hit;
most match. Known remaining errors there: a 6018 overlapped by another number is
read as 601, a 6318 as 63189, and occasional single-digit misreads. Hits that were
never confirmed are only visible through `--trace`; recall has not been counted.
Treat the table as a before/after comparison, not an accuracy figure.

The live confirmation delay is 0.6 s. Offline event manifests must flush the engine
after the last frame (the `replay` example does) so numbers still on screen confirm.
