package com.autocut.app.work

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.core.app.NotificationCompat
import androidx.core.content.getSystemService
import com.autocut.app.R

object Notifications {

    const val CHANNEL_PROGRESS = "autocut.progress"
    const val CHANNEL_RESULTS = "autocut.results"

    fun createChannels(context: Context) {
        val manager = context.getSystemService<NotificationManager>() ?: return
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_PROGRESS,
                context.getString(R.string.channel_progress),
                // Silent: an edit running in the background should be visible,
                // not interrupting.
                NotificationManager.IMPORTANCE_LOW,
            ).apply { description = context.getString(R.string.channel_progress_description) }
        )
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_RESULTS,
                context.getString(R.string.channel_results),
                NotificationManager.IMPORTANCE_DEFAULT,
            ).apply { description = context.getString(R.string.channel_results_description) }
        )
    }

    fun progress(context: Context, title: String, text: String, percent: Int?): Notification =
        NotificationCompat.Builder(context, CHANNEL_PROGRESS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .apply {
                if (percent == null) setProgress(0, 0, true) else setProgress(100, percent, false)
            }
            .build()

    fun result(context: Context, title: String, text: String, video: Uri?): Notification {
        val builder = NotificationCompat.Builder(context, CHANNEL_RESULTS)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setAutoCancel(true)

        if (video != null) {
            val view = Intent(Intent.ACTION_VIEW)
                .setDataAndType(video, "video/mp4")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            builder.setContentIntent(
                PendingIntent.getActivity(
                    context,
                    video.hashCode(),
                    view,
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
            )
        }
        return builder.build()
    }

    fun cancel(context: Context, id: Int) {
        val manager = context.getSystemService<NotificationManager>() ?: return
        runCatching { manager.cancel(id) }
    }

    fun notify(context: Context, id: Int, notification: Notification) {
        val manager = context.getSystemService<NotificationManager>() ?: return
        // Posting without permission on Android 13+ is a no-op rather than a
        // crash, so there is nothing to guard here beyond the null manager.
        runCatching { manager.notify(id, notification) }
    }
}
