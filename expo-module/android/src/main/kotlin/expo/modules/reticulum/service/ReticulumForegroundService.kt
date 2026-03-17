package expo.modules.reticulum.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import android.util.Log

class ReticulumForegroundService : Service() {

    override fun onCreate() {
        super.onCreate()
        Log.i("ReticulumService", "Foreground service created")
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        Log.i("ReticulumService", "Foreground service started")
        
        val notification = NotificationCompat.Builder(this, "reticulum_channel")
            .setContentTitle("Reticulum")
            .setContentText("Mesh node running in background")
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            // .setOngoing(true) // handled by startForeground
            .build()
            
        // Start foreground immediately
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(1, notification, android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(1, notification)
        }

        // Return START_STICKY so the OS tries to restart it if killed
        return START_STICKY
    }

    override fun onDestroy() {
        super.onDestroy()
        Log.i("ReticulumService", "Foreground service destroyed")
    }

    override fun onBind(intent: Intent?): IBinder? {
        return null // We don't need bind, just start/stop
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val name = "Reticulum Service"
            val descriptionText = "Keeps the Reticulum mesh node running in the background"
            val importance = NotificationManager.IMPORTANCE_LOW
            val channel = NotificationChannel("reticulum_channel", name, importance).apply {
                description = descriptionText
            }
            val notificationManager: NotificationManager =
                getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            notificationManager.createNotificationChannel(channel)
        }
    }
}
