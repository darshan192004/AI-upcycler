package org.aiupcycler.worker.metrics

import android.app.ActivityManager
import android.content.Context
import android.os.BatteryManager
import java.io.File
import org.aiupcycler.worker.rpc.DeviceMetricsSource

/**
 * Reads the device telemetry the master uses for scheduling and health checks.
 *
 * RAM comes from [`ActivityManager.MemoryInfo`]; battery capacity from
 * [`BatteryManager`]. Temperature is a best-effort read of
 * `/sys/class/thermal` because modern Android restricts the thermal sysfs to
 * privileged apps — when it is unavailable the metric reports `-1f` rather than
 * inventing a value.
 */
class DeviceMetrics(private val context: Context) : DeviceMetricsSource {

    data class Snapshot(
        val totalRamBytes: Long,
        val freeRamBytes: Long,
        val batteryPercentage: Float,
        val cpuTempCelsius: Float,
    )

    override fun snapshot(): Snapshot {
        val activityManager =
            context.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
        val memoryInfo = ActivityManager.MemoryInfo().also { activityManager.getMemoryInfo(it) }
        return Snapshot(
            totalRamBytes = memoryInfo.totalMem,
            freeRamBytes = memoryInfo.availMem,
            batteryPercentage = readBatteryPercentage(),
            cpuTempCelsius = readCpuTemperature(),
        )
    }

    private fun readBatteryPercentage(): Float {
        val batteryManager =
            context.getSystemService(Context.BATTERY_SERVICE) as BatteryManager
        val capacity = batteryManager.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        return if (capacity in 0..100) capacity.toFloat() else UNKNOWN
    }

    private fun readCpuTemperature(): Float {
        val zones = File(THERMAL_ROOT).listFiles { file -> file.name.startsWith("thermal_zone") }
            ?: return UNKNOWN
        for (zone in zones) {
            val celsius = runCatching {
                val millis = File(zone, "temp").readText().trim().toFloat()
                if (millis > 1000f) millis / 1000f else millis
            }.getOrNull() ?: continue
            return celsius
        }
        return UNKNOWN
    }

    private companion object {
        const val THERMAL_ROOT = "/sys/class/thermal"
        const val UNKNOWN = -1f
    }
}
