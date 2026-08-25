package com.autocut.app.data

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.provider.MediaStore
import java.io.File
import java.io.IOException

/**
 * Publishes a finished export into the device's video library.
 *
 * The encoder writes to a plain file in the cache because that is all it can do;
 * this moves the result somewhere the gallery will show it. Everything lands in
 * `Movies/AutoCut`, which also gives the automatic mode a way to recognise its
 * own output and not edit it again.
 */
object OutputStore {

    const val RELATIVE_PATH = "Movies/AutoCut"

    /**
     * Copies [file] into the media library under [displayName] and deletes it.
     *
     * @throws IOException if the library entry cannot be created or written
     */
    fun publish(context: Context, file: File, displayName: String): Uri {
        val resolver = context.contentResolver
        val collection = MediaStore.Video.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)

        val values = ContentValues().apply {
            put(MediaStore.Video.Media.DISPLAY_NAME, displayName)
            put(MediaStore.Video.Media.MIME_TYPE, "video/mp4")
            put(MediaStore.Video.Media.RELATIVE_PATH, RELATIVE_PATH)
            // Hidden from the gallery until the bytes are actually there, so a
            // failed export never leaves a broken thumbnail behind.
            put(MediaStore.Video.Media.IS_PENDING, 1)
        }

        val uri = resolver.insert(collection, values)
            ?: throw IOException("The video library would not accept a new file.")

        try {
            resolver.openOutputStream(uri)?.use { output ->
                file.inputStream().use { input -> input.copyTo(output) }
            } ?: throw IOException("The new library entry could not be opened for writing.")
        } catch (e: Exception) {
            resolver.delete(uri, null, null)
            throw if (e is IOException) e else IOException("The edited video could not be saved.", e)
        }

        resolver.update(
            uri,
            ContentValues().apply { put(MediaStore.Video.Media.IS_PENDING, 0) },
            null,
            null,
        )
        file.delete()
        return uri
    }

    /** A name that says where it came from and keeps exports of one source apart. */
    fun nameFor(sourceName: String?, timestampMs: Long): String {
        val base = sourceName
            ?.substringBeforeLast('.')
            ?.take(48)
            ?.ifBlank { null }
            ?: "video"
        return "AutoCut_${base}_$timestampMs.mp4"
    }

    /** True when [uri] is something this app produced, so it is not edited again. */
    fun isOwnOutput(context: Context, uri: Uri): Boolean {
        val projection = arrayOf(MediaStore.Video.Media.RELATIVE_PATH)
        return runCatching {
            context.contentResolver.query(uri, projection, null, null, null)?.use { cursor ->
                if (!cursor.moveToFirst()) return@use false
                cursor.getString(0)?.startsWith(RELATIVE_PATH) == true
            } ?: false
        }.getOrDefault(false)
    }
}
