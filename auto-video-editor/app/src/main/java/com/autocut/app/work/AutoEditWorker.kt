package com.autocut.app.work

import android.content.Context
import android.content.pm.ServiceInfo
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import androidx.work.CoroutineWorker
import androidx.work.ForegroundInfo
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.WorkManager
import androidx.work.workDataOf
import com.autocut.app.R
import com.autocut.app.data.OutputStore
import com.autocut.app.data.Settings
import com.autocut.app.media.AutoCutRenderer
import com.autocut.app.media.MediaReadException
import com.autocut.app.media.SignalExtractor
import com.autocut.engine.model.EditPlan
import com.autocut.engine.plan.EditPlanner
import kotlinx.coroutines.CancellationException
import java.io.File

/**
 * Edits one video end to end without anyone watching.
 *
 * This is the "does it by itself" path: analyse, decide, render, publish,
 * notify. It runs the same engine and the same renderer as the interactive
 * screen — the automatic mode is not a reduced version of the app, it is the
 * app with nobody overruling it.
 */
@androidx.annotation.OptIn(markerClass = [androidx.media3.common.util.UnstableApi::class])
class AutoEditWorker(
    context: Context,
    params: androidx.work.WorkerParameters,
) : CoroutineWorker(context, params) {

    /**
     * Notification slots derived from the source, so concurrent workers do not
     * overwrite one another.
     *
     * A single shared id meant four videos queued from one library scan fought
     * over one progress notification, and every outcome without an output uri —
     * "nothing to fix" and both failures — collapsed onto one more. Making the
     * pair odd and even guarantees a video's progress and its result never
     * collide either.
     */
    private val sourceKey: Int get() = inputData.getString(KEY_URI).hashCode()
    private val progressNotificationId: Int get() = sourceKey or 1
    private val resultNotificationId: Int get() = sourceKey and 1.inv()

    override suspend fun doWork(): Result {
        val uri = inputData.getString(KEY_URI)?.let(Uri::parse) ?: return Result.failure()
        val context = applicationContext
        val settings = Settings(context)

        setProgressForeground(R.string.notification_analyzing, null)

        val output = File(context.cacheDir, "autocut_${System.currentTimeMillis()}.mp4")
        try {
            val analysis = SignalExtractor(context).analyze(uri) { fraction ->
                setProgressSafely(R.string.notification_analyzing, (fraction * 100).toInt())
            }
            val plan = EditPlanner.plan(analysis, settings.editPreferences())

            if (!plan.changesAnything) {
                notifyResult(
                    context.getString(R.string.notification_nothing_to_fix_title),
                    context.getString(R.string.notification_nothing_to_fix_text, displayName(uri)),
                    null,
                )
                return Result.success()
            }

            setProgressForeground(R.string.notification_exporting, 0)
            AutoCutRenderer(context).render(uri, plan, output) { percent ->
                setProgressSafely(R.string.notification_exporting, percent)
            }

            val saved = OutputStore.publish(
                context,
                output,
                OutputStore.nameFor(displayName(uri), System.currentTimeMillis()),
            )
            notifyResult(
                context.getString(R.string.notification_done_title),
                summaryOf(plan),
                saved,
            )
            return Result.success()
        } catch (e: MediaReadException) {
            Log.w(TAG, "Could not read $uri", e)
            notifyResult(
                context.getString(R.string.notification_failed_title),
                e.message ?: context.getString(R.string.notification_failed_text),
                null,
            )
            return Result.failure()
        } catch (e: CancellationException) {
            // The system stopping this worker is not a failure to report.
            throw e
        } catch (e: Exception) {
            Log.e(TAG, "Auto edit failed for $uri", e)
            // Worth one more attempt: an export can fail because the encoder was
            // busy with the camera, which is a passing condition. Stay quiet
            // while retrying — announcing a failure and then succeeding left the
            // user with a permanent "could not edit that video" notification
            // sitting beside the finished result.
            val retrying = runAttemptCount < 2
            if (!retrying) {
                notifyResult(
                    context.getString(R.string.notification_failed_title),
                    context.getString(R.string.notification_failed_text),
                    null,
                )
            }
            return if (retrying) Result.retry() else Result.failure()
        } finally {
            output.delete()
            // Nothing else ever removes this: setForeground's notification is
            // WorkManager's to cancel, but the progress updates posted directly
            // are not.
            Notifications.cancel(applicationContext, progressNotificationId)
        }
    }

    private fun summaryOf(plan: EditPlan): String {
        val fixes = plan.enabledFixes.size
        return applicationContext.resources.getQuantityString(
            R.plurals.notification_done_text,
            fixes,
            fixes,
            plan.summary(),
        )
    }

    private fun displayName(uri: Uri): String? = runCatching {
        applicationContext.contentResolver
            .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { if (it.moveToFirst()) it.getString(0) else null }
    }.getOrNull()

    /**
     * Used by the expedited path to put the work in the foreground up front.
     *
     * Overriding this rather than relying only on ad-hoc setForeground calls is
     * what gets the job an exemption from the background foreground-service
     * restriction on Android 12+ — without it, a job started from the library
     * watcher while the app is closed cannot promote itself, loses its progress
     * notification, and is killed at JobScheduler's ten-minute ceiling partway
     * through a long export.
     */
    override suspend fun getForegroundInfo(): ForegroundInfo =
        foregroundInfo(R.string.notification_analyzing, null)

    private suspend fun setProgressForeground(titleRes: Int, percent: Int?) {
        try {
            setForeground(foregroundInfo(titleRes, percent))
        } catch (e: Exception) {
            // Never fatal — the edit itself still works — but silence here used
            // to make "the export stopped after ten minutes" undiagnosable.
            Log.w(TAG, "Could not move the edit into the foreground", e)
        }
    }

    private fun setProgressSafely(titleRes: Int, percent: Int) {
        Notifications.notify(
            applicationContext,
            progressNotificationId,
            Notifications.progress(
                applicationContext,
                applicationContext.getString(titleRes),
                applicationContext.getString(R.string.notification_progress_text, percent),
                percent,
            ),
        )
    }

    private fun foregroundInfo(titleRes: Int, percent: Int?): ForegroundInfo {
        val notification = Notifications.progress(
            applicationContext,
            applicationContext.getString(titleRes),
            applicationContext.getString(R.string.notification_working),
            percent,
        )
        return ForegroundInfo(
            progressNotificationId,
            notification,
            ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
        )
    }

    private fun notifyResult(title: String, text: String, video: Uri?) {
        Notifications.notify(
            applicationContext,
            resultNotificationId,
            Notifications.result(applicationContext, title, text, video),
        )
    }

    companion object {
        private const val TAG = "AutoEditWorker"
        private const val KEY_URI = "uri"

        fun enqueue(context: Context, uri: Uri) {
            val request = OneTimeWorkRequestBuilder<AutoEditWorker>()
                .setInputData(workDataOf(KEY_URI to uri.toString()))
                // Expedited so it may start a foreground service from the
                // background; falls back to ordinary work rather than failing
                // when the app's expedited quota is spent.
                .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
                .build()
            // Keyed on the source so a library scan that reports the same video
            // twice does not edit it twice.
            WorkManager.getInstance(context).enqueueUniqueWork(
                "autocut:$uri",
                androidx.work.ExistingWorkPolicy.KEEP,
                request,
            )
        }
    }
}
