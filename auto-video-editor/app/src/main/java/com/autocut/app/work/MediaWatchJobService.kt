package com.autocut.app.work

import android.app.job.JobParameters
import android.app.job.JobService
import android.content.ContentUris
import android.content.Context
import android.provider.MediaStore
import android.util.Log
import com.autocut.app.data.OutputStore
import com.autocut.app.data.Settings
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * Woken by the system when the video library changes.
 *
 * All it does is work out which entries are new and hand each one to
 * [AutoEditWorker]; the editing itself belongs to WorkManager, which survives
 * the job ending and can be resumed after a reboot.
 */
class MediaWatchJobService : JobService() {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var work: Job? = null

    override fun onStartJob(params: JobParameters?): Boolean {
        // Re-arming happens in the finally below, not here. A content trigger is
        // consumed when it fires and has to be put back — but rescheduling this
        // job's own id while it is executing makes JobScheduler tear the
        // execution down, losing the scan. finally still covers the crash case.
        if (!Settings(this).autoEditNewVideos) {
            MediaWatchScheduler.schedule(applicationContext)
            return false
        }

        work = scope.launch {
            try {
                queueNewVideos(applicationContext)
            } catch (e: SecurityException) {
                // Permission to read the library was revoked while the job was
                // armed. Turn the mode off rather than waking up to fail.
                Log.w(TAG, "Library is no longer readable; turning automatic mode off", e)
                MediaWatchScheduler.disable(applicationContext)
            } catch (e: Exception) {
                Log.e(TAG, "Could not scan for new videos", e)
            } finally {
                MediaWatchScheduler.schedule(applicationContext)
                jobFinished(params, false)
            }
        }
        return true
    }

    override fun onStopJob(params: JobParameters?): Boolean {
        work?.cancel()
        // Worth rescheduling: whatever was added is still there to be found.
        return true
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private fun queueNewVideos(context: Context) {
        val settings = Settings(context)
        val lastSeen = settings.lastSeenVideoId
        var highestSeen = lastSeen
        var queued = 0

        val projection = arrayOf(
            MediaStore.Video.Media._ID,
            MediaStore.Video.Media.DURATION,
            MediaStore.Video.Media.RELATIVE_PATH,
        )
        context.contentResolver.query(
            MediaStore.Video.Media.EXTERNAL_CONTENT_URI,
            projection,
            "${MediaStore.Video.Media._ID} > ?",
            arrayOf(lastSeen.toString()),
            "${MediaStore.Video.Media._ID} ASC",
        )?.use { cursor ->
            val idColumn = cursor.getColumnIndexOrThrow(MediaStore.Video.Media._ID)
            val durationColumn = cursor.getColumnIndexOrThrow(MediaStore.Video.Media.DURATION)
            val pathColumn = cursor.getColumnIndexOrThrow(MediaStore.Video.Media.RELATIVE_PATH)

            while (cursor.moveToNext() && queued < MAX_PER_TRIGGER) {
                val id = cursor.getLong(idColumn)
                highestSeen = maxOf(highestSeen, id)

                val path = cursor.getString(pathColumn).orEmpty()
                // Never edit our own exports: that is how a folder fills up with
                // AutoCut_AutoCut_AutoCut_.
                if (path.startsWith(OutputStore.RELATIVE_PATH)) continue

                val durationMs = cursor.getLong(durationColumn)
                if (durationMs < MIN_DURATION_MS) continue

                AutoEditWorker.enqueue(
                    context,
                    ContentUris.withAppendedId(MediaStore.Video.Media.EXTERNAL_CONTENT_URI, id),
                )
                queued++
            }
        }

        settings.lastSeenVideoId = highestSeen
    }

    private companion object {
        const val TAG = "MediaWatchJobService"

        /** Anything shorter is a fragment, not a recording worth re-encoding. */
        const val MIN_DURATION_MS = 3_000L

        /**
         * A bulk import — restoring a backup, plugging in a camera — should not
         * turn into a hundred queued encodes. The rest are picked up on the next
         * trigger, a few at a time.
         */
        const val MAX_PER_TRIGGER = 4
    }
}
