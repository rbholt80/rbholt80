package com.autocut.engine.model

/**
 * How aggressive the automatic edit should be.
 *
 * The style sets thresholds, never behaviour: every style runs the same analysis
 * and the same planner, so switching style re-decides the whole edit rather than
 * enabling a different code path.
 */
enum class EditStyle(
    val label: String,
    val blurb: String,
    /** A silence must be at least this long before any of it is removed. */
    val minSilenceUs: Long,
    /** Kept at each side of a removed silence so speech does not butt together. */
    val silencePadUs: Long,
    /** Extra silence kept in the middle of a cut gap, so the cut still breathes. */
    val silenceRetainUs: Long,
    /** Leading/trailing silence is trimmed down to this much. */
    val edgeKeepUs: Long,
    /** Below this, a gap is left alone even if it qualifies on length. */
    val minCutUs: Long,
    /** How much of the source the cutting fixes may remove in total. */
    val maxRemovedFraction: Float,
    val allowSpeedRamps: Boolean,
    /** Cuts that come from the picture rather than the soundtrack: out-of-focus,
     *  frozen, or blank stretches. Independent of [allowSpeedRamps], which is
     *  audio-timeline behaviour. */
    val allowVisualCuts: Boolean,
    /** Target programme level in dBFS RMS. */
    val targetLoudnessDb: Float,
) {
    /** Fix the look and the sound, but do not touch the timeline. */
    CLEANUP_ONLY(
        label = "Cleanup only",
        blurb = "Colour and sound fixed. Nothing cut.",
        minSilenceUs = Long.MAX_VALUE,
        silencePadUs = 0L,
        silenceRetainUs = 0L,
        edgeKeepUs = Long.MAX_VALUE,
        minCutUs = Long.MAX_VALUE,
        maxRemovedFraction = 0f,
        allowSpeedRamps = false,
        allowVisualCuts = false,
        targetLoudnessDb = -16f,
    ),

    /** Only the gaps nobody would defend. */
    LIGHT_TOUCH(
        label = "Light touch",
        blurb = "Only the long, obvious pauses.",
        minSilenceUs = 1_200_000L,
        silencePadUs = 150_000L,
        silenceRetainUs = 300_000L,
        edgeKeepUs = 500_000L,
        minCutUs = 250_000L,
        maxRemovedFraction = 0.35f,
        allowSpeedRamps = false,
        allowVisualCuts = false,
        targetLoudnessDb = -17f,
    ),

    /** The default. Tightens the pacing without making it sound clipped. */
    BALANCED(
        label = "Balanced",
        blurb = "Tightens pacing, keeps it natural.",
        minSilenceUs = 600_000L,
        silencePadUs = 110_000L,
        silenceRetainUs = 180_000L,
        edgeKeepUs = 300_000L,
        minCutUs = 150_000L,
        maxRemovedFraction = 0.6f,
        allowSpeedRamps = true,
        allowVisualCuts = true,
        targetLoudnessDb = -16f,
    ),

    /** Talking-head pacing: almost every pause goes. */
    TIGHT(
        label = "Tight",
        blurb = "Jump-cut pacing. Every pause goes.",
        minSilenceUs = 320_000L,
        silencePadUs = 70_000L,
        silenceRetainUs = 60_000L,
        edgeKeepUs = 150_000L,
        minCutUs = 100_000L,
        maxRemovedFraction = 0.8f,
        allowSpeedRamps = true,
        allowVisualCuts = true,
        targetLoudnessDb = -15f,
    ),
    ;

    val cutsTimeline: Boolean get() = this != CLEANUP_ONLY

    companion object {
        val DEFAULT = BALANCED

        fun fromName(name: String?): EditStyle =
            entries.firstOrNull { it.name.equals(name, ignoreCase = true) } ?: DEFAULT
    }
}

/**
 * The style plus whatever the user has personally overruled.
 *
 * [overrides] is keyed by [Fix.id]. An entry means "the user decided this one",
 * and the planner honours it instead of its own judgement — which is the whole
 * contract of the manual mode: you are overruling a decision, not editing a
 * timeline by hand.
 */
data class EditPreferences(
    val style: EditStyle = EditStyle.DEFAULT,
    val overrides: Map<String, Boolean> = emptyMap(),
    /** Off by default on low-end devices, where the per-frame matrix work costs real time. */
    val allowStabilization: Boolean = true,
    /** Cap the output's short edge; 0 lets the planner decide. */
    val maxShortSidePx: Int = 0,
    /** Overrides the style's loudness target when non-null. */
    val targetLoudnessDb: Float? = null,
) {
    val loudnessTargetDb: Float get() = targetLoudnessDb ?: style.targetLoudnessDb

    fun isOverridden(id: String): Boolean = overrides.containsKey(id)

    /** The user's choice for [id], or [default] when they have not expressed one. */
    fun resolve(id: String, default: Boolean): Boolean = overrides[id] ?: default

    fun withFix(id: String, enabled: Boolean): EditPreferences =
        copy(overrides = overrides + (id to enabled))

    fun withStyle(style: EditStyle): EditPreferences = copy(style = style)

    /** Drops user overrides, returning to a fully automatic edit. */
    fun reset(): EditPreferences = copy(overrides = emptyMap())
}
