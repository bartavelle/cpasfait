package cpasfait.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
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
import cpasfait.core.TaskDetail
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

private val TIME = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm").withZone(ZoneId.systemDefault())

private fun fmt(ms: Long?): String = ms?.let { TIME.format(Instant.ofEpochMilli(it)) } ?: "?"

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
    var addingSub by remember { mutableStateOf(false) }
    val focus = LocalFocusManager.current

    fun save() {
        if (title.isNotBlank() && title.trim() != detail.title) model.act { model.core.setTitle(id, title.trim()) }
        if (notes != detail.notes) model.act { model.core.setNotes(id, notes) }
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
                    IconButton(onClick = {
                        if (model.act { model.core.deleteTask(id) }) {
                            model.message = "deleted \"${detail.title}\""
                            model.screen = Screen.Tasks
                        }
                    }) { Icon(Icons.Default.Delete, "Delete") }
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
                OutlinedButton(onClick = { addingSub = true }) { Text("Add subtask") }
            }

            Text(
                "Score ${detail.scorePriority + detail.scoreDue + detail.scoreBlocking}  " +
                    "(priority ${detail.scorePriority}, due ${detail.scoreDue}, blocks ${detail.scoreBlocking})",
                style = MaterialTheme.typography.bodySmall,
            )
            detail.author?.let { Text("Created by $it", style = MaterialTheme.typography.bodySmall) }

            Deps("Requires", detail.requires)
            Deps("Blocks", detail.blocks)

            HorizontalDivider()
            Text("Work log  ${clock(detail.workedMs)}", fontWeight = FontWeight.Bold)
            for (s in detail.sessions.take(20)) {
                val end = s.end
                Row {
                    Text(
                        "${fmt(s.start)} – ${if (end == null) "now" else fmt(end).substringAfter(' ')}",
                        Modifier.weight(1f),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    Text(
                        listOfNotNull(if (s.mine) null else s.user, s.note.ifEmpty { null }).joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
            if (detail.sessions.size > 20) {
                Text("… ${detail.sessions.size - 20} older sessions", style = MaterialTheme.typography.bodySmall)
            }
        }
    }
    if (addingSub) {
        TextDialog("New subtask", "Title", onDismiss = { addingSub = false }) { t ->
            model.act { model.core.addTask(detail.tab, PlaceArg.LastChildOf(id), t) }
            addingSub = false
        }
    }
}

@Composable
private fun Deps(label: String, deps: List<DepView>) {
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
        Text("• ${d.title ?: d.task}$tab$state", style = MaterialTheme.typography.bodyMedium)
    }
}
