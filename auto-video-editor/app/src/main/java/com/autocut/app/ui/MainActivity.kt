package com.autocut.app.ui

import android.Manifest
import android.content.Intent
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
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { granted ->
            if (granted[libraryPermission()] == true) {
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

        binding.autoMode.setOnCheckedChangeListener { _, checked ->
            if (checked) enableAutoMode() else MediaWatchScheduler.disable(this)
        }

        sharedVideo(intent)?.let(::openEditor)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        sharedVideo(intent)?.let(::openEditor)
    }

    override fun onResume() {
        super.onResume()
        // Reflects the real state rather than the last thing tapped: the mode
        // turns itself off if library access is withdrawn.
        binding.autoMode.setOnCheckedChangeListener(null)
        binding.autoMode.isChecked = settings.autoEditNewVideos
        binding.autoMode.setOnCheckedChangeListener { _, checked ->
            if (checked) enableAutoMode() else MediaWatchScheduler.disable(this)
        }
    }

    private fun enableAutoMode() {
        val needed = buildList {
            add(libraryPermission())
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                add(Manifest.permission.POST_NOTIFICATIONS)
            }
        }.filter { checkSelfPermission(it) != android.content.pm.PackageManager.PERMISSION_GRANTED }

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
