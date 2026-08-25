package com.autocut.app.ui

import android.os.Bundle
import androidx.appcompat.app.AppCompatActivity
import com.autocut.app.R
import com.autocut.app.data.Settings
import com.autocut.app.databinding.ActivitySettingsBinding
import com.autocut.engine.model.EditStyle
import com.google.android.material.chip.Chip

class SettingsActivity : AppCompatActivity() {

    private lateinit var binding: ActivitySettingsBinding
    private lateinit var settings: Settings

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivitySettingsBinding.inflate(layoutInflater)
        setContentView(binding.root)
        settings = Settings(this)

        binding.toolbar.setNavigationOnClickListener { finish() }

        for (style in EditStyle.entries) {
            binding.styleGroup.addView(
                Chip(this).apply {
                    text = style.label
                    isCheckable = true
                    tag = style
                    isChecked = style == settings.style
                }
            )
        }
        binding.styleBlurb.text = settings.style.blurb
        binding.styleGroup.setOnCheckedStateChangeListener { group, ids ->
            val chip = ids.firstOrNull()?.let { group.findViewById<Chip>(it) }
                ?: return@setOnCheckedStateChangeListener
            val style = chip.tag as EditStyle
            settings.style = style
            binding.styleBlurb.text = style.blurb
        }

        binding.stabilize.isChecked = settings.allowStabilization
        binding.stabilize.setOnCheckedChangeListener { _, checked ->
            settings.allowStabilization = checked
        }

        binding.loudness.value = settings.targetLoudnessDb.coerceIn(-24f, -9f)
        showLoudness(binding.loudness.value)
        binding.loudness.addOnChangeListener { _, value, _ ->
            settings.targetLoudnessDb = value
            showLoudness(value)
        }
    }

    private fun showLoudness(value: Float) {
        binding.loudnessValue.text = getString(R.string.settings_loudness_value, value)
    }
}
