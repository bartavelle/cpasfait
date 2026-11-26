package cpasfait.app

import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import cpasfait.core.SearchHit
import java.time.DayOfWeek
import java.time.LocalDate
import java.time.ZoneId
import java.time.temporal.TemporalAdjusters

@Composable
private fun HitRow(h: SearchHit, showTab: Boolean, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 8.dp)) {
        Text(
            h.title,
            style = MaterialTheme.typography.bodyLarge,
            textDecoration = if (h.done) TextDecoration.LineThrough else null,
            modifier = Modifier.alpha(if (h.done) 0.5f else 1f),
        )
        val where = listOfNotNull(if (showTab) h.tabName else null, h.path.ifEmpty { null }).joinToString(" · ")
        if (where.isNotEmpty()) {
            Text(where, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** Find a task in any tab. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SearchScreen(model: Model) {
    var query by remember { mutableStateOf("") }
    val hits = remember(query, model.generation) {
        runCatching { model.core.search(query, 100u) }.getOrElse { emptyList() }
    }
    val focus = remember { FocusRequester() }
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    TextField(
                        query, { query = it },
                        Modifier.fillMaxWidth().focusRequester(focus),
                        placeholder = { Text("Search all tabs") },
                        singleLine = true,
                    )
                },
                navigationIcon = {
                    IconButton(onClick = { model.screen = Screen.Tasks }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back")
                    }
                },
            )
        },
    ) { pad ->
        LaunchedEffect(Unit) { focus.requestFocus() }
        LazyColumn(Modifier.padding(pad).fillMaxSize()) {
            items(hits, key = { it.task }) { h ->
                HitRow(h, showTab = true) { model.open(h.task) }
                HorizontalDivider(Modifier.alpha(0.3f))
            }
        }
    }
}

/** Choose a task: of `tab` (all of them until something is typed), or of
 * any tab (`tab` null). */
@Composable
fun TaskPicker(
    model: Model,
    title: String,
    exclude: String,
    tab: String?,
    onDismiss: () -> Unit,
    onPick: (String) -> Unit,
) {
    var query by remember { mutableStateOf("") }
    val hits = remember(query) {
        runCatching {
            if (query.isBlank() && tab != null) {
                model.core.rows(tab, true).map {
                    SearchHit(it.id, tab, "", it.title, "", it.effectivelyDone)
                }
            } else {
                model.core.search(query, 50u).filter { tab == null || it.tab == tab }
            }
        }.getOrElse { emptyList() }.filter { it.task != exclude }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                OutlinedTextField(query, { query = it }, placeholder = { Text("Search") }, singleLine = true)
                LazyColumn(Modifier.heightIn(max = 360.dp)) {
                    items(hits, key = { it.task }) { h -> HitRow(h, showTab = tab == null) { onPick(h.task) } }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
fun TabPickerDialog(model: Model, exclude: String, onDismiss: () -> Unit, onPick: (String) -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Move to…") },
        text = {
            Column {
                val others = model.tabs.filter { it.id != exclude }
                if (others.isEmpty()) Text("No other tab: make one from the tab menu.")
                for (t in others) {
                    Text(
                        t.name,
                        Modifier.fillMaxWidth().clickable { onPick(t.id) }.padding(vertical = 12.dp),
                        style = MaterialTheme.typography.bodyLarge,
                    )
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

private enum class Period(val label: String) {
    TODAY("Today"), YESTERDAY("Yesterday"), WEEK("This week"), LAST_WEEK("Last week"), MONTH("This month"), DAYS30("30 days");

    /** [from, to) in Unix ms. */
    fun range(): Pair<Long, Long> {
        val zone = ZoneId.systemDefault()
        val today = LocalDate.now(zone)
        val monday = today.with(TemporalAdjusters.previousOrSame(DayOfWeek.MONDAY))
        val (from, to) = when (this) {
            TODAY -> today to today.plusDays(1)
            YESTERDAY -> today.minusDays(1) to today
            WEEK -> monday to monday.plusWeeks(1)
            LAST_WEEK -> monday.minusWeeks(1) to monday
            MONTH -> today.withDayOfMonth(1) to today.withDayOfMonth(1).plusMonths(1)
            DAYS30 -> today.minusDays(29) to today.plusDays(1)
        }
        fun ms(d: LocalDate) = d.atStartOfDay(zone).toInstant().toEpochMilli()
        return ms(from) to ms(to)
    }
}

/** Time spent per tab and task over a period (subtasks rolled up). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TimeScreen(model: Model) {
    var period by remember { mutableStateOf(Period.TODAY) }
    val report = remember(period, model.generation) {
        val (from, to) = period.range()
        runCatching { model.core.timeReport(from, to, null) }.getOrElse { emptyList() }
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Time spent") },
                navigationIcon = {
                    IconButton(onClick = { model.screen = Screen.Tasks }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back")
                    }
                },
            )
        },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize()) {
            Row(
                Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                for (p in Period.entries) {
                    FilterChip(selected = p == period, onClick = { period = p }, label = { Text(p.label) })
                }
            }
            val total = report.sumOf { it.totalMs }
            Text(
                "Total ${hm(total)}",
                Modifier.padding(16.dp),
                style = MaterialTheme.typography.titleMedium,
            )
            if (report.isEmpty()) Text("No time logged.", Modifier.padding(horizontal = 16.dp))
            LazyColumn {
                for (t in report) {
                    item(key = t.tab) {
                        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
                            Text(t.name, Modifier.weight(1f), fontWeight = FontWeight.Bold)
                            Text(hm(t.totalMs), fontWeight = FontWeight.Bold)
                        }
                    }
                    items(t.lines, key = { t.tab + (it.task ?: "deleted") }) { l ->
                        val task = l.task
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .clickable(enabled = task != null) { task?.let { model.open(it) } }
                                .padding(start = (16 + 16 * l.depth.toInt()).dp, end = 16.dp, top = 6.dp, bottom = 6.dp),
                        ) {
                            Text(l.title, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                            val own = if (l.ownMs != l.totalMs && l.ownMs > 0) " (${hm(l.ownMs)} itself)" else ""
                            Text(hm(l.totalMs) + own, style = MaterialTheme.typography.bodyMedium)
                        }
                    }
                    item(key = t.tab + "-sep") { HorizontalDivider() }
                }
            }
        }
    }
}

/** `1h05`, `25m`. */
fun hm(ms: Long): String {
    val m = ms / 60_000
    return if (m >= 60) "%dh%02d".format(m / 60, m % 60) else "${m}m"
}
