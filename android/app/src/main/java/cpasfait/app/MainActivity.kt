package cpasfait.app

import android.Manifest
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val app = application as CpasfaitApp
        val model = app.model
        // While visible: pick up what the background sync received.
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                model.reload()
                Dues.check(app)
                Dues.schedule(app)
                launch { model.sync(explicit = false) }
                while (true) {
                    delay(2_000)
                    model.reloadIfChanged()
                }
            }
        }
        setContent {
            Theme {
                AskNotificationPermission()
                App(model)
            }
        }
    }
}

@Composable
private fun Theme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val ctx = LocalContext.current
    val colors = when {
        Build.VERSION.SDK_INT >= 31 && dark -> dynamicDarkColorScheme(ctx)
        Build.VERSION.SDK_INT >= 31 -> dynamicLightColorScheme(ctx)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    MaterialTheme(colorScheme = colors, content = content)
}

@Composable
private fun AskNotificationPermission() {
    if (Build.VERSION.SDK_INT < 33) return
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    LaunchedEffect(Unit) { launcher.launch(Manifest.permission.POST_NOTIFICATIONS) }
}

@Composable
fun App(model: Model) {
    when (val s = model.screen) {
        Screen.Tasks -> TasksScreen(model)
        is Screen.Task -> {
            BackHandler { model.screen = Screen.Tasks }
            TaskScreen(model, s.id)
        }
        Screen.Settings -> {
            BackHandler { model.screen = Screen.Tasks }
            SettingsScreen(model)
        }
        Screen.Search -> {
            BackHandler { model.screen = Screen.Tasks }
            SearchScreen(model)
        }
        Screen.Time -> {
            BackHandler { model.screen = Screen.Tasks }
            TimeScreen(model)
        }
    }
}
