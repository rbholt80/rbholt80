package com.autocut.app.work

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.autocut.app.data.Settings

/**
 * Puts the library watch back after a restart.
 *
 * A content-trigger job cannot be persisted — JobInfo rejects setPersisted on a
 * job with a content trigger — so the system drops it on reboot. Nothing else
 * starts this app's process afterwards, so without this the automatic mode was
 * silently dead from the reboot until the user next opened the app.
 */
class BootReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        if (!Settings(context).autoEditNewVideos) return
        MediaWatchScheduler.scheduleIfAbsent(context.applicationContext)
    }
}
