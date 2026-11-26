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
        fun post(id: Int, title: String, text: String, task: String?) {
            val b = NotificationCompat.Builder(app, CHANNEL)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle(title)
                .setContentText(text)
                .setStyle(NotificationCompat.BigTextStyle().bigText(text))
                .setPriority(NotificationCompat.PRIORITY_HIGH)
                .setCategory(NotificationCompat.CATEGORY_REMINDER)
                .setContentIntent(open)
                .setAutoCancel(true)
            if (task != null) {
                for ((action, label) in listOf(DueAction.DONE to "Done", DueAction.SNOOZE to "In 1 hour")) {
                    val intent = Intent(app, DueAction::class.java)
                        .setAction(action)
                        .putExtra(DueAction.TASK, task)
                        .putExtra(DueAction.NOTIFICATION, id)
                    val pi = PendingIntent.getBroadcast(
                        app, (task + action).hashCode(), intent,
                        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                    )
                    b.addAction(0, label, pi)
                }
            }
            nm.notify(id, b.build())
        }
        if (notices.size > MAX_SEPARATE) {
            post(0, "${notices.size} tasks are due", notices.take(10).joinToString("\n") { "${it.title} (${it.due})" }, null)
        } else {
            for (n in notices) post(n.task.hashCode(), "Due: ${n.title}", "${n.due} · ${n.tabName}", n.task)
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

/** The buttons of a due notification: mark the task done, or due again in an hour. */
class DueAction : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val app = context.applicationContext as CpasfaitApp
        val task = intent.getStringExtra(TASK) ?: return
        app.getSystemService(NotificationManager::class.java).cancel(intent.getIntExtra(NOTIFICATION, 0))
        try {
            when (intent.action) {
                DONE -> app.core.setDone(task, true)
                SNOOZE -> {
                    val at = java.time.LocalDateTime.now().plusHours(1).withSecond(0).withNano(0)
                    app.core.setDue(task, at.toString().replace('T', ' '))
                }
            }
        } catch (e: Exception) {
            CpasfaitApp.log("due action", e)
            return
        }
        app.model.reload()
        app.edited()
    }

    companion object {
        const val DONE = "cpasfait.app.DONE"
        const val SNOOZE = "cpasfait.app.SNOOZE"
        const val TASK = "task"
        const val NOTIFICATION = "notification"
    }
}
