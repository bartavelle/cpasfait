package cpasfait.app

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.util.Log
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import cpasfait.core.Cpasfait
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.bouncycastle.jce.provider.BouncyCastleProvider
import java.io.File
import java.security.Security
import java.time.ZoneId
import java.util.concurrent.TimeUnit

class CpasfaitApp : Application() {
    /** The app's scope (sync after edits...). */
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    val coreDir: File by lazy { File(filesDir, "cpasfait") }

    /** The connection the screens use (fast calls, main thread). */
    val core: Cpasfait by lazy { open() }

    /** The connection syncs use (slow, background threads). */
    val syncCore: Cpasfait by lazy { open() }

    val model: Model by lazy { Model(this) }

    private var pendingSync: Job? = null

    private fun open(): Cpasfait = Cpasfait.open(coreDir.path, ZoneId.systemDefault().id)

    override fun onCreate() {
        super.onCreate()
        // sshj needs the full Bouncy Castle, not Android's trimmed copy.
        Security.removeProvider("BC")
        Security.insertProviderAt(BouncyCastleProvider(), 1)

        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(Dues.CHANNEL, "Due tasks", NotificationManager.IMPORTANCE_HIGH),
        )
        WorkManager.getInstance(this).enqueueUniquePeriodicWork(
            "sync",
            ExistingPeriodicWorkPolicy.KEEP,
            PeriodicWorkRequestBuilder<SyncWorker>(15, TimeUnit.MINUTES)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .build(),
        )
    }

    /** Local changes: push them soon, and update the due alarms. */
    fun edited() {
        Dues.schedule(this)
        pendingSync?.cancel()
        pendingSync = scope.launch {
            delay(3_000)
            model.sync(explicit = false)
        }
    }

    companion object {
        const val TAG = "cpasfait"

        fun log(msg: String, e: Throwable? = null) {
            Log.i(TAG, msg, e)
        }
    }
}
