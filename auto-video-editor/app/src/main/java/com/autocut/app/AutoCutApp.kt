package com.autocut.app

import android.app.Application
import com.autocut.app.data.Settings
import com.autocut.app.work.Notifications
import com.autocut.app.work.MediaWatchScheduler

class AutoCutApp : Application() {

    override fun onCreate() {
        super.onCreate()
        Notifications.createChannels(this)

        // The watch job is one-shot by design: the system consumes it when it
        // fires, so it has to be put back. Only if it is not already armed —
        // this also runs in the process the system starts to service the job
        // itself, and rescheduling that id mid-flight would cancel it.
        if (Settings(this).autoEditNewVideos) {
            MediaWatchScheduler.scheduleIfAbsent(this)
        }
    }
}
