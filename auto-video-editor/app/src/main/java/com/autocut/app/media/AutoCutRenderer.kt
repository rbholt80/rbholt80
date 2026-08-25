package com.autocut.app.media

import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import androidx.media3.common.C
import androidx.media3.common.Effect
import androidx.media3.common.MediaItem
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.SpeedChangingAudioProcessor
import androidx.media3.common.audio.SpeedProvider
import androidx.media3.common.util.UnstableApi
import androidx.media3.effect.Contrast
import androidx.media3.effect.HslAdjustment
import androidx.media3.effect.Presentation
import androidx.media3.effect.RgbAdjustment
import androidx.media3.effect.SpeedChangeEffect
import androidx.media3.transformer.Composition
import androidx.media3.transformer.EditedMediaItem
import androidx.media3.transformer.EditedMediaItemSequence
import androidx.media3.transformer.Effects
import androidx.media3.transformer.ExportException
import androidx.media3.transformer.ExportResult
import androidx.media3.transformer.ProgressHolder
import androidx.media3.transformer.Transformer
import com.autocut.engine.model.Clip
import com.autocut.engine.model.EditPlan
import kotlinx.coroutines.CancellableContinuation
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import java.io.File
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * Turns an [EditPlan] into a new video file.
 *
 * Each kept clip becomes one item in a Media3 composition, trimmed by the
 * container's own clipping configuration so untouched regions can be handled
 * efficiently, and carrying only the effects that clip actually needs.
 */
@UnstableApi
class AutoCutRenderer(private val context: Context) {

    /**
     * @param onProgress percentage 0..100, called on the main thread
     * @return [output], written in place
     * @throws ExportException if the encode fails
     */
    suspend fun render(
        source: Uri,
        plan: EditPlan,
        output: File,
        onProgress: (Int) -> Unit = {},
    ): File = withContext(Dispatchers.Main) {
        // Transformer posts its callbacks to the Looper of the thread that
        // built it, so it is created and driven from the main thread.
        suspendCancellableCoroutine { continuation ->
            startExport(source, plan, output, onProgress, continuation)
        }
    }

    private fun startExport(
        source: Uri,
        plan: EditPlan,
        output: File,
        onProgress: (Int) -> Unit,
        continuation: CancellableContinuation<File>,
    ) {
        val handler = Handler(Looper.getMainLooper())
        val progressHolder = ProgressHolder()

        val listener = object : Transformer.Listener {
            override fun onCompleted(composition: Composition, exportResult: ExportResult) {
                handler.removeCallbacksAndMessages(null)
                onProgress(100)
                if (continuation.isActive) continuation.resume(output)
            }

            override fun onError(
                composition: Composition,
                exportResult: ExportResult,
                exportException: ExportException,
            ) {
                handler.removeCallbacksAndMessages(null)
                output.delete()
                if (continuation.isActive) continuation.resumeWithException(exportException)
            }
        }

        val transformer = Transformer.Builder(context).addListener(listener).build()

        continuation.invokeOnCancellation {
            // Cancellation can arrive on any thread, and Transformer is not
            // thread safe, so it is stopped on the thread that started it.
            handler.post {
                handler.removeCallbacksAndMessages(null)
                runCatching { transformer.cancel() }
                output.delete()
            }
        }

        val poll = object : Runnable {
            override fun run() {
                val state = transformer.getProgress(progressHolder)
                if (state == Transformer.PROGRESS_STATE_AVAILABLE) onProgress(progressHolder.progress)
                if (continuation.isActive) handler.postDelayed(this, PROGRESS_INTERVAL_MS)
            }
        }

        try {
            transformer.start(buildComposition(source, plan), output.absolutePath)
            handler.postDelayed(poll, PROGRESS_INTERVAL_MS)
        } catch (e: RuntimeException) {
            if (continuation.isActive) continuation.resumeWithException(e)
        }
    }

    /**
     * Effects are handed COMPOSITION-relative timestamps, not item-relative ones.
     *
     * Media3 gives each item a `FrameInfo.offsetToAddUs` equal to the running sum
     * of the previous items' durations, and the frame processor adds that to
     * every frame time before an effect sees it. So the running offset is tracked
     * here and passed down, and the stabilisation track subtracts it again.
     * Without that, every clip after the first would be sampled past the end of
     * its own track and get one frozen correction for its whole length.
     */
    private fun buildComposition(source: Uri, plan: EditPlan): Composition {
        var compositionOffsetUs = 0L
        val items = plan.clips.map { clip ->
            val item = buildItem(source, plan, clip, compositionOffsetUs)
            compositionOffsetUs += clip.outputDurationUs
            item
        }
        return Composition.Builder(listOf(EditedMediaItemSequence(items))).build()
    }

    private fun buildItem(
        source: Uri,
        plan: EditPlan,
        clip: Clip,
        clipStartInCompositionUs: Long,
    ): EditedMediaItem {
        val mediaItem = MediaItem.Builder()
            .setUri(source)
            .setClippingConfiguration(
                MediaItem.ClippingConfiguration.Builder()
                    // Microseconds, not milliseconds. Media3's own offset
                    // accumulator uses the real clipped duration, so rounding the
                    // boundaries to whole milliseconds here would drift the
                    // offsets computed above away from the ones the frame
                    // processor actually applies.
                    .setStartPositionUs(clip.sourceStartUs)
                    .setEndPositionUs(clip.sourceEndUs)
                    .build()
            )
            .build()

        return EditedMediaItem.Builder(mediaItem)
            .setEffects(
                Effects(
                    audioProcessorsFor(plan, clip),
                    videoEffectsFor(plan, clip, clipStartInCompositionUs),
                )
            )
            // Audio presence has to match across every item in a sequence, so
            // this is decided for the whole plan rather than per clip.
            .setRemoveAudio(plan.audio.muted || !plan.source.hasAudio)
            .build()
    }

    private fun videoEffectsFor(
        plan: EditPlan,
        clip: Clip,
        clipStartInCompositionUs: Long,
    ): List<Effect> {
        val effects = ArrayList<Effect>(5)
        val video = plan.video

        if (clip.speed != 1f) effects.add(SpeedChangeEffect(clip.speed))

        // Geometry before colour, and output sizing last, so the presentation
        // stage sees the frame the viewer will actually get.
        video.stabilization?.let { track ->
            // Rebase the track onto this clip's own timeline, then tell the
            // effect where that timeline starts within the composition.
            effects.add(StabilizationEffect(track.forClip(clip), clipStartInCompositionUs))
        }

        if (video.redScale != 1f || video.greenScale != 1f || video.blueScale != 1f) {
            effects.add(
                RgbAdjustment.Builder()
                    .setRedScale(video.redScale)
                    .setGreenScale(video.greenScale)
                    .setBlueScale(video.blueScale)
                    .build()
            )
        }
        if (video.contrast != 0f) effects.add(Contrast(video.contrast))
        if (video.saturationPercent != 0f) {
            effects.add(HslAdjustment.Builder().adjustSaturation(video.saturationPercent).build())
        }
        presentationFor(plan, video.maxShortSidePx)?.let(effects::add)
        return effects
    }

    private fun audioProcessorsFor(plan: EditPlan, clip: Clip): List<AudioProcessor> {
        val processors = ArrayList<AudioProcessor>(2)
        // Sound has to be stretched by the same factor as the picture or the
        // clip drifts out of sync with itself.
        if (clip.speed != 1f) {
            processors.add(SpeedChangingAudioProcessor(ConstantSpeedProvider(clip.speed)))
        }
        if (!plan.audio.isIdentity) processors.add(LoudnessAudioProcessor(plan.audio))
        return processors
    }

    /**
     * Caps the output's short edge while keeping the shape of the frame.
     *
     * Media3 1.4.1 has no "short side" presentation, only width-and-height, so
     * both dimensions are worked out here. The basis is the source's *display*
     * size rather than its encoded size: the decoder has already applied the
     * container's rotation by the time effects run, so a portrait clip recorded
     * as 1920x1080-rotated-90 arrives as 1080x1920. Using the encoded size would
     * pillarbox every portrait video.
     *
     * Dimensions are rounded to even numbers because hardware encoders reject
     * odd ones on chroma-subsampled formats.
     */
    private fun presentationFor(plan: EditPlan, maxShortSidePx: Int): Presentation? {
        if (maxShortSidePx <= 0) return null
        val source = plan.source
        val shortSide = min(source.displayWidth, source.displayHeight)
        if (shortSide <= maxShortSidePx || shortSide <= 0) return null

        val scale = maxShortSidePx.toDouble() / shortSide
        return Presentation.createForWidthAndHeight(
            evenPixels(source.displayWidth * scale),
            evenPixels(source.displayHeight * scale),
            Presentation.LAYOUT_SCALE_TO_FIT,
        )
    }

    private fun evenPixels(value: Double): Int =
        (value.roundToInt() / 2 * 2).coerceAtLeast(2)

    /** One speed for the whole clip, because the planner ramps whole gaps. */
    @UnstableApi
    private class ConstantSpeedProvider(private val speed: Float) : SpeedProvider {
        override fun getSpeed(timeUs: Long): Float = speed
        override fun getNextSpeedChangeTimeUs(timeUs: Long): Long = C.TIME_UNSET
    }

    private companion object {
        const val PROGRESS_INTERVAL_MS = 250L
    }
}
