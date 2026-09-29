# Shipped to every app that depends on this module (consumerProguardFiles).
#
# The generated bindings reach the native library through JNA, which works by
# reflection: it lays out Structure subclasses from their fields, resolves
# the Library interface's methods by name against the .so's exported symbols
# and looks up the `callback` method of Callback implementations. R8 must
# therefore neither strip nor rename anything JNA touches. Without these
# rules a minified app dies on its first native call with "Structure class
# UniffiRustCallStatus has unknown or zero size".
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-dontwarn com.sun.jna.**
