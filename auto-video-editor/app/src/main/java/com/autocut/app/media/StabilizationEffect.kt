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
 *
 * @param clipStartInCompositionUs where this clip begins in the finished video.
 *   Media3 hands effects composition-relative timestamps, while [track] has been
 *   rebased to start at zero for this clip, so the two have to be reconciled
 *   here. Getting this wrong is invisible on a single-clip export and freezes
 *   the correction on every clip after the first in a real one.
 */
@UnstableApi
class StabilizationEffect(
    private val track: StabilizationTrack,
    private val clipStartInCompositionUs: Long,
) : MatrixTransformation {

    private val matrix = Matrix()

    override fun getMatrix(presentationTimeUs: Long): Matrix {
        // Media3's matrix space is y-up — Crop documents its top edge as greater
        // than its bottom — while the track is measured in raster order. The
        // conversion between the two lives in the engine, where it is a pure
        // function with a test on it, because nothing that touches a graphics
        // Matrix can be unit tested off a device.
        val (translateX, translateY) =
            track.ndcTranslationAt(presentationTimeUs - clipStartInCompositionUs)
        matrix.reset()
        matrix.postScale(track.zoom, track.zoom)
        matrix.postTranslate(translateX, translateY)
        return matrix
    }
}
