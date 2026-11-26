package cpasfait.app

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import cpasfait.core.CpasfaitException
import cpasfait.core.RowView
import cpasfait.core.SessionView
import cpasfait.core.SyncSummary
import cpasfait.core.TabView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.time.LocalTime
import java.time.format.DateTimeFormatter

sealed interface Screen {
    data object Tasks : Screen
    data class Task(val id: String) : Screen
    data object Settings : Screen
    data object Search : Screen
    data object Time : Screen
}

/** A message for the user, with an action (Undo...). */
data class Notice(val text: String, val action: String? = null, val onAction: (() -> Unit)? = null)

/** The name the core gives when there is no `$USER` (as on Android). */
private const val DEFAULT_USER = "me"

/** What the screens show (process-wide: survives rotations). */
class Model(val app: CpasfaitApp) {
    val core get() = app.core
    val keys by lazy { SshKeys(app.coreDir) }

    var tabs by mutableStateOf(listOf<TabView>())
        private set
    var tabId by mutableStateOf<String?>(app.getSharedPreferences("ui", android.content.Context.MODE_PRIVATE).getString("tab", null))
        private set
    var rows by mutableStateOf(listOf<RowView>())
        private set
    private val prefs = app.getSharedPreferences("ui", android.content.Context.MODE_PRIVATE)

    /** Folded tasks (all tabs), remembered across restarts. */
    var collapsed by mutableStateOf(prefs.getStringSet("collapsed", emptySet())!!.toSet())
        private set
    var hideDone by mutableStateOf(prefs.getBoolean("hideDone", true))
        private set
    var active by mutableStateOf<SessionView?>(null)
        private set
    var screen by mutableStateOf<Screen>(Screen.Tasks)
    var notice by mutableStateOf<Notice?>(null)

    /** A plain message (errors, results). */
    var message: String?
        get() = notice?.text
        set(v) {
            notice = v?.let { Notice(it) }
        }
    /** No name set yet (first start). */
    var askName by mutableStateOf(runCatching { app.core.user() == DEFAULT_USER }.getOrDefault(false))
    var syncing by mutableStateOf(false)
        private set
    var syncStatus by mutableStateOf<String?>(null)
        private set
    /** Bumped on each reload, for screens showing other data (task detail). */
    var generation by mutableStateOf(0)
        private set

    private var dataVersion = -1L
    private val syncLock = Mutex()

    val tab: TabView? get() = tabs.firstOrNull { it.id == tabId }

    fun reload() {
        try {
            tabs = core.tabs()
            if (tabs.none { it.id == tabId }) tabId = tabs.firstOrNull()?.id
            rows = tabId?.let { core.rows(it, hideDone) } ?: emptyList()
            active = core.activeSession()
            dataVersion = core.dataVersion()
            generation++
        } catch (e: Exception) {
            report(e)
        }
    }

    /** Reload if the sync connection changed something. */
    fun reloadIfChanged() {
        val v = runCatching { core.dataVersion() }.getOrNull() ?: return
        if (v != dataVersion) reload()
    }

    fun selectTab(id: String) {
        tabId = id
        prefs.edit().putString("tab", id).apply()
        reload()
    }

    fun toggleHideDone() {
        hideDone = !hideDone
        prefs.edit().putBoolean("hideDone", hideDone).apply()
        reload()
    }

    fun toggleFolded(id: String) {
        collapsed = if (id in collapsed) collapsed - id else collapsed + id
        prefs.edit().putStringSet("collapsed", collapsed).apply()
    }

    /** Show `task` (in its tab, unfolded). */
    fun open(task: String) {
        val tab = runCatching { core.task(task).tab }.getOrNull() ?: return
        if (tab != tabId) selectTab(tab)
        screen = Screen.Task(task)
    }

    /** Delete `task`, offering to undo it. */
    fun delete(task: String, title: String) {
        if (act { core.deleteTask(task) }) {
            notice = Notice("deleted \"$title\"", "Undo") { act { core.undeleteTask(task) } }
        }
    }

    fun report(e: Throwable) {
        message = when (e) {
            is CpasfaitException -> e.message
            else -> e.message ?: e.javaClass.simpleName
        }
        CpasfaitApp.log("error", e)
    }

    /** Run an edit, then reload and schedule a sync. False on error (shown). */
    fun act(f: () -> Unit): Boolean =
        try {
            f()
            reload()
            app.edited()
            true
        } catch (e: Exception) {
            report(e)
            false
        }

    /** Sync all shared tabs (one at a time). `explicit`: say how it went. */
    suspend fun sync(explicit: Boolean): SyncSummary? {
        if (explicit) syncing = true
        val res = try {
            syncLock.withLock {
                withContext(Dispatchers.IO) {
                    SftpProvider(keys).use { app.syncCore.sync(it) }
                }
            }
        } catch (e: Exception) {
            withContext(Dispatchers.Main) {
                syncing = false
                syncStatus = "sync failed"
                report(e)
            }
            return null
        }
        withContext(Dispatchers.Main) {
            syncing = false
            val now = LocalTime.now().format(DateTimeFormatter.ofPattern("HH:mm"))
            syncStatus = if (res.errors.isEmpty()) "synced $now" else "sync failed"
            when {
                res.errors.isNotEmpty() -> message = res.errors.joinToString("\n")
                res.waitingForKey > 0u -> message = "some changes use a key this phone does not have yet (not invited?)"
                explicit -> message = "synced: ${res.pulled} received, ${res.pushed} sent"
            }
            reloadIfChanged()
        }
        return res
    }
}
