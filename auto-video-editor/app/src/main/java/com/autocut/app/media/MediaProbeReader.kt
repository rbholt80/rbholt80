package com.autocut.app.media

import android.content.Context
import android.media.MediaMetadataRetriever
import android.net.Uri
import com.autocut.engine.model.MediaProbe

/**
 * Reads what the container says about a video before anything is decoded.
 */
object MediaProbeReader {

    /**
     * @throws MediaReadException when the file cannot be opened or has no video track.
     */
    fun probe(context: Context, uri: Uri): MediaProbe {
        val retriever = MediaMetadataRetriever()
        try {
            retriever.setDataSource(context, uri)

            val durationMs = retriever.longMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)
                ?: throw MediaReadException("This file has no readable duration.")
            val width = retriever.intMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)
                ?: throw MediaReadException("This file has no video track.")
            val height = retriever.intMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)
                ?: throw MediaReadException("This file has no video track.")

            return MediaProbe(
                durationUs = durationMs * 1_000L,
                width = width,
                height = height,
                // Absent on plenty of files. 30 is only used to size the analysis
                // budget, and every real timing comes from frame timestamps.
                frameRate = retriever.floatMetadata(MediaMetadataRetriever.METADATA_KEY_CAPTURE_FRAMERATE)
                    ?: 30f,
                rotationDegrees = retriever.intMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION) ?: 0,
                hasAudio = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_HAS_AUDIO) == "yes",
                bitRate = retriever.longMetadata(MediaMetadataRetriever.METADATA_KEY_BITRATE) ?: 0L,
                mimeType = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_MIMETYPE),
            )
        } catch (e: MediaReadException) {
            throw e
        } catch (e: RuntimeException) {
            // setDataSource throws bare RuntimeException for anything it cannot open.
            throw MediaReadException("This file could not be opened.", e)
        } finally {
            runCatching { retriever.release() }
        }
    }

    private fun MediaMetadataRetriever.intMetadata(key: Int): Int? =
        extractMetadata(key)?.toIntOrNull()

    private fun MediaMetadataRetriever.longMetadata(key: Int): Long? =
        extractMetadata(key)?.toLongOrNull()

    private fun MediaMetadataRetriever.floatMetadata(key: Int): Float? =
        extractMetadata(key)?.toFloatOrNull()?.takeIf { it > 0f }
}

class MediaReadException(message: String, cause: Throwable? = null) : Exception(message, cause)
