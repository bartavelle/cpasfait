package cpasfait.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.ExperimentalMaterial3Api
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import cpasfait.core.CpasfaitException
import cpasfait.core.MemberView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(model: Model) {
    val snackbar = remember { SnackbarHostState() }
    MessageSnackbar(model, snackbar)
    val scope = rememberCoroutineScope()
    val ctx = LocalContext.current
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Settings & sharing") },
                navigationIcon = {
                    IconButton(onClick = { model.screen = Screen.Tasks }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back")
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
            // ---------------------------------------------------------------- you
            Section("You")
            var name by remember { mutableStateOf(model.core.user()) }
            Field(name, { name = it }, "Name (on your work sessions)") { model.act { model.core.setUser(name) } }
            var email by remember { mutableStateOf(model.core.email() ?: "") }
            Field(email, { email = it }, "Email (author of your tasks)") {
                model.act { model.core.setEmail(email.ifBlank { null }) }
            }

            // ---------------------------------------------------------------- keys
            Section("Keys")
            val ageKey = remember { runCatching { model.core.publicKey() }.getOrElse { "?" } }
            Key(ctx, "This phone's key: give it to :tabinvite on a device of the tab", ageKey)
            val sshKey = remember { runCatching { model.keys.public() }.getOrElse { "?" } }
            Key(ctx, "SSH key for rsync: remotes: add it to the server's ~/.ssh/authorized_keys", sshKey)

            // ---------------------------------------------------------------- join
            Section("Join a shared tab")
            var spec by remember { mutableStateOf("") }
            var joining by remember { mutableStateOf(false) }
            OutlinedTextField(
                spec, { spec = it }, Modifier.fillMaxWidth(),
                label = { Text("Remote") },
                placeholder = { Text("rsync:user@host:path") },
                singleLine = true,
            )
            Button(
                enabled = spec.isNotBlank() && !joining,
                onClick = {
                    joining = true
                    scope.launch {
                        try {
                            val j = withContext(Dispatchers.IO) {
                                SftpProvider(model.keys).use { model.app.syncCore.joinTab(spec.trim(), null, it) }
                            }
                            model.reload()
                            model.selectTab(j.tab)
                            model.message = j.waitingForInvitation?.let {
                                "joined, waiting for an invitation: run :tabinvite $it on a device of the tab"
                            } ?: "joined"
                            if (j.waitingForInvitation == null) model.screen = Screen.Tasks
                        } catch (e: CpasfaitException) {
                            model.report(e)
                        } finally {
                            joining = false
                        }
                    }
                },
            ) { Text(if (joining) "Joining…" else "Join") }

            // ---------------------------------------------------------------- current tab
            val tab = model.tab
            if (tab != null) {
                Section("This tab: ${tab.name}")
                var newName by remember(tab.id) { mutableStateOf(tab.name) }
                Field(newName, { newName = it }, "Name") { model.act { model.core.renameTab(tab.id, newName.trim()) } }
                Text(
                    "Remote: ${tab.remote ?: "none (only on this phone)"}",
                    style = MaterialTheme.typography.bodyMedium,
                )
                tab.pendingMove?.let { to ->
                    Text("Moved by another device to $to", color = MaterialTheme.colorScheme.error)
                    Button(onClick = {
                        scope.launch {
                            try {
                                val warnings = withContext(Dispatchers.IO) {
                                    SftpProvider(model.keys).use { model.app.syncCore.acceptMove(tab.id, null, it) }
                                }
                                model.reload()
                                model.message = if (warnings.isEmpty()) "now using $to" else warnings.joinToString("\n")
                            } catch (e: CpasfaitException) {
                                model.report(e)
                            }
                        }
                    }) { Text("Follow the move") }
                }
                if (tab.remote == null) {
                    var share by remember(tab.id) { mutableStateOf("") }
                    Field(share, { share = it }, "Share through (rsync:user@host:path)", "Share") {
                        if (model.act { model.core.setRemote(tab.id, share.trim()) }) model.message = "shared, syncing"
                    }
                } else {
                    var key by remember(tab.id) { mutableStateOf("") }
                    Field(key, { key = it }, "Invite a key (age1… or ssh-ed25519 …)", "Invite") {
                        if (model.act { model.core.invite(tab.id, key.trim(), null) }) {
                            model.message = "invited: they can now join ${tab.remote}"
                            key = ""
                        }
                    }
                    val members = remember(tab.id, model.generation) {
                        runCatching { model.core.members(tab.id) }.getOrElse { emptyList() }
                    }
                    Text("Members", style = MaterialTheme.typography.labelLarge)
                    val me = remember { runCatching { model.core.publicKey() }.getOrNull() }
                    var revoking by remember { mutableStateOf<MemberView?>(null) }
                    for (m in members.filter { it.active }) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                "• ${m.name}" + if (m.key == me) " (this phone)" else "",
                                Modifier.weight(1f),
                                style = MaterialTheme.typography.bodyMedium,
                            )
                            if (m.key != me) {
                                IconButton(onClick = { revoking = m }) { Icon(Icons.Default.Close, "Remove ${m.name}") }
                            }
                        }
                    }
                    revoking?.let { m ->
                        Confirm(
                            "Remove ${m.name}?",
                            "They won't get the new changes (a new key is made at the next sync). " +
                                "Also stop sharing the remote folder with them.",
                            "Remove",
                            onDismiss = { revoking = null },
                        ) {
                            revoking = null
                            if (model.act { model.core.revoke(tab.id, m.key) }) model.message = "removed ${m.name}"
                        }
                    }

                    var unsharing by remember { mutableStateOf(false) }
                    OutlinedButton(onClick = { unsharing = true }) { Text("Stop syncing this tab") }
                    if (unsharing) {
                        Confirm(
                            "Stop syncing \"${tab.name}\"?",
                            "It stays on this phone, and on the other devices and the remote as it is now. " +
                                "To sync it again, share it again or join it.",
                            "Stop",
                            onDismiss = { unsharing = false },
                        ) {
                            unsharing = false
                            if (model.act { model.core.setRemote(tab.id, null) }) model.message = "no longer synced"
                        }
                    }

                    var moveTo by remember(tab.id) { mutableStateOf("") }
                    var moving by remember { mutableStateOf(false) }
                    Field(moveTo, { moveTo = it }, "Move the tab to another remote", if (moving) "Moving…" else "Move") {
                        if (moving || moveTo.isBlank()) return@Field
                        moving = true
                        scope.launch {
                            try {
                                val n = withContext(Dispatchers.IO) {
                                    SftpProvider(model.keys).use { model.app.syncCore.moveTab(tab.id, moveTo.trim(), it) }
                                }
                                model.reload()
                                model.message = "moved ($n files copied); the other devices are asked to follow"
                                moveTo = ""
                            } catch (e: CpasfaitException) {
                                model.report(e)
                            } finally {
                                moving = false
                            }
                        }
                    }
                }

                Text("Priority of new tasks", style = MaterialTheme.typography.labelLarge)
                val prio = remember(tab.id, model.generation) {
                    runCatching { model.core.defaultPriority(tab.id) }.getOrDefault(0L)
                }
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    for (p in -3L..3L) {
                        FilterChip(
                            selected = prio == p,
                            onClick = { model.act { model.core.setDefaultPriority(tab.id, if (p == 0L) null else p) } },
                            label = { Text(if (p > 0) "+$p" else "$p") },
                        )
                    }
                }

                var deleting by remember { mutableStateOf(false) }
                OutlinedButton(onClick = { deleting = true }) {
                    Text("Delete this tab", color = MaterialTheme.colorScheme.error)
                }
                if (deleting) {
                    Confirm(
                        "Delete \"${tab.name}\"?",
                        if (tab.remote != null) "It is deleted for everyone sharing it." else "Its tasks are deleted.",
                        "Delete",
                        onDismiss = { deleting = false },
                    ) {
                        deleting = false
                        if (model.act { model.core.deleteTab(tab.id) }) {
                            model.message = "deleted ${tab.name}"
                            model.screen = Screen.Tasks
                        }
                    }
                }
            }
            HorizontalDivider()
            Text(
                "Data: ${model.app.coreDir}",
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(bottom = 24.dp),
            )
        }
    }
}

@Composable
private fun Confirm(title: String, text: String, confirm: String, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(text) },
        confirmButton = { TextButton(onClick = onConfirm) { Text(confirm, color = MaterialTheme.colorScheme.error) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun Section(title: String) {
    HorizontalDivider()
    Text(title, style = MaterialTheme.typography.titleMedium)
}

@Composable
private fun Field(value: String, onChange: (String) -> Unit, label: String, button: String = "Save", onSave: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(value, onChange, Modifier.weight(1f), label = { Text(label) }, singleLine = true)
        TextButton(onClick = onSave) { Text(button) }
    }
}

@Composable
private fun Key(ctx: Context, label: String, key: String) {
    Text(label, style = MaterialTheme.typography.bodySmall)
    SelectionContainer {
        Text(key, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
    }
    OutlinedButton(onClick = {
        ctx.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("key", key))
    }) { Text("Copy") }
}
