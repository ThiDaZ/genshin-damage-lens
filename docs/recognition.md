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

## Burst hits and live load (2026-11-13 recording)

A 1080p recording made with the overlay visible showed its own diagnostics:
about 21 fps and 22.6 ms per frame, against 6–7 ms in offline replays. Replaying
that recording (`--mask 0,330,340,510` blanks the panel) found 21 hits where about
30 numbers are readable. The causes, in order of effect:

- **Evidence thresholds tied to frame count.** A number in a burst is readable only
  until the next one pops in over it, often 100–130 ms. A rule of "six readings or
  150 ms" tuned on 60 fps recordings dropped numbers read four or five times at
  20–30 fps. The bar is now 60 ms on screen with three agreeing readings, or a
  number of four or more digits read whole in its last two frames.
- **Overlapping numbers traded fragments.** When two numbers overlap, each loses
  digits to the other for a frame. The fragment of one was paired with the other's
  track, which moved that track onto the wrong number and lost a 112721. Fragments
  are now absorbed by their own track before any pairing.
- **Cryo/Anemo hue border.** The border at hue 175 cut through Cryo glyphs (hue
  about 180, edge pixels blended towards warm backgrounds), splitting digits into
  two elements. It now sits at 168, midway between the two text colors.
- **The 7 template.** The generic 7 scored about 0.5 against the game's 7, so small
  numbers containing 7 were rejected. A template made from the captured glyph is
  added as an alternate.
- **CPU contention.** With every core busy, eight label workers at normal priority
  took 44–74 ms per frame in a synthetic test (sixteen spinning processes) against
  4 ms idle. The process now runs at above-normal priority class with at most four
  workers, labels only the rows of the combat viewport, and uses a two-level color
  table that stays in cache: 11–13 ms in the same test.
- **GPU read-back.** Mapping a staging texture right after copying into it waits
  for the GPU. Capture now alternates two staging textures and reads back the one
  copied a frame earlier. `capture_bench` measures this path without the game:
  about 2 ms per frame on an idle desktop.

After these changes the same replay yields 30 hits, including every large hit of
the burst. Still missed there: one of two small 677 hits behind a pop-in, and a
stack of overlapping 384s of which one is read (as 381).

The synthetic load test is harsher than a game and proves only the direction. The
live effect has to be read from the panel: the status line shows frames per second,
the footer the per-frame reading and capture times.

## Piles of small hits and raw-frame cost (2026-11-14 recording)

The 2026-11-14 recording was read about equally live and in replay (38 hits
each; roughly 46 numbers are legible in it), while the panel reported 22–32 ms
of reading time per frame and 27–38 fps with a recorder running. Two findings:

- **Raw captures cost more than video frames.** Replays of recordings took about
  10 ms per frame, but the raw DXGI frames saved by `capture_probe` took 15.6 ms
  on an idle machine (p95 23 ms): uncompressed pixels produce more runs and
  more candidate components. CPU load from the game and recorder roughly doubles
  that. The digit splitter now derives each candidate box from per-column
  statistics instead of rescanning and copying the mask, and the labeling loop
  classifies and cuts runs in two tight passes. The same raw frames now take
  7.9 ms (p95 10.8 ms). With every core kept busy by a synthetic load they take
  about 20 ms.
- **Piles of small hits.** A multi-hit attack can put nine numbers of 22–25 px in
  one spot (seven 384s and a 351 in the 2026-11-13 recording). Several are hidden
  under others and cannot be read by anyone. Of the legible ones the reader got
  one, as "381": readings of a 351 and of a 384 thirty pixels away were voted
  into one track digit by digit. A track that has read one value three times now
  refuses a reading that differs by more than one digit and sits off its spot;
  that reading starts its own track. White text also lost its anti-aliased edge
  over warm floors because Physical accepted only near-pure white; warm-tinted
  white up to 16 % saturation is now accepted, which keeps small digits whole.
  The pile now yields 331 (the 351 with one digit wrong) and 384.

A reading that adds two or more digits to the majority reading must now be seen
at least as often as that reading. A neighbor's digits glued to the front of a
number had produced a 902752 from a 2752 next to a 906.

Still open: hits fully covered by others in a pile are not counted, and digits of
22–25 px score only 0.5–0.8 against the templates, so small numbers are read in
fewer frames than large ones.

## Lunar-Bloom numbers and green glow (2026-11-14)

Lunar-Bloom damage was not counted at all, for two separate reasons.

**The numbers are drawn differently.** Measured in the local recording
`2026-11-14 20-39-26.mp4` (Nefer, Lauma, Columbina, Nahida): every digit is a
vertical gradient at full brightness, pale green at the top (about RGB
185,253,180: saturation 30 %) fading to white at the bottom, inside a dark green
outline, and the digits lean right by about 0.27 px per pixel of height. The
element classes cut such a digit into pale Dendro, nothing and Physical, and
the templates are upright, so no digit was ever matched.

- The green part of the gradient has a segmentation label of its own (hue
  60-150, saturation 9-45 %, brightness 88 % or more). It reports as Dendro,
  which is the damage type of Lunar-Bloom.
- The white end cannot be told from white text by color. Taking pixels of 4-8 %
  saturation as well broke up white Physical numbers over green scenery (a 2176
  produced an extra "176"), so those stay Physical. Instead each Lunar-Bloom
  component is grown into the white pixels connected to it, inside a window
  reaching 0.7 of its height below it.
- The completed glyph is sheared upright before clustering, splitting and
  template matching, which then need no knowledge of the lean. Growing by
  connectivity also keeps leaning neighbors out of the mask: their boxes overlap.

**Green glow swallowed ordinary Dendro text.** Lunar-Bloom, Lauma's field and the
Dendro weak-point marker cover the fight in a pale green glow (saturation
20-45 %) that is Dendro by hue, like the fully saturated text (about RGB
0,202,80). Text and glow formed one component, in one measured frame 46,821
pixels around a five-digit number, which the size filter drops. The Dendro
label now carries a split bit like Pyro and Electro: saturation of 60 % or more
is text. Components are compared by label instead of per-element hue ranges.

Measured, original binary against the new one:

- `2026-11-14 20-39-26.mp4`: 34 hits before, 65 after. 23 of the new ones are
  Lunar-Bloom numbers (11980 eight times, 10941 three times, 52860, 18579,
  13934, 49125 and 65500 twice each, 70480, 12949); the repeated values were
  checked against frames and are separate hits. Ten more are ordinary Dendro
  numbers freed from glow, and two misreads ("1004", "52") are gone. With
  `--realtime` the result is the same.
- `sample_video/elements/dendro.mp4`: 26 hits before, 33 after. New: two 4191
  over the weak-point marker, a 298 and several 246/261/345. Gone: a "981" that
  was a misread of an overlapping 298 and 281.
- Six other element recordings and four of five earlier combat recordings:
  identical hits. In the fifth (720p), eight instead of nine false "100"
  readings and one new false "798"; see below.
- Processing time per frame: equal on a recording without Lunar-Bloom (8.5 ms
  against 8.4 ms, run back to back), 7.8 ms against 9.0 ms on the Lunar-Bloom
  recording.

Not measured: how many Lunar-Bloom numbers the recording contains in total, so
this is not a recall figure. Numbers covered by a larger one are still missed
(one of four 10941 in the `lunar-bloom.png` fixture), and while a number pops in
it is larger than the glyph limit. Lunar-Bloom text is larger than ordinary
text, so the height rule marks every one of these hits as a crit. Lunar-Charged
and Lunar-Crystallize were not examined. Live capture was not retested.

Still open, and older than this change: the green enemy label "Lv. 100" is read
as a Dendro 100, and green healing numbers ("+1543") are counted as Dendro damage.

## Stacked equal hits in a pale arena (2026-11-14, second recording)

`2026-11-14 21-36-37.mp4` is a Nefer fight in an arena with pale green walls.
Counting by eye from contact sheets (approximate, not a labeled set), about 33
numbers are legible. The recognizer counted 25, two of them wrong values (a
"109" from a 4409 that was half covered while popping in, a "994" from a 9942
whose last digit was under the next number).

What goes wrong, in order of effect:

- **Equal numbers on top of each other.** Nefer's hits land as two or three
  equal numbers within a quarter of a second. Each one pops in several times
  its final size over the others, and some settle one digit apart: two 95922
  read as "959222" to the eye and as fragments ("922", "95") to the reader.
  Three 29274 gave two hits: the third popped in over the fading first, then
  settled where bright wall ornaments cross it, and was only ever read as
  "2927", "274" and "20274". Nothing here separates touching glyphs of two
  numbers; clusters assume one number per row of glyphs.
- **Wall ornaments in Lunar-Bloom colors.** The walls carry bright lines two to
  four pixels wide in the same pale green as the top of a Lunar-Bloom digit
  (the wall has 33-42 % saturation, the text 28-36 % in recorded video). Where
  a number drifts across them its digits are chained to the lines. Three ways
  of cutting them off were tried and reverted, because each lost hits in
  `2026-11-14 20-39-26.mp4` (65 before): a saturation limit of 32 % (50 hits), a
  text core of at most 32 % with a limited rim of the rest (57), and treating
  runs shorter than three or four pixels as rim only (58, 57). Small
  Lunar-Bloom numbers have strokes as thin as the lines.
- **A steady number covered early.** A 7715 was read five times in a row and
  dropped: it had floated four pixels, under the 4.5 px that mark a track as
  moving at 1080p, when the next hit covered it. A track seen for at most half
  a second now also counts when all of its steps went one way and add up to
  two pixels per scale unit, with four agreeing readings of a value of at
  least 100. Text whose box jitters back and forth still does not count. This
  recovers that hit; the other twelve recordings are unchanged.

The result is 26 hits, 24 of them right. The stacked numbers need a reader
that can take apart overlapping glyphs, which is the recognizer decision
described above rather than another threshold.

## Equal numbers on one spot (2026-11-15)

Frame-by-frame inspection of `2026-11-14 21-36-37.mp4` showed how stacked hits
look to the reader. `label_dump <png> <x,y,w,h> <out.png> [zoom]` draws the
segmentation labels of a region and lists its components and clusters; it was
written for this.

- **The newest number is drawn on top, enlarged.** A number pops in at three to
  four times its final size (a 29274 measured 168 px, then 118 px, against
  45-47 px settled at 1080p) and is whole and unobstructed for two to five
  frames. After that it sits on or under its equals. Those frames were lost:
  168 px is above the 165 px glyph limit, and the tracker took an equal value
  on the same spot for the old number.
- **A returning number was assumed too often.** A value counted within the
  last 1.6 s and then out of view was dropped as "the same number coming back".
  Attacks that repeat a value every second (a 4409 in this recording, 1159,
  646, 5165 in the element recordings) lost every second hit that way.
- **Two numbers side by side form one row of glyphs.** A 25286 next to a 95922
  was one ten-digit cluster and rejected. A 9942 with the "216" of a number
  behind it was read as 9942216.

What changed:

- Lunar-Bloom glyphs are accepted up to 150 px at 720p (225 px at 1080p)
  instead of 110. Other text keeps 110.
- A reading 1.5 times taller than the tallest of a track's last three readings
  is not that track's number: numbers only shrink. It starts its own track. If
  it lies across a track that has established the same value, it counts as a
  hit when seen in two frames, whole in at least one; a partial reading at
  about its size ("29" at 118 px after "29274" at 168 px) is the second
  sighting. Such a track accepts no other value: a 1210661 popping in next to
  it had been paired with it. Three heights are compared because Lunar-Bloom
  digits lose their white lower end in single frames.
- A confirmed number is no longer a repeat of an earlier equal one when it was
  first read at 1.5 times the size the earlier one was last read at.
- A row of at least four glyphs is cut where the glyphs on each side share a
  top line or baseline and the two sides differ by 12 % of the glyph height
  (at least 3 px at 720p). Each side keeps two glyphs or more, and the cut is
  used only when both sides read as numbers. Cutting off a single odd glyph
  would shorten a number whose last digit is damaged.

Measured, the binary before these changes against the one after, all frames
with recording timestamps. Every added hit was looked up in the frames.

- `2026-11-14 21-36-37.mp4`: 26 hits before, 29 after. New: the third 29274
  (frames 536-537, on top of the first), a 25286 (read in the row
  "2528695922" in frame 536), and a 4409 that pops in at frame 490 and had
  been dropped as a repeat of the one a second earlier. 27 of the 29 are right;
  "109" and "994" remain.
- `2026-11-14 20-39-26.mp4`: 65 before, 68 after. New: an 11980 popping in
  over another at frames 328-332, one at 506-510, and the third 10941 at
  688-692. A 7715 is confirmed 0.7 s earlier.
- Nine more hits in recordings without Lunar-Bloom, all equal numbers that pop
  in while or shortly after an earlier one is shown: 471 Physical
  (`2026-09-05 14-47-13`, 36 to 37), 4604 Pyro and 762 Cryo
  (`2026-09-05 14-48-50`, 19 to 21), 646 (cryo, 34 to 35), 298, 281 and 261
  (dendro, 33 to 36), 5165 (hydro, 26 to 27), 1159 (physical, 19 to 20). In
  the first of these a 931 and a false "100" are confirmed at another moment.
- Identical hit lists: `2026-09-05 14-50-08`, `2026-11-13 23-37-01`,
  `2026-11-14 14-34-23`, electro, geo, pyro, and the two recordings without
  damage.

Tried and removed:

- **Closing gaps in enlarged glyphs.** The outline of the small number
  underneath runs through the strokes of the enlarged one as dark lines two to
  four pixels wide. Collecting a glyph over a morphologically closed mask
  (gaps up to 4 px at 1080p) joined neighboring digits, whose outlines are no
  further apart, and lost readings that worked before: the first 29274 in
  three frames, a 95922 in two, a 121065 in four.
- **Preferring a whole reading of the last two frames over a shorter
  majority.** Meant for the "994": the 9942 is read as 994 four times while
  its last digit is covered and whole in one game frame that the recording
  holds twice. The track is confirmed by the regular vote after 0.6 s, where
  the rule does not apply, and lowering the three repeats that vote asks of a
  longer reading would let a neighbor's digit in. Nothing changed in any
  recording, so the rule was taken out again.

Still open in the pale-arena recording:

- **Three 95922 at frames 471-487, none counted.** The first is alone for
  three frames, and in two of them small interface icons and a green trail
  break its "5" ("922" twice). The second pops in at frame 475 ("93922", then
  "95" twice at 150 and 108 px): its digits are found, but pieces of the first
  number attached to them widen their boxes and the template scores fall to
  0.49-0.59. From frame 480 a white flash merges everything. No track there
  ever establishes the value, which the rule for equal numbers needs.
- **"109" from a 4409.** The number lies under a Hydro 7715 for twenty frames.
  Its visible right part is a valid reading ("1" is the stem of the second 4),
  four frames long, and expires one frame before the number is read whole.
  Which of two overlapping numbers of different colors is on top cannot be
  told from the masks: both are cut by an outline.
- **"994"**, as described above.

Processing time per frame is unchanged: 7.2 ms before and 7.3 ms after on the
pale-arena recording, 5.8 ms and 5.9 ms on the dark one, each run twice back
to back. With `--realtime` the dark-arena recording also gives 68 hits.

Not measured: live capture and resolutions other than 720p and 1080p. The 1.5
growth factor and the 12 % step come from these recordings.
