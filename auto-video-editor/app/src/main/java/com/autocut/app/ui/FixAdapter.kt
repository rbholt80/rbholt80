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
    private var interactive: Boolean = true

    init {
        // Rows are identified by the fix they show, so RecyclerView can match a
        // holder to its item across an update instead of putting every view
        // through the recycler.
        setHasStableIds(true)
    }

    /**
     * Every toggle re-plans, so this runs on each tap.
     *
     * Rebuilding the whole list each time recycled the row the user had just
     * touched: the switch came back from the pool detached and unlaid-out, which
     * cancels its thumb animation and makes the tap look like it did not land.
     * When only the contents changed — the ordinary case, since the plan keeps
     * the same fixes in the same order — only the rows that actually differ are
     * rebound.
     */
    fun submit(fixes: List<Fix>) {
        val previous = items
        items = fixes
        if (previous.size != fixes.size || previous.indices.any { previous[it].id != fixes[it].id }) {
            notifyDataSetChanged()
            return
        }
        for (index in fixes.indices) {
            if (previous[index] != fixes[index]) notifyItemChanged(index)
        }
    }

    /** Locks the rows while an export is running. */
    fun setInteractive(value: Boolean) {
        if (interactive == value) return
        interactive = value
        notifyDataSetChanged()
    }

    override fun getItemCount(): Int = items.size

    override fun getItemId(position: Int): Long = items[position].id.hashCode().toLong()

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
            binding.toggle.isEnabled = interactive
            binding.toggle.setOnCheckedChangeListener { _, checked -> onToggle(fix.id, checked) }
            binding.root.isEnabled = interactive
            binding.root.alpha = if (interactive) 1f else 0.5f
            binding.root.setOnClickListener(
                if (interactive) View.OnClickListener { binding.toggle.toggle() } else null
            )
        }

        private fun colorFor(severity: Severity): Int = when (severity) {
            Severity.IMPORTANT -> R.color.severity_important
            Severity.SUGGESTED -> R.color.severity_suggested
            Severity.INFO -> R.color.severity_info
        }
    }
}
