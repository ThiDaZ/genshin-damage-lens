# Recognition fixtures

These full-size PNGs were copied from the existing local gameplay screenshots on
2026-09-08, so the small regression set is available in a fresh checkout.

- `pyro-over-effects.png`: `sample_video/frame_035.png`. Visible orange 4046 and
  14229 damage over a large red attack effect. The old test's Geo wording was
  misleading: these orange glyphs use the same saturated Pyro colors as the Pyro
  recordings. Labels use the number's top-center position.
- `physical-in-geo-sequence.png`: `scratch/elements/geo/frame_039.png`. Two visible
  white Physical 261 numbers. A recording directory's name is not an element label
  for every hit in the recording.
- `no-damage.png`: `sample_video/user_problem_frame.png`, the existing negative
  regression screenshot. Its expected damage list is empty.
- `electro-000.png`, `electro-100.png`, `electro-300.png`: original Electro frames
  142, 143 and 145. One visible 2066 event; times preserve the existing 100 ms
  extraction assumption and deliberately omit frame 144 to exercise a gap.
  `electro-events.json` evaluates confirmation of this single event.

`recognition.json` labels **per-frame OCR**, not unique temporal hit events. This is
a development regression set, not a held-out accuracy benchmark or training set.
It deliberately includes difficult examples and must not be used to claim overall
gameplay accuracy. Keep separate recordings for evaluating future learned OCR.
