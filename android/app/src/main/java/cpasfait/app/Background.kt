package cpasfait.app

import android.Manifest
import android.app.AlarmManager
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import cpasfait.core.DueView

/** Periodic sync (every 15 minutes, as often as Android allows). */
class SyncWorker(ctx: Context, params: WorkerParameters) : CoroutineWorker(ctx, params) {
    override suspend fun doWork(): Result {
        val app = applicationContext as CpasfaitApp
        app.model.sync(explicit = false)
        Dues.check(app)
        Dues.schedule(app)
        return Result.success()
    }
}

/** Due date notifications: an alarm at the next due time, then the core says
 * what fell due (each due date once, see `takeDueNotices`). */
object Dues {
    const val CHANNEL = "due"
    private const val MAX_SEPARATE = 3

    fun check(app: CpasfaitApp) {
        val notices = try {
            app.core.takeDueNotices()
        } catch (e: Exception) {
            CpasfaitApp.log("due notices", e)
            return
        }
        if (notices.isEmpty()) return
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(app, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val nm = app.getSystemService(NotificationManager::class.java)
        val open = PendingIntent.getActivity(
            app, 0, Intent(app, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        fun post(id: Int, title: String, text: String) {
            nm.notify(
                id,
                NotificationCompat.Builder(app, CHANNEL)
                    .setSmallIcon(android.R.drawable.ic_popup_reminder)
                    .setContentTitle(title)
                    .setContentText(text)
                    .setStyle(NotificationCompat.BigTextStyle().bigText(text))
                    .setPriority(NotificationCompat.PRIORITY_HIGH)
                    .setCategory(NotificationCompat.CATEGORY_REMINDER)
                    .setContentIntent(open)
                    .setAutoCancel(true)
                    .build(),
            )
        }
        if (notices.size > MAX_SEPARATE) {
            post(0, "${notices.size} tasks are due", notices.take(10).joinToString("\n") { "${it.title} (${it.due})" })
        } else {
            for (n in notices) post(n.task.hashCode(), "Due: ${n.title}", "${n.due} · ${n.tabName}")
        }
    }

    /** An alarm at the next due time. */
    fun schedule(app: CpasfaitApp) {
        val next: DueView = try {
            app.core.upcomingDues().firstOrNull()
        } catch (e: Exception) {
            CpasfaitApp.log("upcoming dues", e)
            null
        } ?: return
        val am = app.getSystemService(AlarmManager::class.java)
        val pi = PendingIntent.getBroadcast(
            app, 0, Intent(app, DueReceiver::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        if (Build.VERSION.SDK_INT < 31 || am.canScheduleExactAlarms()) {
            am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, next.dueMs, pi)
        } else {
            am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, next.dueMs, pi)
        }
    }
}

class DueReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val app = context.applicationContext as CpasfaitApp
        Dues.check(app)
        Dues.schedule(app)
    }
}

class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) {
            val app = context.applicationContext as CpasfaitApp
            Dues.check(app)
            Dues.schedule(app)
        }
    }
}
