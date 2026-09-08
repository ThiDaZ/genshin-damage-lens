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

Tracker unit tests separately cover repeated values, nearby reordered hits, outlier readings, no-frame gaps, static noise, invalid confidence, expiry and reset. State tests check that reset/pause invalidate in-flight results. Crop tests cover negative monitor origins and intersections.

## Next recognizer decision

Collect complete, manually reviewed combat and no-damage sequences across elements, resolutions, UI scales, effects, overlapping hits and camera movement. Split by recording/session before tuning. Count every visible number; retain difficult and negative examples. Record ambiguous or unreadable labels separately instead of inventing values.

Compare the existing recognizer with a compact whole-number OCR candidate on identical recordings and the same temporal tracker. Report event precision/recall, duplicate rate, damage error, confirmation delay, processing latency and memory. Choose acceptance thresholds before evaluating the held-out recordings. Ship a model only if these measurements justify its added runtime and maintenance cost.

Windows Graphics Capture can be evaluated separately through the capture outcome contract. Live checks should include game startup/minimize/restore, reset during recognition, moving the game across displays, display/device changes, and comparing totals against a manually counted recording. Screenshot replays cannot establish these device behaviors.
