package com.autocut.engine.analysis

import com.autocut.engine.model.MediaProbe
import com.autocut.engine.model.MediaSignals

/**
 * Everything measured about one source file.
 *
 * Analysis is expensive — it means decoding the whole file — and planning is
 * not, so the two are separate. The app analyses once and then re-plans on every
 * toggle, style change and slider drag without touching the decoder again.
 */
data class Analysis(
    val signals: MediaSignals,
    val audio: AudioProfile,
    val video: VideoProfile,
) {
    val probe: MediaProbe get() = signals.probe
}

object MediaAnalyzer {
    fun analyze(signals: MediaSignals): Analysis = Analysis(
        signals = signals,
        audio = if (signals.hasAudioSignal) {
            AudioAnalyzer.analyze(signals.audio, signals.probe.durationUs)
        } else {
            AudioProfile.EMPTY
        },
        video = if (signals.hasVideoSignal) VideoAnalyzer.analyze(signals) else VideoProfile.EMPTY,
    )
}
