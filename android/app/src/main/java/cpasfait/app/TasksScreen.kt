package cpasfait.app

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Sync
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import cpasfait.core.PlaceArg
import cpasfait.core.RowView
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** The rows to show: those under a folded task are left out. */
fun visibleRows(rows: List<RowView>, collapsed: Set<String>): List<RowView> {
    val out = mutableListOf<RowView>()
    var hiddenBelow = Int.MAX_VALUE
    for (r in rows) {
        val depth = r.depth.toInt()
        if (depth > hiddenBelow) continue
        hiddenBelow = if (r.id in collapsed) depth else Int.MAX_VALUE
        out.add(r)
    }
    return out
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TasksScreen(model: Model) {
    val scope = rememberCoroutineScope()
    val snackbar = remember { SnackbarHostState() }
    var adding by remember { mutableStateOf<String?>(null) }
    var newTab by remember { mutableStateOf(false) }
    MessageSnackbar(model, snackbar)
    Scaffold(
        topBar = {
            TopAppBar(
                title = { TabPicker(model) { newTab = true } },
                actions = {
                    if (model.syncing) {
                        CircularProgressIndicator(Modifier.size(24.dp).padding(2.dp), strokeWidth = 2.dp)
                    } else {
                        IconButton(onClick = { scope.launch { model.sync(explicit = true) } }) {
                            Icon(Icons.Default.Sync, "Sync")
                        }
                    }
                    var menu by remember { mutableStateOf(false) }
                    IconButton(onClick = { menu = true }) { Icon(Icons.Default.MoreVert, "Menu") }
                    DropdownMenu(menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(
                            text = { Text(if (model.hideDone) "Show done tasks" else "Hide done tasks") },
                            onClick = { menu = false; model.toggleHideDone() },
                        )
                        DropdownMenuItem(
                            text = { Text("Settings & sharing") },
                            onClick = { menu = false; model.screen = Screen.Settings },
                        )
                    }
                },
            )
        },
        floatingActionButton = {
            FloatingActionButton(onClick = { adding = "" }) { Icon(Icons.Default.Add, "Add a task") }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize()) {
            TimerBar(model)
            val rows = visibleRows(model.rows, model.collapsed)
            if (rows.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text("Nothing to do!", style = MaterialTheme.typography.bodyLarge)
                }
            }
            LazyColumn(contentPadding = PaddingValues(bottom = 88.dp)) {
                items(rows, key = { it.id }) { r ->
                    TaskRow(model, r)
                    HorizontalDivider(Modifier.alpha(0.3f))
                }
            }
            model.syncStatus?.let {
                Text(it, Modifier.padding(8.dp), style = MaterialTheme.typography.labelSmall)
            }
        }
    }
    adding?.let {
        TextDialog("New task", "Title", onDismiss = { adding = null }) { title ->
            val tab = model.tabId ?: return@TextDialog
            model.act { model.core.addTask(tab, PlaceArg.LastChildOf(null), title) }
            adding = null
        }
    }
    if (newTab) {
        TextDialog("New tab", "Name", onDismiss = { newTab = false }) { name ->
            model.act {
                val id = model.core.createTab(name)
                model.selectTab(id)
            }
            newTab = false
        }
    }
}

@Composable
fun MessageSnackbar(model: Model, snackbar: SnackbarHostState) {
    LaunchedEffect(model.message) {
        model.message?.let {
            snackbar.showSnackbar(it)
            model.message = null
        }
    }
}

@Composable
private fun TabPicker(model: Model, onNew: () -> Unit) {
    var open by remember { mutableStateOf(false) }
    Row(Modifier.clickable { open = true }, verticalAlignment = Alignment.CenterVertically) {
        Text(model.tab?.name ?: "cpasfait", maxLines = 1, overflow = TextOverflow.Ellipsis)
        Icon(Icons.Default.ArrowDropDown, "Tabs")
    }
    DropdownMenu(open, onDismissRequest = { open = false }) {
        for (t in model.tabs) {
            DropdownMenuItem(
                text = {
                    Text(
                        t.name + if (t.remote != null) "  ⇅" else "",
                        fontWeight = if (t.id == model.tabId) FontWeight.Bold else null,
                    )
                },
                onClick = { open = false; model.selectTab(t.id) },
            )
        }
        HorizontalDivider()
        DropdownMenuItem(text = { Text("New tab…") }, onClick = { open = false; onNew() })
    }
}

/** The running session, with a live clock and a stop button. */
@Composable
private fun TimerBar(model: Model) {
    val s = model.active ?: return
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(s.id) {
        while (true) {
            now = System.currentTimeMillis()
            delay(1_000)
        }
    }
    val title = model.rows.firstOrNull { it.id == s.task }?.title ?: "…"
    Surface(color = MaterialTheme.colorScheme.primaryContainer) {
        Row(
            Modifier.fillMaxWidth().clickable { s.task?.let { model.screen = Screen.Task(it) } }.padding(start = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("⏱ ${clock(now - (s.start ?: now))}", fontWeight = FontWeight.Bold)
            Spacer(Modifier.width(12.dp))
            Text(title, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            IconButton(onClick = { model.act { model.core.stopWork() } }) {
                Icon(Icons.Default.Stop, "Stop")
            }
        }
    }
}

@Composable
private fun TaskRow(model: Model, r: RowView) {
    val running = model.active?.task == r.id
    Row(
        Modifier
            .fillMaxWidth()
            .clickable { model.screen = Screen.Task(r.id) }
            .padding(start = (r.depth.toInt() * 20).dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (r.hasChildren) {
            val folded = r.id in model.collapsed
            IconButton(onClick = {
                model.collapsed = if (folded) model.collapsed - r.id else model.collapsed + r.id
            }) {
                Icon(
                    if (folded) Icons.Default.KeyboardArrowRight else Icons.Default.KeyboardArrowDown,
                    if (folded) "Unfold" else "Fold",
                )
            }
        } else {
            Spacer(Modifier.width(48.dp))
        }
        Checkbox(checked = r.done, onCheckedChange = { done -> model.act { model.core.setDone(r.id, done) } })
        Column(Modifier.weight(1f).padding(vertical = 6.dp)) {
            Text(
                r.title,
                style = MaterialTheme.typography.bodyLarge,
                textDecoration = if (r.effectivelyDone) TextDecoration.LineThrough else null,
                modifier = Modifier.alpha(if (r.effectivelyDone || r.blocked) 0.5f else 1f),
            )
            val meta = meta(r)
            if (meta.isNotEmpty()) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    for ((text, color) in meta) {
                        Text(text, style = MaterialTheme.typography.labelMedium, color = color ?: Color.Unspecified)
                    }
                }
            }
        }
        IconButton(onClick = {
            model.act { if (running) model.core.stopWork() else model.core.startWork(r.id) }
        }) {
            Icon(
                if (running) Icons.Default.Stop else Icons.Default.PlayArrow,
                if (running) "Stop working" else "Start working",
                tint = if (running) MaterialTheme.colorScheme.primary else LocalContentColorDim(),
            )
        }
    }
}

@Composable
private fun LocalContentColorDim(): Color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.4f)

/** Small labels under a title: due date, priority, tags, who works on it. */
@Composable
private fun meta(r: RowView): List<Pair<String, Color?>> {
    val out = mutableListOf<Pair<String, Color?>>()
    if (r.blocked) out.add("⛔ blocked" to null)
    r.due?.let { due ->
        val ms = r.dueMs
        val now = System.currentTimeMillis()
        val color = when {
            r.effectivelyDone || ms == null -> null
            ms <= now -> MaterialTheme.colorScheme.error
            ms - now < 24 * 3600_000 -> Color(0xFFB8860B)
            else -> null
        }
        out.add("⌛ $due" to color)
    }
    if (r.priority != 0L) out.add((if (r.priority > 0) "+${r.priority}" else "${r.priority}") to null)
    if (r.tags.isNotEmpty()) out.add(r.tags.joinToString(" ") { "#$it" } to null)
    if (r.working.isNotEmpty()) out.add("⏱ ${r.working.joinToString(", ")}" to MaterialTheme.colorScheme.primary)
    if (r.hasNotes) out.add("✎" to null)
    return out
}

fun clock(ms: Long): String {
    val s = ms / 1000
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, s / 60 % 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

@Composable
fun TextDialog(
    title: String,
    label: String,
    initial: String = "",
    confirm: String = "OK",
    onDismiss: () -> Unit,
    onOk: (String) -> Unit,
) {
    var text by remember { mutableStateOf(initial) }
    val focus = remember { FocusRequester() }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            OutlinedTextField(
                text,
                { text = it },
                label = { Text(label) },
                singleLine = true,
                modifier = Modifier.focusRequester(focus),
            )
            LaunchedEffect(Unit) { focus.requestFocus() }
        },
        confirmButton = {
            TextButton(onClick = { if (text.isNotBlank()) onOk(text.trim()) }) { Text(confirm) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
