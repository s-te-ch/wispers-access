# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# The keep rules for the SDK's native bridge (JNA) come with the SDK module
# itself, from sdk/android/consumer-rules.pro. The WebView @JavascriptInterface
# bridge is already kept by the default Android rules.

# Readable release stack traces.
-keepattributes SourceFile,LineNumberTable

# JVM-only / compile-only references that don't exist on Android; the code
# paths are never taken there (Ktor's IDE-debugger probe, tink's annotations).
-dontwarn java.lang.management.**
-dontwarn com.google.errorprone.annotations.**