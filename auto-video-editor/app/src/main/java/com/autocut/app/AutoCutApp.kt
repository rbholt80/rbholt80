package com.autocut.app

import android.app.Application
import com.autocut.app.data.Settings
import com.autocut.app.work.Notifications
import com.autocut.app.work.MediaWatchScheduler

class AutoCutApp : Application() {

    override fun onCreate() {
        super.onCreate()
        Notifications.createChannels(this)

        // The watch job is one-shot by design: the system cancels it once it
        // fires, so it has to be put back. Doing it at startup as well as after
        // each firing covers a reboot or an app update clearing the schedule.
        if (Settings(this).autoEditNewVideos) {
            MediaWatchScheduler.schedule(this)
        }
    }
}
