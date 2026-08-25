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

    private fun highestVideoId(context: Context): Long {
        val projection = arrayOf(MediaStore.Video.Media._ID)
        return runCatching {
            context.contentResolver.query(
                MediaStore.Video.Media.EXTERNAL_CONTENT_URI,
                projection,
                null,
                null,
                "${MediaStore.Video.Media._ID} DESC LIMIT 1",
            )?.use { if (it.moveToFirst()) it.getLong(0) else 0L } ?: 0L
        }.getOrDefault(0L)
    }
}
