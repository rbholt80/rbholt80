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

    /** The threshold is clamped into this window whatever the material says. */
    private const val THRESHOLD_MIN_DB = -70f
    private const val THRESHOLD_MAX_DB = -32f

    /** A level holding less than this share of the windows counts as empty. */
    private const val EMPTY_DENSITY_FRACTION = 0.002f
    private const val MIN_EMPTY_DENSITY = 0.5f

    /** A gap only means something with this much material on both sides of it. */
    private const val MIN_SIDE_FRACTION = 0.005f
    private const val MIN_SIDE_WINDOWS = 2f

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

        // The split is found as a GAP in the level distribution rather than from
        // percentiles. A percentile quietly assumes you already know how much of
        // the clip is silence, and both ends of that assumption fail on real
        // recordings. Above about nine tenths silence, the high percentile stops
        // describing the loud parts and starts describing room tone, which
        // declared the most cuttable file there is — a long recording with a
        // short burst of speech — to contain no usable silence at all. Below
        // about a tenth it is worse: with an ordinary sentence-level dynamic
        // range the threshold landed inside the speech and reported a third of
        // the actual speech as silence, which the planner would then cut out.
        //
        // Where the gap is does not depend on how wide either side of it is.
        val gapDb = findQuietGapDb(levels)
        val thresholdDb = gapDb?.coerceIn(THRESHOLD_MIN_DB, THRESHOLD_MAX_DB)
            ?: Dsp.SILENCE_FLOOR_DB

        val quietLevels = levels.filter { it < thresholdDb }
        val activeIndices = levels.indices.filter { levels[it] >= thresholdDb }

        // Reliability is then judged on what the threshold actually separated,
        // rather than on a percentile spread that says nothing about whether the
        // quiet and loud parts are really two different things.
        val noiseFloorDb = if (quietLevels.isEmpty()) thresholdDb else Dsp.mean(quietLevels.toFloatArray())
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
     * Finds the quiet gap in the level distribution: the widest run of empty
     * levels with real material on both sides of it.
     *
     * Speech and room tone are two populations with nothing in between, and it
     * is that emptiness — not the size of either population — that says where
     * one ends and the other begins. Measuring it directly is what makes this
     * behave identically whether pauses are 2% of the running time or 90% of it.
     *
     * Returns null when there is no such gap, which is the honest answer for a
     * music bed, a noisy street, or anything heavily compressed. Those really do
     * have no quiet to find, and inventing a split would cut speech.
     */
    private fun findQuietGapDb(levels: FloatArray): Float? {
        val binCount = (-Dsp.SILENCE_FLOOR_DB).toInt() + 1
        val histogram = FloatArray(binCount)
        for (level in levels) {
            val index = (level - Dsp.SILENCE_FLOOR_DB).toInt().coerceIn(0, binCount - 1)
            histogram[index]++
        }

        // Smoothed across neighbouring levels so a single stray window can
        // neither punch a hole in a population nor look like one on its own.
        val density = FloatArray(binCount) { i ->
            (histogram[maxOf(0, i - 1)] + histogram[i] + histogram[minOf(binCount - 1, i + 1)]) / 3f
        }

        val total = levels.size.toFloat()
        val emptyBelow = maxOf(MIN_EMPTY_DENSITY, EMPTY_DENSITY_FRACTION * total)
        val minSide = maxOf(MIN_SIDE_WINDOWS, MIN_SIDE_FRACTION * total)

        val cumulative = FloatArray(binCount + 1)
        for (i in 0 until binCount) cumulative[i + 1] = cumulative[i] + histogram[i]

        var bestWidth = 0
        var bestStart = -1
        var bestEnd = -1
        var index = 0
        while (index < binCount) {
            if (density[index] >= emptyBelow) {
                index++
                continue
            }
            var end = index
            while (end + 1 < binCount && density[end + 1] < emptyBelow) end++

            val width = end - index + 1
            val populationBelow = cumulative[index]
            val populationAbove = total - cumulative[end + 1]
            if (width >= MIN_USABLE_RANGE_DB &&
                populationBelow >= minSide &&
                populationAbove >= minSide &&
                width > bestWidth
            ) {
                bestWidth = width
                bestStart = index
                bestEnd = end
            }
            index = end + 1
        }

        if (bestStart < 0) return null
        // The middle of the gap, so a slightly noisier room still lands on the
        // quiet side of it.
        return Dsp.SILENCE_FLOOR_DB + (bestStart + bestEnd) / 2f
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
