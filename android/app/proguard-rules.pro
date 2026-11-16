# Reached only from native code (JNA, JNI), which R8 cannot see.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class uniffi.flts_ffi.** { *; }
-keep, includedescriptorclasses class org.rustls.platformverifier.** { *; }
-keep class com.TS.FLTS.core.Native { *; }
-dontwarn java.awt.**
