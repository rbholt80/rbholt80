package com.autocut.engine.analysis

import com.autocut.engine.model.AudioSample
import com.autocut.engine.model.TimeRange

/**
 * A run of source time that is quiet enough to be a candidate for cutting.
 */
data class SilenceSpan(
    val range: TimeRange,
    val meanDb: Float,
)

/**
 * What the soundtrack looks like as a whole.
 *
 * @param noiseFloorDb   measured level of the windows that fell below the
 *                       threshold — the room when nobody is talking
 * @param programDb      RMS across the parts that are not silence — the working level
 * @param loudDb         90th-percentile level, i.e. how loud the loud parts get
 * @param truePeakDb     the single loudest sample in the file
 * @param clippedFraction share of windows that hit digital full scale
 * @param reliable       false when the audio has too little dynamic range for
 *                       silence detection to mean anything (constant noise,
 *                       music beds, heavily compressed sources)
 */
data class AudioProfile(
    val noiseFloorDb: Float,
    val programDb: Float,
    val loudDb: Float,
    val truePeakDb: Float,
    val thresholdDb: Float,
    val clippedFraction: Float,
    val activeFraction: Float,
    val silences: List<SilenceSpan>,
    val reliable: Boolean,
) {
    val dynamicRangeDb: Float get() = loudDb - noiseFloorDb

    companion object {
        val EMPTY = AudioProfile(
            noiseFloorDb = Dsp.SILENCE_FLOOR_DB,
            programDb = Dsp.SILENCE_FLOOR_DB,
            loudDb = Dsp.SILENCE_FLOOR_DB,
            truePeakDb = Dsp.SILENCE_FLOOR_DB,
            thresholdDb = Dsp.SILENCE_FLOOR_DB,
            clippedFraction = 0f,
            activeFraction = 0f,
            silences = emptyList(),
            reliable = false,
        )
    }
}

/**
 * Turns a list of audio windows into a description of the soundtrack.
 *
 * Deliberately style-agnostic: it finds every quiet run over a very short
 * minimum and leaves it to the planner to decide which of them are long enough
 * to be worth cutting. That way changing the edit style never means decoding the
 * file again.
 */
object AudioAnalyzer {

    /** Runs shorter than this are never reported; no style cuts below it. */
    private const val MIN_DETECT_US = 120_000L

    /** How far above the room tone a window has to be to count as content. */
    private const val NOISE_MARGIN_DB = 7f

    /** Never put the threshold this close to the loud parts. */
    private const val LOUD_HEADROOM_DB = 10f

    /** The threshold is clamped into this window whatever the material says. */
    private const val THRESHOLD_MIN_DB = -70f
    private const val THRESHOLD_MAX_DB = -32f

    /** Schmitt-trigger gap, so a window hovering at the threshold does not chatter. */
    private const val HYSTERESIS_DB = 3f

    /** Below this much separation between room tone and speech, silence means nothing. */
    private const val MIN_USABLE_RANGE_DB = 12f

    private const val CLIPPING_AMPLITUDE = 0.995f

    fun analyze(samples: List<AudioSample>, durationUs: Long): AudioProfile {
        if (samples.isEmpty() || durationUs <= 0L) return AudioProfile.EMPTY

        val levels = FloatArray(samples.size) { Dsp.amplitudeToDb(samples[it].rms) }
        val sorted = levels.copyOf().also { it.sort() }

        val loudDb = Dsp.percentileOfSorted(sorted, 0.90f)
        val truePeakDb = Dsp.amplitudeToDb(samples.maxOf { it.peak })
        val clippedFraction = samples.count { it.peak >= CLIPPING_AMPLITUDE }.toFloat() / samples.size

        // A low percentile is only a seed for the threshold, not the noise floor
        // itself. On a well-paced take where pauses are less than a tenth of the
        // running time, the 10th percentile lands inside the speech and reads far
        // too high — which is exactly why the threshold is also held a fixed
        // distance below the loud parts. Between the two, the split lands in the
        // gap whether pauses are 3% of the clip or 40% of it.
        val quietSeedDb = Dsp.percentileOfSorted(sorted, 0.10f)
        val thresholdDb = (quietSeedDb + NOISE_MARGIN_DB)
            .coerceAtMost(loudDb - LOUD_HEADROOM_DB)
            .coerceIn(THRESHOLD_MIN_DB, THRESHOLD_MAX_DB)

        val quietLevels = levels.filter { it < thresholdDb }
        val activeIndices = levels.indices.filter { levels[it] >= thresholdDb }

        // Reliability is then judged on what the threshold actually separated,
        // rather than on a percentile spread that says nothing about whether the
        // quiet and loud parts are really two different things.
        val noiseFloorDb = if (quietLevels.isEmpty()) quietSeedDb else Dsp.mean(quietLevels.toFloatArray())
        val activeDb = if (activeIndices.isEmpty()) {
            loudDb
        } else {
            Dsp.mean(FloatArray(activeIndices.size) { levels[activeIndices[it]] })
        }
        val reliable = samples.size >= 8 &&
            quietLevels.isNotEmpty() &&
            (activeDb - noiseFloorDb) >= MIN_USABLE_RANGE_DB

        val silences = if (reliable) findSilences(samples, levels, thresholdDb) else emptyList()

        val silentUs = TimeRange.totalDurationUs(silences.map { it.range })
        val activeFraction = ((durationUs - silentUs).toFloat() / durationUs).coerceIn(0f, 1f)

        val programAmplitude = if (activeIndices.isEmpty()) {
            Dsp.rootMeanSquare(FloatArray(samples.size) { samples[it].rms })
        } else {
            Dsp.rootMeanSquare(FloatArray(activeIndices.size) { samples[activeIndices[it]].rms })
        }

        return AudioProfile(
            noiseFloorDb = noiseFloorDb,
            programDb = Dsp.amplitudeToDb(programAmplitude),
            loudDb = loudDb,
            truePeakDb = truePeakDb,
            thresholdDb = thresholdDb,
            clippedFraction = clippedFraction,
            activeFraction = activeFraction,
            silences = silences,
            reliable = reliable,
        )
    }

    /**
     * Schmitt-triggered run detection: a window drops into silence below
     * [thresholdDb] but only climbs back out above `thresholdDb + HYSTERESIS_DB`.
     * Without the gap, a breath sitting right on the threshold splits one pause
     * into a dozen unusable fragments.
     */
    private fun findSilences(
        samples: List<AudioSample>,
        levels: FloatArray,
        thresholdDb: Float,
    ): List<SilenceSpan> {
        val exitDb = thresholdDb + HYSTERESIS_DB
        val spans = ArrayList<SilenceSpan>()

        var runStartUs = -1L
        var runEndUs = -1L
        var runSum = 0.0
        var runCount = 0

        fun closeRun() {
            if (runStartUs >= 0 && runEndUs - runStartUs >= MIN_DETECT_US) {
                spans.add(
                    SilenceSpan(
                        range = TimeRange(runStartUs, runEndUs),
                        meanDb = (runSum / runCount).toFloat(),
                    )
                )
            }
            runStartUs = -1L
            runEndUs = -1L
            runSum = 0.0
            runCount = 0
        }

        for (i in samples.indices) {
            val sample = samples[i]
            val level = levels[i]
            val inRun = runStartUs >= 0
            val quiet = if (inRun) level < exitDb else level < thresholdDb
            if (quiet) {
                if (!inRun) runStartUs = sample.startUs
                runEndUs = sample.endUs
                runSum += level.toDouble()
                runCount++
            } else {
                closeRun()
            }
        }
        closeRun()
        return spans
    }
}
