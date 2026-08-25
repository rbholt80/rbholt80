package com.autocut.app.ui

import android.app.Application
import android.net.Uri
import android.provider.OpenableColumns
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.autocut.app.data.OutputStore
import com.autocut.app.data.Settings
import com.autocut.app.media.AutoCutRenderer
import com.autocut.app.media.MediaReadException
import com.autocut.app.media.SignalExtractor
import com.autocut.engine.analysis.Analysis
import com.autocut.engine.model.EditPlan
import com.autocut.engine.model.EditPreferences
import com.autocut.engine.model.EditStyle
import com.autocut.engine.plan.EditPlanner
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.io.File

/**
 * Holds one video's analysis and the plan currently derived from it.
 *
 * The expensive step — decoding the file — happens once. Every style change and
 * every toggle after that is a re-plan from the same measurements, so the UI can
 * respond immediately and the user never waits to see what their change did.
 */
@androidx.annotation.OptIn(markerClass = [androidx.media3.common.util.UnstableApi::class])
class EditViewModel(application: Application) : AndroidViewModel(application) {

    sealed interface State {
        data object Idle : State
        data class Analyzing(val percent: Int) : State
        data class Ready(val plan: EditPlan) : State
        data class Exporting(val percent: Int, val plan: EditPlan) : State
        data class Saved(val uri: Uri, val plan: EditPlan) : State
        data class Failed(val message: String) : State
    }

    private val _state = MutableStateFlow<State>(State.Idle)
    val state: StateFlow<State> = _state.asStateFlow()

    private val settings = Settings(application)
    private var analysis: Analysis? = null
    private var preferences: EditPreferences = settings.editPreferences()
    private var source: Uri? = null
    private var running: Job? = null

    var sourceName: String? = null
        private set

    val style: EditStyle get() = preferences.style

    /** True once the user has overruled at least one of the app's decisions. */
    val hasOverrides: Boolean get() = preferences.overrides.isNotEmpty()

    fun open(uri: Uri) {
        if (uri == source && analysis != null) return
        source = uri
        sourceName = displayName(uri)
        analyze(uri)
    }

    private fun analyze(uri: Uri) {
        running?.cancel()
        _state.value = State.Analyzing(0)
        running = viewModelScope.launch {
            try {
                val result = SignalExtractor(getApplication()).analyze(uri) { fraction ->
                    _state.value = State.Analyzing((fraction * 100).toInt())
                }
                analysis = result
                _state.value = State.Ready(EditPlanner.plan(result, preferences))
            } catch (e: MediaReadException) {
                _state.value = State.Failed(e.message ?: FALLBACK_ERROR)
            } catch (e: Exception) {
                _state.value = State.Failed(e.message ?: FALLBACK_ERROR)
            }
        }
    }

    fun setStyle(style: EditStyle) {
        if (style == preferences.style) return
        // A style is a different set of thresholds, so the user's per-fix
        // overrides were answers to a different question. Starting clean is
        // less surprising than silently keeping them.
        preferences = EditPreferences(
            style = style,
            allowStabilization = preferences.allowStabilization,
            targetLoudnessDb = preferences.targetLoudnessDb,
        )
        replan()
    }

    fun setFix(id: String, enabled: Boolean) {
        preferences = preferences.withFix(id, enabled)
        replan()
    }

    fun resetOverrides() {
        preferences = preferences.reset()
        replan()
    }

    private fun replan() {
        val current = analysis ?: return
        _state.value = State.Ready(EditPlanner.plan(current, preferences))
    }

    fun export() {
        val uri = source ?: return
        val plan = (_state.value as? State.Ready)?.plan ?: return
        running?.cancel()
        _state.value = State.Exporting(0, plan)

        running = viewModelScope.launch {
            val context = getApplication<Application>()
            val scratch = File(context.cacheDir, "autocut_${System.currentTimeMillis()}.mp4")
            try {
                AutoCutRenderer(context).render(uri, plan, scratch) { percent ->
                    _state.value = State.Exporting(percent, plan)
                }
                val saved = OutputStore.publish(
                    context,
                    scratch,
                    OutputStore.nameFor(sourceName, System.currentTimeMillis()),
                )
                _state.value = State.Saved(saved, plan)
            } catch (e: Exception) {
                _state.value = State.Failed(e.message ?: FALLBACK_ERROR)
            } finally {
                scratch.delete()
            }
        }
    }

    private fun displayName(uri: Uri): String? = runCatching {
        getApplication<Application>().contentResolver
            .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { if (it.moveToFirst()) it.getString(0) else null }
    }.getOrNull()

    private companion object {
        const val FALLBACK_ERROR = "Something went wrong with that video."
    }
}
