# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# Keep JNA and the wispers-connect bridge: JNA resolves callback classes and
# native method mappings reflectively, so R8 must not strip or rename them.
# (Same rules as the Files app. The WebView @JavascriptInterface bridge is
# already kept by the default Android rules.)
-keep class com.sun.jna.** { *; }
-keep class dev.wispers.connect.** { *; }
-dontwarn com.sun.jna.**

# Readable release stack traces.
-keepattributes SourceFile,LineNumberTable

# JVM-only / compile-only references that don't exist on Android; the code
# paths are never taken there (Ktor's IDE-debugger probe, tink's annotations).
-dontwarn java.lang.management.**
-dontwarn com.google.errorprone.annotations.**