package com.autocut.app.ui

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.view.View
import androidx.activity.viewModels
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import androidx.recyclerview.widget.LinearLayoutManager
import com.autocut.app.R
import com.autocut.app.databinding.ActivityEditBinding
import com.autocut.engine.model.EditPlan
import com.autocut.engine.model.EditStyle
import com.google.android.material.chip.Chip
import com.google.android.material.snackbar.Snackbar
import kotlinx.coroutines.launch

/**
 * One video's edit: what the app decided, and every lever to disagree with it.
 */
class EditActivity : AppCompatActivity() {

    private lateinit var binding: ActivityEditBinding
    private val viewModel: EditViewModel by viewModels()
    private lateinit var fixAdapter: FixAdapter
    private var savedUri: Uri? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityEditBinding.inflate(layoutInflater)
        setContentView(binding.root)

        binding.toolbar.setNavigationOnClickListener { finish() }

        fixAdapter = FixAdapter { id, enabled -> viewModel.setFix(id, enabled) }
        binding.fixes.layoutManager = LinearLayoutManager(this)
        binding.fixes.adapter = fixAdapter

        buildStyleChips()
        binding.reset.setOnClickListener { viewModel.resetOverrides() }
        binding.save.setOnClickListener {
            val saved = savedUri
            if (saved != null) share(saved) else viewModel.export()
        }
        binding.play.setOnClickListener { savedUri?.let(::play) }

        val source = intent.data
        if (source == null) {
            finish()
            return
        }
        viewModel.open(source)

        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                viewModel.state.collect(::render)
            }
        }
    }

    private fun buildStyleChips() {
        for (style in EditStyle.entries) {
            val chip = Chip(this).apply {
                text = style.label
                isCheckable = true
                tag = style
            }
            binding.styleGroup.addView(chip)
        }
        selectStyleChip(viewModel.style)
        binding.styleGroup.setOnCheckedStateChangeListener { group, ids ->
            val chip = ids.firstOrNull()?.let { group.findViewById<Chip>(it) } ?: return@setOnCheckedStateChangeListener
            viewModel.setStyle(chip.tag as EditStyle)
        }
    }

    private fun selectStyleChip(style: EditStyle) {
        for (index in 0 until binding.styleGroup.childCount) {
            val chip = binding.styleGroup.getChildAt(index) as Chip
            if (chip.tag == style) chip.isChecked = true
        }
    }

    private fun render(state: EditViewModel.State) = when (state) {
        is EditViewModel.State.Idle -> showBusy(getString(R.string.edit_analyzing), null)

        is EditViewModel.State.Analyzing ->
            showBusy(getString(R.string.edit_analyzing_percent, state.percent), state.percent)

        is EditViewModel.State.Ready -> {
            savedUri = null
            hideBusy()
            showPlan(state.plan)
            binding.save.setText(R.string.action_save)
            binding.save.isEnabled = state.plan.changesAnything
            binding.play.visibility = View.GONE
            binding.reset.isEnabled = viewModel.hasOverrides
        }

        is EditViewModel.State.Exporting -> {
            showBusy(getString(R.string.edit_exporting, state.percent), state.percent)
            binding.save.isEnabled = false
            binding.reset.isEnabled = false
        }

        is EditViewModel.State.Saved -> {
            savedUri = state.uri
            hideBusy()
            showPlan(state.plan)
            binding.save.setText(R.string.action_share)
            binding.save.isEnabled = true
            binding.play.visibility = View.VISIBLE
            binding.reset.isEnabled = false
            Snackbar.make(binding.root, R.string.edit_saved, Snackbar.LENGTH_LONG).show()
        }

        is EditViewModel.State.Failed -> {
            hideBusy()
            binding.save.isEnabled = false
            Snackbar.make(
                binding.root,
                getString(R.string.edit_failed, state.message),
                Snackbar.LENGTH_INDEFINITE,
            ).setAction(android.R.string.ok) { finish() }.show()
        }
    }

    private fun showPlan(plan: EditPlan) {
        binding.content.visibility = View.VISIBLE
        binding.summary.text = plan.summary()
        binding.sourceName.text = viewModel.sourceName.orEmpty()
        selectStyleChip(plan.style)

        fixAdapter.submit(plan.fixes)
        val hasFixes = plan.fixes.isNotEmpty()
        binding.fixes.visibility = if (hasFixes) View.VISIBLE else View.GONE
        binding.fixesLabel.text =
            if (hasFixes) getString(R.string.edit_fixes_label) else getString(R.string.edit_no_fixes)

        val notes = plan.notes.joinToString("\n\n") { "• ${it.text}" }
        binding.notes.text = notes
        val hasNotes = notes.isNotBlank()
        binding.notes.visibility = if (hasNotes) View.VISIBLE else View.GONE
        binding.notesLabel.visibility = if (hasNotes) View.VISIBLE else View.GONE
    }

    private fun showBusy(label: String, percent: Int?) {
        binding.busy.visibility = View.VISIBLE
        binding.busyLabel.text = label
        if (percent == null) {
            binding.busyProgress.isIndeterminate = true
        } else {
            binding.busyProgress.isIndeterminate = false
            binding.busyProgress.setProgressCompat(percent, true)
        }
    }

    private fun hideBusy() {
        binding.busy.visibility = View.GONE
    }

    private fun play(uri: Uri) {
        startActivity(
            Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, "video/mp4")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        )
    }

    private fun share(uri: Uri) {
        startActivity(
            Intent.createChooser(
                Intent(Intent.ACTION_SEND)
                    .setType("video/mp4")
                    .putExtra(Intent.EXTRA_STREAM, uri)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION),
                getString(R.string.action_share),
            )
        )
    }
}
