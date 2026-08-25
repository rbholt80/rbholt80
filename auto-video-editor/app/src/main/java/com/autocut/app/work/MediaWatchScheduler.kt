package com.autocut.app.work

import android.app.job.JobInfo
import android.app.job.JobScheduler
import android.content.ComponentName
import android.content.Context
import android.provider.MediaStore
import androidx.core.content.getSystemService
import com.autocut.app.data.Settings

/**
 * Keeps the system watching the video library on the app's behalf.
 *
 * A content-trigger job wakes the app when something is added to the library,
 * which is what lets a recording be edited without the app being open — and
 * without polling, which would cost battery for nothing most of the day.
 */
object MediaWatchScheduler {

    private const val JOB_ID = 1001

    /** Wait this long after a change, so a still-recording file is finished. */
    private const val UPDATE_DELAY_MS = 15_000L

    /** ...but never sit on a change longer than this. */
    private const val MAX_DELAY_MS = 120_000L

    /**
     * Turns the automatic mode on from now, not retroactively.
     *
     * The current highest library id is recorded first, so switching this on
     * does not queue an edit of every video already on the phone.
     */
    fun enable(context: Context) {
        val settings = Settings(context)
        settings.lastSeenVideoId = highestVideoId(context)
        settings.autoEditNewVideos = true
        schedule(context)
    }

    fun disable(context: Context) {
        Settings(context).autoEditNewVideos = false
        context.getSystemService<JobScheduler>()?.cancel(JOB_ID)
    }

    /**
     * Arms the trigger. Safe to call repeatedly — a content trigger fires once
     * and is then dropped by the system, so it has to be put back after every
     * firing and after every reboot.
     */
    /**
     * Arms the trigger only if it is not already armed or running.
     *
     * Calling [schedule] for an id that is currently executing makes
     * JobScheduler tear down that execution — which, from inside the job's own
     * service, means cancelling the scan it was woken up to perform.
     */
    fun scheduleIfAbsent(context: Context) {
        val scheduler = context.getSystemService<JobScheduler>() ?: return
        if (scheduler.getPendingJob(JOB_ID) != null) return
        schedule(context)
    }

    fun schedule(context: Context) {
        val scheduler = context.getSystemService<JobScheduler>() ?: return
        val job = JobInfo.Builder(JOB_ID, ComponentName(context, MediaWatchJobService::class.java))
            .addTriggerContentUri(
                JobInfo.TriggerContentUri(
                    MediaStore.Video.Media.EXTERNAL_CONTENT_URI,
                    JobInfo.TriggerContentUri.FLAG_NOTIFY_FOR_DESCENDANTS,
                )
            )
            .setTriggerContentUpdateDelay(UPDATE_DELAY_MS)
            .setTriggerContentMaxDelay(MAX_DELAY_MS)
            .build()
        runCatching { scheduler.schedule(job) }
    }

    /**
     * The highest id currently in the library, or 0 if it cannot be read.
     *
     * Deliberately no "LIMIT 1" on the sort order. That trick is rejected by
     * MediaProvider from Android 11 onward, and the failure here is not loud:
     * the exception would be swallowed, this would return 0, and the automatic
     * mode would then treat every video ever taken as new and start re-editing
     * the whole library four at a time. Reading the first row of a descending
     * cursor works on every version.
     */
    private fun highestVideoId(context: Context): Long {
        val projection = arrayOf(MediaStore.Video.Media._ID)
        return runCatching {
            context.contentResolver.query(
                MediaStore.Video.Media.EXTERNAL_CONTENT_URI,
                projection,
                null,
                null,
                "${MediaStore.Video.Media._ID} DESC",
            )?.use { if (it.moveToFirst()) it.getLong(0) else 0L } ?: 0L
        }.getOrDefault(0L)
    }
}
