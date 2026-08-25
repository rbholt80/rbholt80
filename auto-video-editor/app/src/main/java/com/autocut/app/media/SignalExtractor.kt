package com.autocut.app.media

import android.content.Context
import android.net.Uri
import android.util.Log
import com.autocut.engine.analysis.Analysis
import com.autocut.engine.analysis.MediaAnalyzer
import com.autocut.engine.model.MediaSignals
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

/**
 * Runs both decoders over a file and hands back everything the planner needs.
 *
 * This is the only expensive step in the app: it decodes the whole file once.
 * Planning afterwards is arithmetic over the result, so the user can change
 * style and toggle fixes as much as they like without paying for it again.
 */
class SignalExtractor(private val context: Context) {

    private val audio = AudioSignalExtractor(context)
    private val video = VideoSignalExtractor(context)

    /**
     * @param onProgress 0f..1f across the whole analysis
     * @throws MediaReadException when the file cannot be read at all
     */
    suspend fun analyze(uri: Uri, onProgress: (Float) -> Unit = {}): Analysis =
        withContext(Dispatchers.Default) {
            val probe = MediaProbeReader.probe(context, uri)
            ensureActive()

            // A file can have a video track the decoder handles and a soundtrack
            // it does not. Losing the sound costs the pause cuts and the level
            // fix; losing the whole edit over it would be worse.
            val audioSamples = if (probe.hasAudio) {
                try {
                    audio.extract(uri, probe.durationUs) { onProgress(it * AUDIO_SHARE) }
                } catch (e: MediaReadException) {
                    Log.w(TAG, "Soundtrack could not be read; continuing without it", e)
                    emptyList()
                }
            } else {
                emptyList()
            }
            ensureActive()

            val videoSamples = video.extract(uri, probe.durationUs) {
                onProgress(AUDIO_SHARE + it * (1f - AUDIO_SHARE))
            }
            ensureActive()

            onProgress(1f)
            MediaAnalyzer.analyze(
                MediaSignals(
                    probe = probe.copy(hasAudio = probe.hasAudio && audioSamples.isNotEmpty()),
                    audio = audioSamples,
                    video = videoSamples,
                )
            )
        }

    private companion object {
        const val TAG = "SignalExtractor"

        /** Audio decoding is far cheaper than video, so it owns less of the bar. */
        const val AUDIO_SHARE = 0.25f
    }
}
