# AutoCut — an Android video editor that edits by itself

Point it at a video. It watches the whole thing, decides what is wrong with it,
fixes that, and hands you a new file. You never have to open a timeline.

If you disagree with something it decided, every decision is a switch, and each
one shows the measurement it was based on — so you are overruling a stated
reason, not a black box.

Everything runs on the phone. Nothing is uploaded, and the original file is
never modified: an edit is always written as a new video in `Movies/AutoCut`.

---

## Three ways to use it

| | What you do |
|---|---|
| **Automatic** | Turn on *Edit new recordings automatically*. New videos get edited in the background and a notification tells you when one is ready. |
| **One tap** | Pick a video, wait for the analysis, press **Save video**. |
| **Hands on** | Same, but change the style, flip individual fixes off, and watch the summary update instantly. |

---

## What it fixes

Each of these is one decision, with one switch, and one sentence of evidence.

**Timeline**
- **Trim the top and tail** — dead air before you started talking and after you stopped.
- **Cut the pauses** — the silences in between, shortened rather than erased: a
  configurable breath is left at each cut so speech does not butt together.
- **Speed through the quiet parts** — a long silence with something still moving
  on screen is played faster instead of being cut, so you do not lose the picture.
- **Drop the out-of-focus bits** — stretches that are soft *while nothing is
  moving*, which is a camera hunting for focus rather than a pan.

**Sound**
- **Even out the level** — brings the programme level to a target loudness.
- **Hold the peaks** — a limiter, added only when the gain would otherwise clip.

**Picture**
- **Fix the exposure** — pulls the average frame toward mid grey.
- **Neutralise the colour cast** — grey-world white balance, deliberately partial.
- **Open up a flat picture** — contrast, from the measured tonal spread.
- **Bring the colour back** — saturation, when the frame measures washed out.
- **Steady the camera** — real per-frame stabilisation (see below).
- **Shrink it for sharing** — caps 4K output at 1080p on the short edge.

## What it refuses to do

The interesting part of an automatic editor is what it declines to do. These are
enforced in code, not left to a threshold:

- **It will not cut audio it cannot read.** If the soundtrack never really drops
  to quiet — a music bed, a noisy street, a heavily compressed source — there is
  nothing safe to cut, so the timeline is left alone and it says so.
- **It will not cut you down to nothing.** A removal budget per style, plus a
  floor of 8% of the source, and when the budget binds it keeps the longest
  pauses rather than whichever came first.
- **It will not brighten a frame that is already blowing out.** Past a threshold
  of pure-white pixels, brightening only destroys more highlights.
- **It will not "fix" a sunset.** A colour cast strong enough to be deliberate
  gets half the correction and a note explaining why.
- **It will not push clipped audio louder.** Clipping is already in the
  recording; the level is held back and the damage is reported, not hidden.
- **It will not cut a shot that is soft all the way through.** That is the
  footage, not a run of bad takes — so it is offered switched off.
- **It will not desaturate on its own.** Strong colour is usually a look.
- **It will not invent camera movement.** Where the picture has too little
  structure to measure a shift, the answer is zero, not a confident guess.

---

## How it is put together

```
engine/   pure Kotlin, no Android imports — measurement and judgement
app/      Android — decoders, encoder, UI, background scheduling
```

The split is the point. Deciding what an edit should be is arithmetic over a
list of numbers, so it lives in a plain JVM module and is tested on a laptop in
milliseconds. Getting those numbers out of an H.264 file, and writing a new one,
is platform work and lives in the app.

### The pipeline

```
    file
     │
     ▼
  decode once ─────────────────────────────────► MediaSignals
   audio → one RMS + peak per 100ms window        (a few thousand numbers)
   video → every frame, point-sampled to 96×54
     │
     ▼
  MediaAnalyzer ───────────────────────────────► Analysis
   silence runs, noise floor, loudness, peaks
   exposure, colour, sharpness, camera path
     │
     ▼
  EditPlanner(Analysis, EditPreferences) ───────► EditPlan
   pure function. every toggle re-runs it.        clips + adjustments + fixes
     │
     ▼
  Media3 Transformer ──────────────────────────► new file
```

Decoding is the only expensive step, and it happens once. Changing the style or
flipping a fix re-runs the planner over the same measurements, which is why the
UI updates instantly and why the automatic edit and the hand-tuned edit are the
same code path. `EditPlanner.plan()` is a pure function of its two arguments:
the same file and the same preferences always produce the same edit, and "undo
my change" is just planning again without the override.

### Stabilisation is real, not a crop

The usual shortcut is to zoom in and call it stabilised. This does the actual
thing:

1. **Measure.** Each frame's shift from the last is estimated by matching
   projection profiles — the frame collapsed to a row of column averages and a
   column of row averages. Two one-dimensional searches instead of a
   two-dimensional one, a few thousand operations per frame instead of a few
   hundred thousand. That is what makes it affordable to look at *every* frame,
   and a camera path sampled twice a second is not a camera path.
2. **Separate intent from tremor.** The shifts are integrated into the real
   camera path and low-pass filtered. A deliberate pan is low-frequency and
   survives the filter; hand tremor is high-frequency and does not.
3. **Cancel.** Each frame is translated by the difference between the real path
   and the smoothed one, through a Media3 `MatrixTransformation`.
4. **Hide the edges.** The zoom is computed *from* the correction rather than
   guessed, and if the shake is too big to hide within a 16% crop, the
   correction is scaled down to fit instead of the video being cropped into
   uselessness.

It models translation only. Rotation and rolling shutter are not corrected.

---

## Build

Android Studio, JDK 21. `minSdk 29`, `targetSdk 35`. Toolchain: Gradle 9.5,
AGP 9.3.1, Kotlin 2.2.10.

Open the `auto-video-editor` folder — the one containing `settings.gradle` — not
the repository root.

```bash
./gradlew :engine:test           # the editorial logic, on the JVM, in seconds
./gradlew assembleDebug          # the app
```

From a terminal, the Android SDK has to be findable before either command:
Android Studio writes `local.properties` when it imports the project, otherwise
`export ANDROID_HOME=$HOME/Android/Sdk`. **Every** task needs it, including
`:engine:test` — the Android plugin configures the whole build before any task
runs. SDK platform 35 must be installed, or AGP stops with *"Failed to find
target with hash string android-35"*.

Media3 is pinned to **1.4.1** deliberately. Its composition API changes shape
across versions in both directions — `EditedMediaItemSequence.Builder` and
varargs composition builders arrive in 1.5, `Presentation.createForShortSide`
only in 1.8 — so the version and the call sites in `AutoCutRenderer` move
together or not at all.

Release builds run lint with `abortOnError`. Nothing is suppressed: the three
classes that sit on Media3's `@UnstableApi` surface are marked as such, and the
two that call into them opt in explicitly. If a lint error blocks a release build
after a dependency bump, `./gradlew updateLintBaseline` records the current state
rather than lowering the bar.

### Tests

`./gradlew :engine:test` covers, among others:

- Silence detection against real distributions, including the case that breaks
  a naive noise floor: a well-paced take where pauses are under a tenth of the
  running time and no low percentile lands inside one.
- The removal budget, with the exact clip lengths it should and should not keep.
- Loudness, including that switching the limiter off pulls the gain down with it
  rather than leaving a setting that clips.
- Every picture fix and every guard that declines one.
- Stabilisation: that the correction smooths the path, that a deliberate pan
  survives it, that the zoom always covers the correction, and that violent
  shake scales the correction down rather than the crop up.
- Frame measurement, including that an axis with no structure and a hard cut
  both report *no* shift rather than a confident wrong one.

There are no unit tests for the decoders or the encoder; those need a device and
a real file. They are, however, type-checked: the app module is compiled outside
Android Studio against Robolectric's real API-35 `android.jar`, against Media3
stubs transcribed from the actual 1.4.1 sources, and against `R` and viewBinding
classes generated from the real layouts. That is what caught
`Presentation.createForShortSide` not existing in 1.4.1, and reading Media3's own
source is what caught stabilisation being handed composition-relative timestamps
rather than clip-relative ones — a bug invisible on a single-clip export and
wrong on every real multi-cut edit.

---

## Known limits

- **Analysis decodes the whole file.** A long 4K clip takes real time, roughly
  proportional to what playing it would cost. Recordings past about twenty
  minutes have their frames thinned out, which coarsens the camera path.
- **Stabilisation is translation-only.** No rotation, no rolling-shutter
  correction.
- **The limiter is a `tanh` soft clipper**, not a look-ahead limiter. It cannot
  exceed the ceiling, which is what makes the gain decision safe, but it is a
  gentle colouration rather than transparent.
- **Export always re-encodes.** There is no passthrough for untouched segments.
- **Speech is never transcribed**, so filler words ("um", "so, yeah") are not
  detected — only silence is.
- **An interactive export lives in the screen's ViewModel.** Leaving the app for
  a long time mid-export can lose it. The automatic mode uses WorkManager and
  does not have this problem.
- **10-bit HDR sources are analysed at 8-bit precision.** P010 output is read
  from the high byte of each sample, which is correct but throws away two bits.
- **Speed ramps drive the video effect and the audio processor independently.**
  Media3 offers a paired "experimental speed changing effect" that keeps them
  locked; it is not used here because it is driven by the audio track and this
  app also exports clips with no audio at all. The two are anchored slightly
  differently, so a ramped clip can start with up to about one frame of A/V skew.
- **English only.** No localisation yet.
