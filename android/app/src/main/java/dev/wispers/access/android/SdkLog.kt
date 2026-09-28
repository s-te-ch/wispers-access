package dev.wispers.access.android

import android.util.Log
import dev.wispers.access.sdk.LogLevel
import dev.wispers.access.sdk.LogSink

/** The SDK's log lines into logcat, tagged by the Rust module they came from. */
class SdkLog : LogSink {
    override fun log(level: LogLevel, target: String, message: String) {
        val tag = "sdk:" + target.substringAfterLast("::").take(20)
        when (level) {
            LogLevel.ERROR -> Log.e(tag, message)
            LogLevel.WARN -> Log.w(tag, message)
            LogLevel.INFO -> Log.i(tag, message)
            LogLevel.DEBUG, LogLevel.TRACE -> Log.d(tag, message)
        }
    }
}
