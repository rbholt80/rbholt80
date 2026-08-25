package com.autocut.app.data

import android.content.Context
import androidx.core.content.edit
import com.autocut.engine.model.EditPreferences
import com.autocut.engine.model.EditStyle

/**
 * The handful of choices that outlive a single edit.
 *
 * Per-video decisions are not stored here. Those live in the plan, which is
 * recomputed from the file every time, so a preference can never quietly apply
 * a stale judgement to a different video.
 */
class Settings(context: Context) {

    private val prefs = context.applicationContext
        .getSharedPreferences("autocut", Context.MODE_PRIVATE)

    var style: EditStyle
        get() = EditStyle.fromName(prefs.getString(KEY_STYLE, null))
        set(value) = prefs.edit { putString(KEY_STYLE, value.name) }

    /** When on, new recordings are edited in the background without being asked. */
    var autoEditNewVideos: Boolean
        get() = prefs.getBoolean(KEY_AUTO_EDIT, false)
        set(value) = prefs.edit { putBoolean(KEY_AUTO_EDIT, value) }

    var allowStabilization: Boolean
        get() = prefs.getBoolean(KEY_STABILIZE, true)
        set(value) = prefs.edit { putBoolean(KEY_STABILIZE, value) }

    var targetLoudnessDb: Float
        get() = prefs.getFloat(KEY_LOUDNESS, DEFAULT_LOUDNESS_DB)
        set(value) = prefs.edit { putFloat(KEY_LOUDNESS, value.coerceIn(-24f, -9f)) }

    /**
     * Highest MediaStore id already considered by the automatic mode.
     *
     * Stored rather than derived from timestamps because a device's clock can go
     * backwards and ids do not, and re-editing a video the user already has is
     * worse than missing one.
     */
    var lastSeenVideoId: Long
        get() = prefs.getLong(KEY_LAST_VIDEO_ID, 0L)
        set(value) = prefs.edit { putLong(KEY_LAST_VIDEO_ID, value) }

    fun editPreferences(): EditPreferences = EditPreferences(
        style = style,
        allowStabilization = allowStabilization,
        targetLoudnessDb = targetLoudnessDb,
    )

    private companion object {
        const val KEY_STYLE = "style"
        const val KEY_AUTO_EDIT = "auto_edit"
        const val KEY_STABILIZE = "stabilize"
        const val KEY_LOUDNESS = "loudness_db"
        const val KEY_LAST_VIDEO_ID = "last_video_id"
        const val DEFAULT_LOUDNESS_DB = -16f
    }
}
