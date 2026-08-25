package com.autocut.app.ui

import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import androidx.core.content.ContextCompat
import androidx.recyclerview.widget.RecyclerView
import com.autocut.app.R
import com.autocut.app.databinding.ItemFixBinding
import com.autocut.engine.model.Fix
import com.autocut.engine.model.Severity
import java.util.Locale

/**
 * The list of decisions, each with the measurement behind it and a switch to
 * overrule it.
 */
class FixAdapter(
    private val onToggle: (id: String, enabled: Boolean) -> Unit,
) : RecyclerView.Adapter<FixAdapter.FixViewHolder>() {

    private var items: List<Fix> = emptyList()

    fun submit(fixes: List<Fix>) {
        items = fixes
        notifyDataSetChanged()
    }

    override fun getItemCount(): Int = items.size

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): FixViewHolder =
        FixViewHolder(ItemFixBinding.inflate(LayoutInflater.from(parent.context), parent, false))

    override fun onBindViewHolder(holder: FixViewHolder, position: Int) = holder.bind(items[position])

    inner class FixViewHolder(private val binding: ItemFixBinding) :
        RecyclerView.ViewHolder(binding.root) {

        fun bind(fix: Fix) {
            binding.title.text = fix.title
            binding.detail.text = fix.detail

            if (fix.savedUs > 0) {
                binding.saves.visibility = View.VISIBLE
                binding.saves.text = binding.root.context.getString(
                    R.string.edit_saves_time,
                    String.format(Locale.getDefault(), "%.1fs", fix.savedUs / 1e6),
                )
                binding.saves.setTextColor(
                    ContextCompat.getColor(binding.root.context, colorFor(fix.severity))
                )
            } else {
                binding.saves.visibility = View.GONE
            }

            // Detached before setting the state, so restoring the switch to what
            // the plan says never looks like the user flipping it.
            binding.toggle.setOnCheckedChangeListener(null)
            binding.toggle.isChecked = fix.enabled
            binding.toggle.setOnCheckedChangeListener { _, checked -> onToggle(fix.id, checked) }
            binding.root.setOnClickListener { binding.toggle.toggle() }
        }

        private fun colorFor(severity: Severity): Int = when (severity) {
            Severity.IMPORTANT -> R.color.severity_important
            Severity.SUGGESTED -> R.color.severity_suggested
            Severity.INFO -> R.color.severity_info
        }
    }
}
