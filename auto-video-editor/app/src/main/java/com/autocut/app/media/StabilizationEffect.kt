package com.autocut.app.media

import android.graphics.Matrix
import androidx.media3.common.util.UnstableApi
import androidx.media3.effect.MatrixTransformation
import com.autocut.engine.model.StabilizationTrack

/**
 * Moves each frame against the camera's wobble, and zooms in enough to keep the
 * moving edge off screen.
 *
 * The matrix works in normalised device coordinates, where the frame spans -1 to
 * 1 on both axes — so a correction expressed as a fraction of frame size becomes
 * twice that in this space. The zoom is applied first, about the centre, so the
 * translation that follows moves picture that is already outside the visible
 * area into it rather than dragging an empty edge in.
 */
@UnstableApi
class StabilizationEffect(private val track: StabilizationTrack) : MatrixTransformation {

    private val matrix = Matrix()

    override fun getMatrix(presentationTimeUs: Long): Matrix {
        val (offsetX, offsetY) = track.offsetAt(presentationTimeUs)
        matrix.reset()
        matrix.postScale(track.zoom, track.zoom)
        matrix.postTranslate(2f * offsetX, 2f * offsetY)
        return matrix
    }
}
