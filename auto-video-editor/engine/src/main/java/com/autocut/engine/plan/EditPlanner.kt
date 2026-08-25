package com.autocut.engine.plan

import com.autocut.engine.analysis.Analysis
import com.autocut.engine.model.AudioAdjust
import com.autocut.engine.model.Clip
import com.autocut.engine.model.EditPlan
import com.autocut.engine.model.EditPreferences
import com.autocut.engine.model.Fix
import com.autocut.engine.model.FixKind
import com.autocut.engine.model.Note
import com.autocut.engine.model.Severity
import com.autocut.engine.model.TimeRange
import com.autocut.engine.model.VideoAdjust
import java.util.Locale
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Turns measurements into an edit.
 *
 * The planner is a pure function of ([Analysis], [EditPreferences]). That is the
 * property the whole app leans on: the automatic edit and the hand-tuned one are
 * the same code path, and "undo my change" is just planning again without the
 * override. Nothing here mutates, caches, or remembers.
 *
 * Every decision it makes shows up as a [Fix] with the numbers that justified
 * it, so the user is overruling a stated reason rather than a black box.
 */
object EditPlanner {

    // ---- picture targets -------------------------------------------------
    /** Mid-grey we aim the average frame at. Slightly under 128 reads better on phone screens. */
    private const val TARGET_LUMA = 118f
    private const val EXPOSURE_MIN_SCALE = 0.72f
    private const val EXPOSURE_MAX_SCALE = 1.75f
    private const val EXPOSURE_DEADBAND = 0.06f

    /** Above this share of blown pixels, brightening further just destroys highlights. */
    private const val HIGHLIGHT_GUARD = 0.15f
    private const val HIGHLIGHT_GUARD_SCALE = 1.08f

    /** Below this median luma the shot is dark enough that lifting it will show noise. */
    private const val VERY_DARK_LUMA = 26f

    private const val WHITE_BALANCE_DAMPING = 0.55f
    private const val WHITE_BALANCE_MIN = 0.86f
    private const val WHITE_BALANCE_MAX = 1.16f
    private const val WHITE_BALANCE_DEADBAND = 0.03f

    /** A colour cast this strong is more likely a sunset than a mistake. */
    private const val DELIBERATE_CAST = 0.34f

    private const val TARGET_STDDEV = 52f
    private const val CONTRAST_GAIN = 0.55f
    private const val CONTRAST_MIN = -0.20f
    private const val CONTRAST_MAX = 0.30f
    private const val CONTRAST_DEADBAND = 0.05f

    private const val TARGET_CHROMA = 0.20f
    private const val SATURATION_GAIN = 50f
    private const val SATURATION_MIN = -25f
    private const val SATURATION_MAX = 30f
    private const val SATURATION_DEADBAND = 5f

    // ---- sound targets ---------------------------------------------------
    private const val PEAK_CEILING_DB = -1f

    /** How much the limiter is trusted to hold back before the gain is capped instead. */
    private const val MAX_LIMITED_GAIN_DB = 6f
    private const val MIN_GAIN_DB = -15f
    private const val MAX_GAIN_DB = 20f
    private const val GAIN_DEADBAND_DB = 1f
    private const val CLIPPING_NOTICEABLE = 0.005f

    // ---- timeline --------------------------------------------------------
    /** A gap this long is dead air rather than a pause. */
    private const val LONG_GAP_US = 8_000_000L

    /** ...and if something is moving on screen through it, speeding it up beats cutting it. */
    private const val ACTIVE_GAP_MOTION = 0.05f
    private const val RAMP_TARGET_US = 4_000_000L
    private const val RAMP_MIN_SPEED = 1.6f
    private const val RAMP_MAX_SPEED = 4f

    /** Never leave less than this share of the source, whatever the fixes want. */
    private const val MIN_OUTPUT_FRACTION = 0.08
    private const val MIN_OUTPUT_US = 1_500_000L

    /** Shorter than this and a clip is an encoder problem rather than an edit. */
    private const val MIN_CLIP_US = 120_000L

    /** Cutting more than this share of soft-focus footage means the whole thing is soft. */
    private const val FOCUS_AUTO_LIMIT = 0.25f
    private const val FOCUS_SOFT_CEILING = 0.45f

    // ---- resolution ------------------------------------------------------
    private const val SHARE_SHORT_SIDE = 1080

    // Stable fix identifiers. They are persisted in user overrides, so they must
    // not change once shipped.
    const val ID_TRIM_EDGES = "trim_edges"
    const val ID_REMOVE_SILENCE = "remove_silence"
    const val ID_SPEED_DEAD_TIME = "speed_dead_time"
    const val ID_REMOVE_SOFT_FOCUS = "remove_soft_focus"
    const val ID_NORMALIZE_LOUDNESS = "normalize_loudness"
    const val ID_LIMIT_PEAKS = "limit_peaks"
    const val ID_FIX_EXPOSURE = "fix_exposure"
    const val ID_FIX_WHITE_BALANCE = "fix_white_balance"
    const val ID_FIX_CONTRAST = "fix_contrast"
    const val ID_BOOST_SATURATION = "boost_saturation"
    const val ID_STABILIZE = "stabilize"
    const val ID_DOWNSCALE = "downscale_output"

    fun plan(analysis: Analysis, prefs: EditPreferences = EditPreferences()): EditPlan {
        val probe = analysis.probe
        if (probe.durationUs <= 0L) {
            return EditPlan.untouched(
                probe,
                prefs.style,
                listOf(Note("unreadable", "This file has no readable duration.", Severity.IMPORTANT)),
            )
        }

        val draft = Draft(analysis, prefs)
        planTimeline(draft)
        planSound(draft)
        planPicture(draft)
        planFraming(draft)
        return draft.build()
    }

    // =====================================================================
    // Timeline
    // =====================================================================

    private fun planTimeline(draft: Draft) {
        val probe = draft.probe
        val style = draft.prefs.style
        val audio = draft.analysis.audio

        if (probe.durationUs < 2_000_000L) {
            draft.note("too_short", "Under two seconds long, so nothing was cut out of it.")
        }
        if (!probe.hasAudio) {
            draft.note("no_audio", "This clip has no sound, so there were no pauses to find.")
        } else if (!audio.reliable && style.cutsTimeline) {
            draft.note(
                "audio_flat",
                "The sound never really drops to quiet — constant noise or music underneath — " +
                    "so there is nothing safe to cut. The timeline was left alone.",
                Severity.SUGGESTED,
            )
        }

        if (!style.cutsTimeline || probe.durationUs < 2_000_000L) return

        // Pause-based cuts need trustworthy audio. Focus-based cuts do not, and
        // a clip with no soundtrack at all should still lose its bad takes.
        if (audio.reliable) {
            if (audio.activeFraction < 0.35f) {
                draft.note(
                    "mostly_silent",
                    String.format(
                        Locale.ROOT,
                        "%.0f%% of this clip is silence.",
                        (1f - audio.activeFraction) * 100f,
                    ),
                )
            }

            val edges = splitEdgeSilences(draft)
            planEdgeTrim(draft, edges.leading, edges.trailing)
            planSilenceCuts(draft, edges.interior)
        }
        planFocusCuts(draft)
    }

    private class EdgeSplit(
        val leading: TimeRange?,
        val trailing: TimeRange?,
        val interior: List<TimeRange>,
    )

    /**
     * Silence at the very start and end is a different problem from silence in
     * the middle: it can be taken out wholesale rather than shortened, and it is
     * the fix people notice most.
     */
    private fun splitEdgeSilences(draft: Draft): EdgeSplit {
        val duration = draft.probe.durationUs
        val edgeTolerance = 150_000L
        var leading: TimeRange? = null
        var trailing: TimeRange? = null
        val interior = ArrayList<TimeRange>()
        for (silence in draft.analysis.audio.silences) {
            val range = silence.range
            when {
                range.startUs <= edgeTolerance -> leading = range
                range.endUs >= duration - edgeTolerance -> trailing = range
                else -> interior.add(range)
            }
        }
        return EdgeSplit(leading, trailing, interior)
    }

    private fun planEdgeTrim(draft: Draft, leading: TimeRange?, trailing: TimeRange?) {
        val style = draft.prefs.style
        val duration = draft.probe.durationUs
        val candidates = ArrayList<TimeRange>(2)
        var headUs = 0L
        var tailUs = 0L

        if (leading != null) {
            val cutEnd = leading.endUs - style.edgeKeepUs
            if (cutEnd - leading.startUs >= style.minCutUs) {
                candidates.add(TimeRange(leading.startUs, cutEnd))
                headUs = cutEnd - leading.startUs
            }
        }
        if (trailing != null) {
            val cutStart = trailing.startUs + style.edgeKeepUs
            if (trailing.endUs - cutStart >= style.minCutUs) {
                candidates.add(TimeRange(cutStart, min(trailing.endUs, duration)))
                tailUs = min(trailing.endUs, duration) - cutStart
            }
        }
        if (candidates.isEmpty()) return

        val description = when {
            headUs > 0 && tailUs > 0 -> String.format(
                Locale.ROOT,
                "Dead air trimmed off both ends: %.1fs from the front, %.1fs from the end.",
                headUs / 1e6, tailUs / 1e6,
            )
            headUs > 0 -> String.format(Locale.ROOT, "%.1fs of dead air trimmed off the front.", headUs / 1e6)
            else -> String.format(Locale.ROOT, "%.1fs of dead air trimmed off the end.", tailUs / 1e6)
        }

        draft.addCutFix(
            id = ID_TRIM_EDGES,
            kind = FixKind.TRIM_EDGES,
            title = "Trim the top and tail",
            detail = description,
            severity = Severity.SUGGESTED,
            defaultEnabled = true,
            candidates = candidates,
        )
    }

    private fun planSilenceCuts(draft: Draft, interior: List<TimeRange>) {
        val style = draft.prefs.style
        val cuts = ArrayList<TimeRange>()
        val ramps = ArrayList<SpeedRamp>()
        var longestCutUs = 0L

        // Longest gaps first: when the removal budget binds, the big wins should
        // survive and the marginal quarter-second pauses should be the ones lost.
        val ordered = interior.filter { it.durationUs >= style.minSilenceUs }
            .sortedByDescending { it.durationUs }

        for (gap in ordered) {
            if (gap.durationUs >= LONG_GAP_US &&
                style.allowSpeedRamps &&
                meanMotionOver(draft, gap) >= ACTIVE_GAP_MOTION
            ) {
                // Something is happening on screen through this gap, so cutting
                // it would lose picture the user may want. Compress time instead.
                val speed = (gap.durationUs.toFloat() / RAMP_TARGET_US)
                    .coerceIn(RAMP_MIN_SPEED, RAMP_MAX_SPEED)
                ramps.add(SpeedRamp(gap, speed))
                continue
            }

            val keepUs = style.silencePadUs * 2 + style.silenceRetainUs
            if (gap.durationUs <= keepUs + style.minCutUs) continue
            val cut = TimeRange(
                gap.startUs + style.silencePadUs + style.silenceRetainUs / 2,
                gap.endUs - style.silencePadUs - style.silenceRetainUs / 2,
            )
            if (cut.durationUs < style.minCutUs) continue
            cuts.add(cut)
            longestCutUs = max(longestCutUs, cut.durationUs)
        }

        if (cuts.isNotEmpty()) {
            val totalUs = TimeRange.totalDurationUs(cuts)
            draft.addCutFix(
                id = ID_REMOVE_SILENCE,
                kind = FixKind.REMOVE_SILENCE,
                title = "Cut the pauses",
                detail = String.format(
                    Locale.ROOT,
                    "%d pause%s removed, %.1fs in total (longest %.1fs). A %.2fs breath is left at each cut " +
                        "so the speech does not butt together.",
                    cuts.size,
                    if (cuts.size == 1) "" else "s",
                    totalUs / 1e6,
                    longestCutUs / 1e6,
                    style.silenceRetainUs / 1e6,
                ),
                severity = Severity.SUGGESTED,
                defaultEnabled = true,
                candidates = cuts.sortedByDescending { it.durationUs },
            )
        }

        if (ramps.isNotEmpty()) {
            val totalUs = ramps.sumOf { it.range.durationUs }
            val averageSpeed = ramps.map { it.speed }.average()
            draft.addRampFix(
                id = ID_SPEED_DEAD_TIME,
                title = "Speed through the quiet parts",
                detail = String.format(
                    Locale.ROOT,
                    "%d long quiet stretch%s (%.1fs) plays at about %.1fx instead of being cut, " +
                        "because there is still something happening on screen.",
                    ramps.size,
                    if (ramps.size == 1) "" else "es",
                    totalUs / 1e6,
                    averageSpeed,
                ),
                defaultEnabled = true,
                ramps = ramps,
            )
        }
    }

    private fun planFocusCuts(draft: Draft) {
        val style = draft.prefs.style
        if (!style.allowFocusCuts) return
        val spans = draft.analysis.video.focusSpans
            .filter { it.relativeSharpness < FOCUS_SOFT_CEILING }
        if (spans.isEmpty()) return

        val totalUs = TimeRange.totalDurationUs(spans.map { it.range })
        val share = totalUs.toFloat() / draft.probe.durationUs

        // If most of the clip is soft, the clip is soft — that is a property of
        // the footage, not a run of bad takes, and cutting it all out is not a
        // fix. Offer it, switched off, and say why.
        val defaultEnabled = share <= FOCUS_AUTO_LIMIT
        if (!defaultEnabled) {
            draft.note(
                "mostly_soft",
                String.format(
                    Locale.ROOT,
                    "%.0f%% of this clip is out of focus, which usually means the whole shot is soft " +
                        "rather than a few bad moments. Removing it is offered but switched off.",
                    share * 100f,
                ),
                Severity.SUGGESTED,
            )
        }

        draft.addCutFix(
            id = ID_REMOVE_SOFT_FOCUS,
            kind = FixKind.REMOVE_SOFT_FOCUS,
            title = "Drop the out-of-focus bits",
            detail = String.format(
                Locale.ROOT,
                "%d stretch%s (%.1fs) where the camera was hunting for focus and nothing was moving.",
                spans.size,
                if (spans.size == 1) "" else "es",
                totalUs / 1e6,
            ),
            severity = if (defaultEnabled) Severity.SUGGESTED else Severity.INFO,
            defaultEnabled = defaultEnabled,
            candidates = spans.map { it.range }.sortedByDescending { it.durationUs },
        )
    }

    private fun meanMotionOver(draft: Draft, range: TimeRange): Float {
        val frames = draft.analysis.signals.video.filter { it.timeUs in range }
        if (frames.isEmpty()) return 0f
        return frames.map { it.motion }.average().toFloat()
    }

    // =====================================================================
    // Sound
    // =====================================================================

    private fun planSound(draft: Draft) {
        val probe = draft.probe
        val audio = draft.analysis.audio
        if (!probe.hasAudio || audio.programDb <= -95f) return

        val clipped = audio.clippedFraction > CLIPPING_NOTICEABLE
        if (clipped) {
            draft.note(
                "audio_clipped",
                String.format(
                    Locale.ROOT,
                    "The audio is already clipped in %.1f%% of the file. That distortion is part of the " +
                        "recording and cannot be undone — the level was held back rather than pushed up.",
                    audio.clippedFraction * 100f,
                ),
                Severity.IMPORTANT,
            )
        }

        val target = draft.prefs.loudnessTargetDb
        var desiredGainDb = target - audio.programDb
        if (clipped) desiredGainDb = min(desiredGainDb, 0f)

        val headroomDb = PEAK_CEILING_DB - audio.truePeakDb
        val needsLimiter = desiredGainDb > headroomDb
        val gainDb = if (needsLimiter) {
            min(desiredGainDb, headroomDb + MAX_LIMITED_GAIN_DB)
        } else {
            desiredGainDb
        }.coerceIn(MIN_GAIN_DB, MAX_GAIN_DB)

        if (abs(gainDb) >= GAIN_DEADBAND_DB) {
            val direction = if (gainDb > 0) "raised" else "pulled back"
            draft.addAudioFix(
                id = ID_NORMALIZE_LOUDNESS,
                kind = FixKind.NORMALIZE_LOUDNESS,
                title = "Even out the level",
                detail = String.format(
                    Locale.ROOT,
                    "Recorded at %.1f dBFS, %s %.1f dB to hit %.0f dBFS.",
                    audio.programDb, direction, abs(gainDb), target,
                ),
                severity = if (abs(gainDb) > 6f) Severity.IMPORTANT else Severity.SUGGESTED,
                defaultEnabled = true,
            )
            draft.gainDb = gainDb
        }

        if (needsLimiter && gainDb > 0f) {
            draft.addAudioFix(
                id = ID_LIMIT_PEAKS,
                kind = FixKind.LIMIT_PEAKS,
                title = "Hold the peaks",
                detail = String.format(
                    Locale.ROOT,
                    "Peaks already reach %.1f dBFS. A limiter holds them at %.1f dBFS so the louder " +
                        "level does not distort.",
                    audio.truePeakDb, PEAK_CEILING_DB,
                ),
                severity = Severity.IMPORTANT,
                defaultEnabled = true,
            )
            draft.limiterAvailable = true
            draft.headroomDb = headroomDb
        }
    }

    // =====================================================================
    // Picture
    // =====================================================================

    private fun planPicture(draft: Draft) {
        if (!draft.analysis.signals.hasVideoSignal) return
        val exposure = draft.analysis.video.exposure

        planExposure(draft)
        planWhiteBalance(draft)
        planContrast(draft)
        planSaturation(draft)

        if (exposure.medianLuma < VERY_DARK_LUMA) {
            draft.note(
                "very_dark",
                String.format(
                    Locale.ROOT,
                    "This was shot very dark (average brightness %.0f out of 255). Lifting it also lifts " +
                        "the sensor noise — there is only so much to recover.",
                    exposure.medianLuma,
                ),
                Severity.SUGGESTED,
            )
        }
    }

    private fun planExposure(draft: Draft) {
        val exposure = draft.analysis.video.exposure
        var scale = (TARGET_LUMA / max(exposure.medianLuma, 4f))
            .coerceIn(EXPOSURE_MIN_SCALE, EXPOSURE_MAX_SCALE)

        // Brightening a frame that is already blowing out just moves more of it
        // to pure white, and pure white cannot be brought back.
        if (scale > 1f && exposure.highlightRatio > HIGHLIGHT_GUARD) {
            scale = min(scale, HIGHLIGHT_GUARD_SCALE)
        }
        if (abs(scale - 1f) < EXPOSURE_DEADBAND) return

        val direction = if (scale > 1f) "Brightened" else "Pulled back"
        draft.addPictureFix(
            id = ID_FIX_EXPOSURE,
            kind = FixKind.FIX_EXPOSURE,
            title = "Fix the exposure",
            detail = String.format(
                Locale.ROOT,
                "Average brightness is %.0f out of 255. %s %.2fx toward %.0f.",
                exposure.medianLuma, direction, scale, TARGET_LUMA,
            ),
            severity = if (abs(scale - 1f) > 0.3f) Severity.IMPORTANT else Severity.SUGGESTED,
            defaultEnabled = true,
        )
        draft.exposureScale = scale
    }

    /**
     * Grey-world white balance: on average, a scene's colours should add up to
     * grey, so whatever the average is off by is the cast.
     *
     * It is only ever partly applied. Grey-world is wrong exactly when a scene
     * is legitimately dominated by one colour — a sunset, a neon sign, a green
     * field — and there is no way to tell that from a mistake, so a strong cast
     * gets a lighter correction rather than a confident one.
     */
    private fun planWhiteBalance(draft: Draft) {
        val exposure = draft.analysis.video.exposure
        val y = exposure.medianLuma
        val u = exposure.meanU - 128f
        val v = exposure.meanV - 128f

        val r = y + 1.402f * v
        val g = y - 0.344136f * u - 0.714136f * v
        val b = y + 1.772f * u
        if (r < 1f || g < 1f || b < 1f) return

        val grey = (r + g + b) / 3f
        val damping = if (exposure.colorCast > DELIBERATE_CAST) WHITE_BALANCE_DAMPING * 0.5f
        else WHITE_BALANCE_DAMPING

        fun correct(channel: Float): Float =
            (1f + (grey / channel - 1f) * damping).coerceIn(WHITE_BALANCE_MIN, WHITE_BALANCE_MAX)

        val rScale = correct(r)
        val gScale = correct(g)
        val bScale = correct(b)
        val deviation = maxOf(abs(rScale - 1f), abs(gScale - 1f), abs(bScale - 1f))
        if (deviation < WHITE_BALANCE_DEADBAND) return

        if (exposure.colorCast > DELIBERATE_CAST) {
            draft.note(
                "strong_cast",
                "The colour cast is strong enough that it might be deliberate, so only part of it " +
                    "was corrected.",
            )
        }

        val warmth = if (v > 0f) "warm" else "cool"
        draft.addPictureFix(
            id = ID_FIX_WHITE_BALANCE,
            kind = FixKind.FIX_WHITE_BALANCE,
            title = "Neutralise the colour cast",
            detail = String.format(
                Locale.ROOT,
                "A %s cast was measured across the clip. Red x%.2f, green x%.2f, blue x%.2f.",
                warmth, rScale, gScale, bScale,
            ),
            severity = Severity.SUGGESTED,
            defaultEnabled = true,
        )
        draft.whiteBalance = floatArrayOf(rScale, gScale, bScale)
    }

    private fun planContrast(draft: Draft) {
        val exposure = draft.analysis.video.exposure
        // Already clipping at both ends: the picture has all the contrast it can
        // hold, and adding more only crushes detail.
        if (exposure.highlightRatio > 0.12f && exposure.shadowRatio > 0.12f) return

        val contrast = (((TARGET_STDDEV - exposure.medianStdDev) / TARGET_STDDEV) * CONTRAST_GAIN)
            .coerceIn(CONTRAST_MIN, CONTRAST_MAX)
        if (abs(contrast) < CONTRAST_DEADBAND) return

        draft.addPictureFix(
            id = ID_FIX_CONTRAST,
            kind = FixKind.FIX_CONTRAST,
            title = if (contrast > 0) "Open up a flat picture" else "Ease off the contrast",
            detail = String.format(
                Locale.ROOT,
                "Tonal spread measures %.0f against a target of %.0f. Contrast %+.2f.",
                exposure.medianStdDev, TARGET_STDDEV, contrast,
            ),
            severity = Severity.INFO,
            defaultEnabled = true,
        )
        draft.contrast = contrast
    }

    private fun planSaturation(draft: Draft) {
        val exposure = draft.analysis.video.exposure
        if (exposure.meanChroma <= 0f) return

        val percent = (((TARGET_CHROMA - exposure.meanChroma) / TARGET_CHROMA) * SATURATION_GAIN)
            .coerceIn(SATURATION_MIN, SATURATION_MAX)
        if (abs(percent) < SATURATION_DEADBAND) return

        // Adding colour to a washed-out clip is usually right. Taking colour away
        // usually is not — it is far more often a look than a mistake — so it is
        // offered switched off.
        val defaultEnabled = percent > 0f
        draft.addPictureFix(
            id = ID_BOOST_SATURATION,
            kind = FixKind.BOOST_SATURATION,
            title = if (percent > 0) "Bring the colour back" else "Tone the colour down",
            detail = String.format(
                Locale.ROOT,
                "Colour intensity measures %.2f against a target of %.2f. Saturation %+.0f%%.",
                exposure.meanChroma, TARGET_CHROMA, percent,
            ),
            severity = Severity.INFO,
            defaultEnabled = defaultEnabled,
        )
        draft.saturationPercent = percent
    }

    // =====================================================================
    // Framing
    // =====================================================================

    private fun planFraming(draft: Draft) {
        planStabilization(draft)
        planDownscale(draft)
    }

    private fun planStabilization(draft: Draft) {
        val shake = draft.analysis.video.shake
        if (!shake.isShaky) return

        val track = shake.track
        if (track == null || track.isEmpty) {
            draft.note(
                "shake_unfixable",
                "The camera moves too much to steady without cropping the picture badly, so it was " +
                    "left as shot.",
                Severity.SUGGESTED,
            )
            return
        }
        if (!draft.prefs.allowStabilization) {
            draft.note("stabilization_off", "Stabilisation is switched off in settings.")
            return
        }

        val zoomPercent = ((track.zoom - 1f) * 100f).roundToInt()
        draft.addPictureFix(
            id = ID_STABILIZE,
            kind = FixKind.STABILIZE,
            title = "Steady the camera",
            detail = String.format(
                Locale.ROOT,
                "Handheld wobble measures %.1f%% of the frame between frames. Each frame is moved " +
                    "against it, and the picture is zoomed %d%% so the moving edges stay off screen.",
                shake.shakeIndex * 100f, zoomPercent,
            ),
            severity = if (shake.shakeIndex > 0.02f) Severity.IMPORTANT else Severity.SUGGESTED,
            defaultEnabled = true,
        )
        draft.stabilization = track
    }

    private fun planDownscale(draft: Draft) {
        val probe = draft.probe
        val shortSide = min(probe.displayWidth, probe.displayHeight)
        if (shortSide <= SHARE_SHORT_SIDE) return

        val requested = draft.prefs.maxShortSidePx.takeIf { it > 0 } ?: SHARE_SHORT_SIDE
        if (shortSide <= requested) return

        val ratio = (shortSide.toFloat() / requested)
        val sizeFactor = ratio * ratio
        val scaledLong = (max(probe.displayWidth, probe.displayHeight) * requested / shortSide.toFloat())
            .roundToInt()

        draft.addPictureFix(
            id = ID_DOWNSCALE,
            kind = FixKind.DOWNSCALE_OUTPUT,
            title = "Shrink it for sharing",
            detail = String.format(
                Locale.ROOT,
                "%dx%d is more than most things will play. Exporting at %dx%d — roughly %.1fx smaller.",
                probe.displayWidth, probe.displayHeight,
                if (probe.isPortrait) requested else scaledLong,
                if (probe.isPortrait) scaledLong else requested,
                sizeFactor,
            ),
            severity = Severity.INFO,
            defaultEnabled = shortSide > 1440,
        )
        draft.maxShortSidePx = requested
    }

    // =====================================================================
    // Draft
    // =====================================================================

    /**
     * Collects proposals while planning runs, then assembles them.
     *
     * Every `add*Fix` resolves the user's override immediately, so a fix the user
     * has switched off never reaches the timeline, never spends removal budget,
     * and never contributes a colour scale — while still appearing in the list
     * with the reasoning that would have applied.
     */
    private class Draft(val analysis: Analysis, val prefs: EditPreferences) {
        val probe = analysis.probe
        private val fixes = ArrayList<Fix>()
        private val notes = ArrayList<Note>()
        private val cuts = ArrayList<TimeRange>()
        private val ramps = ArrayList<SpeedRamp>()

        /** Source time still available to remove before the guards bite. */
        private var budgetUs: Long = run {
            val floorUs = max(MIN_OUTPUT_US, (probe.durationUs * MIN_OUTPUT_FRACTION).toLong())
            min(
                (probe.durationUs * prefs.style.maxRemovedFraction.toDouble()).toLong(),
                (probe.durationUs - floorUs).coerceAtLeast(0L),
            )
        }

        var gainDb = 0f
        var limiterAvailable = false
        var headroomDb = 0f
        var exposureScale = 1f
        var whiteBalance: FloatArray? = null
        var contrast = 0f
        var saturationPercent = 0f
        var stabilization: com.autocut.engine.model.StabilizationTrack? = null
        var maxShortSidePx = 0

        private val enabledIds = HashSet<String>()

        fun note(id: String, text: String, severity: Severity = Severity.INFO) {
            if (notes.none { it.id == id }) notes.add(Note(id, text, severity))
        }

        fun isEnabled(id: String): Boolean = id in enabledIds

        fun addCutFix(
            id: String,
            kind: FixKind,
            title: String,
            detail: String,
            severity: Severity,
            defaultEnabled: Boolean,
            candidates: List<TimeRange>,
        ) {
            val enabled = prefs.resolve(id, defaultEnabled)
            var savedUs = TimeRange.totalDurationUs(candidates)
            if (enabled) {
                val accepted = ArrayList<TimeRange>(candidates.size)
                for (range in candidates) {
                    if (range.durationUs <= budgetUs) {
                        budgetUs -= range.durationUs
                        accepted.add(range)
                    }
                }
                if (accepted.isEmpty()) return
                cuts.addAll(accepted)
                savedUs = TimeRange.totalDurationUs(accepted)
                enabledIds.add(id)
            }
            fixes.add(Fix(id, kind, title, detail, severity, enabled, savedUs))
        }

        fun addRampFix(
            id: String,
            title: String,
            detail: String,
            defaultEnabled: Boolean,
            ramps: List<SpeedRamp>,
        ) {
            val enabled = prefs.resolve(id, defaultEnabled)
            if (enabled) {
                this.ramps.addAll(ramps)
                enabledIds.add(id)
            }
            val savedUs = ramps.sumOf { ramp ->
                ramp.range.durationUs - (ramp.range.durationUs / ramp.speed).toLong()
            }
            fixes.add(
                Fix(id, FixKind.SPEED_UP_DEAD_TIME, title, detail, Severity.INFO, enabled, savedUs)
            )
        }

        fun addAudioFix(
            id: String,
            kind: FixKind,
            title: String,
            detail: String,
            severity: Severity,
            defaultEnabled: Boolean,
        ) = addSimpleFix(id, kind, title, detail, severity, defaultEnabled)

        fun addPictureFix(
            id: String,
            kind: FixKind,
            title: String,
            detail: String,
            severity: Severity,
            defaultEnabled: Boolean,
        ) = addSimpleFix(id, kind, title, detail, severity, defaultEnabled)

        private fun addSimpleFix(
            id: String,
            kind: FixKind,
            title: String,
            detail: String,
            severity: Severity,
            defaultEnabled: Boolean,
        ) {
            val enabled = prefs.resolve(id, defaultEnabled)
            if (enabled) enabledIds.add(id)
            fixes.add(Fix(id, kind, title, detail, severity, enabled, 0L))
        }

        fun build(): EditPlan {
            val clips = assembleClips()
            return EditPlan(
                source = probe,
                clips = clips,
                video = buildVideoAdjust(),
                audio = buildAudioAdjust(),
                fixes = fixes.sortedBy { presentationOrder(it.kind) },
                notes = notes,
                style = prefs.style,
            )
        }

        private fun assembleClips(): List<Clip> {
            val clips = Timeline.assemble(
                durationUs = probe.durationUs,
                cuts = cuts,
                ramps = ramps,
                minClipUs = MIN_CLIP_US,
            )
            // Every guard above should make this unreachable; if one ever fails,
            // handing back the untouched source beats handing back nothing.
            if (clips.isEmpty()) {
                note(
                    "cut_everything",
                    "The cuts would have removed the whole clip, so it was left as it is.",
                    Severity.IMPORTANT,
                )
                return listOf(Clip(0L, probe.durationUs))
            }
            return clips
        }

        private fun buildVideoAdjust(): VideoAdjust {
            val exposure = if (isEnabled(ID_FIX_EXPOSURE)) exposureScale else 1f
            val wb = if (isEnabled(ID_FIX_WHITE_BALANCE)) whiteBalance else null
            return VideoAdjust(
                redScale = exposure * (wb?.get(0) ?: 1f),
                greenScale = exposure * (wb?.get(1) ?: 1f),
                blueScale = exposure * (wb?.get(2) ?: 1f),
                contrast = if (isEnabled(ID_FIX_CONTRAST)) contrast else 0f,
                saturationPercent = if (isEnabled(ID_BOOST_SATURATION)) saturationPercent else 0f,
                stabilization = if (isEnabled(ID_STABILIZE)) stabilization else null,
                maxShortSidePx = if (isEnabled(ID_DOWNSCALE)) maxShortSidePx else 0,
            )
        }

        private fun buildAudioAdjust(): AudioAdjust {
            if (!isEnabled(ID_NORMALIZE_LOUDNESS)) return AudioAdjust()
            val limiter = limiterAvailable && isEnabled(ID_LIMIT_PEAKS)
            // Without the limiter the same gain would drive the peaks into
            // clipping, so turning the limiter off has to pull the gain back to
            // whatever headroom actually exists.
            val gain = if (limiterAvailable && !limiter) min(gainDb, headroomDb) else gainDb
            return AudioAdjust(
                gainDb = gain,
                limiterEnabled = limiter,
                limiterCeilingDb = PEAK_CEILING_DB,
            )
        }

        private fun presentationOrder(kind: FixKind): Int = when (kind) {
            FixKind.TRIM_EDGES -> 0
            FixKind.REMOVE_SILENCE -> 1
            FixKind.SPEED_UP_DEAD_TIME -> 2
            FixKind.REMOVE_SOFT_FOCUS -> 3
            FixKind.NORMALIZE_LOUDNESS -> 4
            FixKind.LIMIT_PEAKS -> 5
            FixKind.FIX_EXPOSURE -> 6
            FixKind.FIX_WHITE_BALANCE -> 7
            FixKind.FIX_CONTRAST -> 8
            FixKind.BOOST_SATURATION -> 9
            FixKind.STABILIZE -> 10
            FixKind.DOWNSCALE_OUTPUT -> 11
        }
    }
}
