package com.autocut.engine.model

/**
 * The kinds of change the app knows how to make.
 *
 * Every kind maps to exactly one thing the renderer does, which is what keeps the
 * "what did it do to my video" list honest: there is no bucket called
 * "enhancement" hiding three effects.
 */
enum class FixKind {
    /** Drop dead air at the very beginning and end. */
    TRIM_EDGES,

    /** Drop the middles of long silences, keeping a breath at each edge. */
    REMOVE_SILENCE,

    /** Play a long silent-but-active stretch faster instead of cutting it out. */
    SPEED_UP_DEAD_TIME,

    /** Drop stretches that are out of focus while nothing is moving. */
    REMOVE_SOFT_FOCUS,

    /** Bring the overall level to the target loudness. */
    NORMALIZE_LOUDNESS,

    /** Catch the peaks that normalising would otherwise push into clipping. */
    LIMIT_PEAKS,

    /** Lift or pull back overall brightness. */
    FIX_EXPOSURE,

    /** Neutralise a colour cast. */
    FIX_WHITE_BALANCE,

    /** Open up a flat, hazy picture. */
    FIX_CONTRAST,

    /** Add a little colour to a washed-out picture. */
    BOOST_SATURATION,

    /** Cancel handheld shake by moving each frame against the wobble. */
    STABILIZE,

    /** Cap the output resolution so the file is a sane size to share. */
    DOWNSCALE_OUTPUT,
    ;
}

enum class Severity {
    /** Cosmetic. Fine either way. */
    INFO,

    /** The video is noticeably better with this on. */
    SUGGESTED,

    /** Something is actually wrong and this fixes it. */
    IMPORTANT,
}

/**
 * One proposed change, in the form the UI shows it.
 *
 * [detail] carries the real measured numbers rather than an adjective, because
 * the whole point of letting the user overrule the automatic decision is that
 * they can see what it decided and on what evidence.
 */
data class Fix(
    val id: String,
    val kind: FixKind,
    val title: String,
    val detail: String,
    val severity: Severity,
    val enabled: Boolean,
    /** Source duration this fix removes, for the ones that cut. */
    val savedUs: Long = 0L,
) {
    val cuts: Boolean
        get() = kind == FixKind.TRIM_EDGES || kind == FixKind.REMOVE_SILENCE ||
            kind == FixKind.REMOVE_SOFT_FOCUS
}

/**
 * Something found during analysis that the app is deliberately not fixing —
 * either because it cannot be fixed after the fact, or because fixing it would
 * be a guess.
 */
data class Note(
    val id: String,
    val text: String,
    val severity: Severity = Severity.INFO,
)
