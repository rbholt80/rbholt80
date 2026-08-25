package com.autocut.app.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.IntentCompat
import com.autocut.app.R
import com.autocut.app.data.Settings
import com.autocut.app.databinding.ActivityMainBinding
import com.autocut.app.work.MediaWatchScheduler
import com.google.android.material.snackbar.Snackbar

class MainActivity : AppCompatActivity() {

    private lateinit var binding: ActivityMainBinding
    private lateinit var settings: Settings

    private val pickVideo = registerForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri != null) openEditor(uri)
    }

    private val requestAutoModePermissions =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            // Deliberately ignores the result map. It only carries the
            // permissions that were actually asked for, and the request below
            // filters out anything already granted — so on the very common path
            // of re-enabling auto mode with video access already held, the map
            // has no entry for it, which read as a refusal and switched the mode
            // straight back off. The system is the authority here, not the map.
            if (hasLibraryAccess()) {
                MediaWatchScheduler.enable(this)
            } else {
                binding.autoMode.isChecked = false
                Snackbar.make(binding.root, R.string.auto_mode_permission_denied, Snackbar.LENGTH_LONG)
                    .show()
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityMainBinding.inflate(layoutInflater)
        setContentView(binding.root)
        settings = Settings(this)
        binding.scroll.padForBottomSystemBar()

        binding.toolbar.setOnMenuItemClickListener { item ->
            if (item.itemId == R.id.action_settings) {
                startActivity(Intent(this, SettingsActivity::class.java))
                true
            } else {
                false
            }
        }

        binding.pickVideo.setOnClickListener {
            pickVideo.launch(
                PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.VideoOnly)
            )
        }

        syncAutoModeSwitch()

        // Only on a genuinely new launch. getIntent() still returns the original
        // ACTION_SEND intent after a configuration change, so without this guard
        // every rotation pushed the editor back on top and the user could never
        // get back to this screen.
        if (savedInstanceState == null) sharedVideo(intent)?.let(::openEditor)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        // Replaces the launch intent, so the same once-only rule applies to it.
        setIntent(intent)
        sharedVideo(intent)?.let(::openEditor)
    }

    override fun onResume() {
        super.onResume()
        syncAutoModeSwitch()
    }

    /**
     * Shows what the mode is actually doing, not what was last tapped.
     *
     * Video access can be taken away from system settings while the app is in
     * the background. The scheduled job survives that, so it would keep waking
     * up to a library it can no longer read, behind a switch still showing on.
     */
    private fun syncAutoModeSwitch() {
        if (settings.autoEditNewVideos && !hasLibraryAccess()) {
            MediaWatchScheduler.disable(this)
        }
        binding.autoMode.setOnCheckedChangeListener(null)
        binding.autoMode.isChecked = settings.autoEditNewVideos
        binding.autoMode.setOnCheckedChangeListener { _, checked ->
            if (checked) enableAutoMode() else MediaWatchScheduler.disable(this)
        }
    }

    private fun hasLibraryAccess(): Boolean =
        checkSelfPermission(libraryPermission()) == PackageManager.PERMISSION_GRANTED

    private fun enableAutoMode() {
        val needed = buildList {
            add(libraryPermission())
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                add(Manifest.permission.POST_NOTIFICATIONS)
            }
        }.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }

        if (needed.isEmpty()) {
            MediaWatchScheduler.enable(this)
        } else {
            Snackbar.make(binding.root, R.string.auto_mode_needs_permission, Snackbar.LENGTH_SHORT).show()
            requestAutoModePermissions.launch(needed.toTypedArray())
        }
    }

    private fun libraryPermission(): String =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            Manifest.permission.READ_MEDIA_VIDEO
        } else {
            Manifest.permission.READ_EXTERNAL_STORAGE
        }

    private fun sharedVideo(intent: Intent?): Uri? {
        if (intent?.action != Intent.ACTION_SEND) return null
        if (intent.type?.startsWith("video/") != true) return null
        return IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)
    }

    private fun openEditor(uri: Uri) {
        startActivity(
            Intent(this, EditActivity::class.java)
                .setData(uri)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        )
    }
}
