package cpasfait.app

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import cpasfait.core.DepStateView
import cpasfait.core.DepView
import cpasfait.core.PlaceArg
import cpasfait.core.SessionView
import cpasfait.core.TaskDetail
import java.time.Instant
import java.time.LocalDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter

private val TIME = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm").withZone(ZoneId.systemDefault())

fun fmtTime(ms: Long?): String = ms?.let { TIME.format(Instant.ofEpochMilli(it)) } ?: "?"

/** `yyyy-MM-dd HH:mm` (or with seconds) in local time → Unix ms. */
fun parseTime(s: String): Long? {
    val t = s.trim().replace('T', ' ')
    for (p in listOf("yyyy-MM-dd HH:mm", "yyyy-MM-dd HH:mm:ss")) {
        runCatching {
            return LocalDateTime.parse(t, DateTimeFormatter.ofPattern(p))
                .atZone(ZoneId.systemDefault()).toInstant().toEpochMilli()
        }
    }
    return null
}

/** What the task screen is asking for. */
private sealed interface Ask {
    data object Subtask : Ask
    data object Parent : Ask
    data object Requires : Ask
    data object Tab : Ask
    data object NewSession : Ask
    data class Session(val s: SessionView) : Ask
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TaskScreen(model: Model, id: String) {
    val snackbar = remember { SnackbarHostState() }
    MessageSnackbar(model, snackbar)
    // Reloaded with the model (edits here, sync).
    val detail: TaskDetail? = remember(id, model.generation) {
        runCatching { model.core.task(id) }.getOrNull()
    }
    if (detail == null) {
        // Deleted (here or by a sync): back to the list.
        LaunchedEffect(id) { model.screen = Screen.Tasks }
        return
    }
    var title by remember(id) { mutableStateOf(detail.title) }
    var notes by remember(id) { mutableStateOf(detail.notes) }
    var due by remember(id, detail.due) { mutableStateOf(detail.due ?: "") }
    var tags by remember(id, detail.tags) { mutableStateOf(detail.tags.joinToString(" ")) }
    var ask by remember { mutableStateOf<Ask?>(null) }
    val focus = LocalFocusManager.current

    fun save() {
        val d = runCatching { model.core.task(id) }.getOrNull() ?: return
        if (title.isNotBlank() && title.trim() != d.title) model.act { model.core.setTitle(id, title.trim()) }
        if (notes != d.notes) model.act { model.core.setNotes(id, notes) }
    }
    // Leaving the screen (back, another screen) keeps the edits.
    DisposableEffect(id) { onDispose { save() } }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Task") },
                navigationIcon = {
                    IconButton(onClick = { model.screen = Screen.Tasks }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back")
                    }
                },
                actions = {
                    var menu by remember { mutableStateOf(false) }
                    IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, "Task menu") }
                    DropdownMenu(menu, onDismissRequest = { menu = false }) {
                        val item: @Composable (String, () -> Unit) -> Unit = { label, f ->
                            DropdownMenuItem(text = { Text(label) }, onClick = { menu = false; f() })
                        }
                        item("Indent (under the task above)") {
                            model.act { if (!model.core.indent(id)) model.message = "no task above to go under" }
                        }
                        item("Outdent") {
                            model.act { if (!model.core.outdent(id)) model.message = "already at the top" }
                        }
                        item("Move under…") { ask = Ask.Parent }
                        if (detail.parent != null) {
                            item("Move to the top level") {
                                model.act { model.core.moveTask(id, PlaceArg.LastChildOf(null)) }
                            }
                        }
                        item("Move to another tab…") { ask = Ask.Tab }
                        item("Requires…") { ask = Ask.Requires }
                        HorizontalDivider()
                        item("Delete") {
                            save()
                            model.screen = Screen.Tasks
                            model.delete(id, detail.title)
                        }
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { pad ->
        Column(
            Modifier.padding(pad).fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Checkbox(detail.done, { done -> model.act { model.core.setDone(id, done) } })
                OutlinedTextField(title, { title = it }, Modifier.fillMaxWidth(), label = { Text("Title") })
            }
            OutlinedTextField(
                notes, { notes = it }, Modifier.fillMaxWidth(), label = { Text("Notes") }, minLines = 3,
            )

            Text("Priority", style = MaterialTheme.typography.labelLarge)
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                for (p in -3L..3L) {
                    FilterChip(
                        selected = detail.priority == p,
                        onClick = { model.act { model.core.setPriority(id, p) } },
                        label = { Text(if (p > 0) "+$p" else "$p") },
                    )
                }
            }

            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    due, { due = it }, Modifier.weight(1f),
                    label = { Text("Due") },
                    placeholder = { Text("fri 9:00, 17:30, 2026-12-01") },
                    singleLine = true,
                )
                TextButton(onClick = {
                    focus.clearFocus()
                    model.act { model.core.setDue(id, due.ifBlank { null }) }
                }) { Text("Set") }
                if (detail.due != null) {
                    TextButton(onClick = { model.act { model.core.setDue(id, null) } }) { Text("Clear") }
                }
            }

            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    tags, { tags = it }, Modifier.weight(1f), label = { Text("Tags") }, singleLine = true,
                )
                TextButton(onClick = {
                    focus.clearFocus()
                    val want = tags.split(' ', ',').map { it.trim().removePrefix("#") }.filter { it.isNotEmpty() }.toSet()
                    model.act {
                        for (t in detail.tags) if (t !in want) model.core.setTag(id, t, false)
                        for (t in want) if (t !in detail.tags) model.core.setTag(id, t, true)
                    }
                }) { Text("Set") }
            }

            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                val running = model.active?.task == id
                Button(onClick = { model.act { if (running) model.core.stopWork() else model.core.startWork(id) } }) {
                    Icon(if (running) Icons.Default.Stop else Icons.Default.PlayArrow, null)
                    Text(if (running) " Stop working" else " Start working")
                }
                OutlinedButton(onClick = { ask = Ask.Subtask }) { Text("Add subtask") }
            }

            Text(
                "Score ${detail.scorePriority + detail.scoreDue + detail.scoreBlocking}  " +
                    "(priority ${detail.scorePriority}, due ${detail.scoreDue}, blocks ${detail.scoreBlocking})",
                style = MaterialTheme.typography.bodySmall,
            )
            detail.author?.let { Text("Created by $it", style = MaterialTheme.typography.bodySmall) }

            Deps(model, "Requires", detail.requires) { dep ->
                model.act { model.core.setBlockedBy(id, dep, false) }
            }
            Deps(model, "Blocks", detail.blocks, null)

            HorizontalDivider()
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Work log  ${clock(detail.workedMs)}", Modifier.weight(1f), fontWeight = FontWeight.Bold)
                TextButton(onClick = { ask = Ask.NewSession }) { Text("Log time") }
            }
            for (s in detail.sessions.take(30)) {
                val end = s.end
                Row(Modifier.fillMaxWidth().clickable { ask = Ask.Session(s) }.padding(vertical = 4.dp)) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            "${fmtTime(s.start)} – ${if (end == null) "now" else fmtTime(end).substringAfter(' ')}",
                            style = MaterialTheme.typography.bodyMedium,
                        )
                        val who = listOfNotNull(if (s.mine) null else s.user, s.note.ifEmpty { null })
                        if (who.isNotEmpty()) Text(who.joinToString(" · "), style = MaterialTheme.typography.bodySmall)
                    }
                    Text(
                        clock((end ?: System.currentTimeMillis()) - (s.start ?: 0)),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
            }
            if (detail.sessions.size > 30) {
                Text("… ${detail.sessions.size - 30} older sessions", style = MaterialTheme.typography.bodySmall)
            }
            Text("", Modifier.padding(bottom = 24.dp))
        }
    }

    when (val a = ask) {
        null -> {}
        Ask.Subtask -> TextDialog("New subtask", "Title", onDismiss = { ask = null }) { t ->
            model.act { model.core.addTask(detail.tab, PlaceArg.LastChildOf(id), t) }
            ask = null
        }
        Ask.Parent -> TaskPicker(model, "Move under…", exclude = id, tab = detail.tab, onDismiss = { ask = null }) { p ->
            ask = null
            model.act { model.core.moveTask(id, PlaceArg.LastChildOf(p)) }
        }
        Ask.Requires -> TaskPicker(model, "Requires…", exclude = id, tab = null, onDismiss = { ask = null }) { b ->
            ask = null
            model.act { model.core.setBlockedBy(id, b, true) }
        }
        Ask.Tab -> TabPickerDialog(model, exclude = detail.tab, onDismiss = { ask = null }) { tab ->
            ask = null
            save()
            var moved: String? = null
            if (model.act { moved = model.core.moveToTab(id, tab) }) {
                moved?.let { model.open(it) }
                model.message = "moved with its subtasks and work log"
            }
        }
        Ask.NewSession -> SessionDialog(model, id, null) { ask = null }
        is Ask.Session -> SessionDialog(model, id, a.s) { ask = null }
    }
}

@Composable
private fun Deps(model: Model, label: String, deps: List<DepView>, remove: ((String) -> Unit)?) {
    if (deps.isEmpty()) return
    Text(label, style = MaterialTheme.typography.labelLarge)
    for (d in deps) {
        val state = when (d.state) {
            DepStateView.TODO -> ""
            DepStateView.DONE -> " ✓"
            DepStateView.DELETED -> " (deleted)"
            DepStateView.MISSING -> " (not synced yet)"
            DepStateView.UNKNOWN_TAB -> " (in a tab you don't have)"
        }
        val tab = d.tabName?.let { " [$it]" } ?: ""
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                "• ${d.title ?: d.task}$tab$state",
                Modifier.weight(1f).clickable(enabled = d.title != null) { model.open(d.task) }.padding(vertical = 6.dp),
                style = MaterialTheme.typography.bodyMedium,
            )
            if (remove != null) {
                IconButton(onClick = { remove(d.task) }, Modifier.size(32.dp)) {
                    Icon(Icons.Default.Close, "No longer required")
                }
            }
        }
    }
}

/** A work session: new (`s` null: logged after the fact) or to correct. */
@Composable
private fun SessionDialog(model: Model, task: String, s: SessionView?, onDismiss: () -> Unit) {
    val now = System.currentTimeMillis()
    var start by remember { mutableStateOf(fmtTime(s?.start ?: (now - 3_600_000))) }
    var end by remember { mutableStateOf(if (s == null) fmtTime(now) else s.end?.let { fmtTime(it) } ?: "") }
    var note by remember { mutableStateOf(s?.note ?: "") }
    var error by remember { mutableStateOf<String?>(null) }
    val running = s != null && s.end == null
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (s == null) "Log time" else "Work session") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(start, { start = it }, label = { Text("Start (yyyy-MM-dd HH:mm)") }, singleLine = true)
                OutlinedTextField(
                    end, { end = it },
                    label = { Text(if (running) "End (empty: still running)" else "End") },
                    singleLine = true,
                )
                OutlinedTextField(note, { note = it }, label = { Text("Note") }, singleLine = true)
                if (s != null && !s.mine) Text("Recorded by ${s.user}", style = MaterialTheme.typography.bodySmall)
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                val st = parseTime(start)
                val en = if (end.isBlank()) null else parseTime(end)
                when {
                    st == null -> error = "can't read the start"
                    end.isNotBlank() && en == null -> error = "can't read the end"
                    en == null && !running -> error = "give an end"
                    else -> {
                        val ok = model.act {
                            val id = if (s == null) {
                                model.core.addSession(task, st, en!!)
                            } else {
                                model.core.setSessionSpan(s.id, st, en)
                                s.id
                            }
                            if (note != (s?.note ?: "")) model.core.setSessionNote(id, note.trim())
                        }
                        if (ok) onDismiss()
                    }
                }
            }) { Text("Save") }
        },
        dismissButton = {
            Row {
                if (s != null) {
                    TextButton(onClick = {
                        if (model.act { model.core.deleteSession(s.id) }) onDismiss()
                    }) { Text("Delete", color = MaterialTheme.colorScheme.error) }
                }
                TextButton(onClick = onDismiss) { Text("Cancel") }
            }
        },
    )
}
