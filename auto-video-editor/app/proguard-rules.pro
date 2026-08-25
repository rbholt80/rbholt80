# The engine is plain Kotlin with no reflection; the default rules cover it.

# Media3 keeps some entry points reachable only from native/GL code paths.
-keep class androidx.media3.** { *; }
-dontwarn androidx.media3.**

# Guava is pulled in transitively by Media3 and drags optional J2ObjC and
# error-prone annotations that are not on the runtime classpath.
-dontwarn com.google.common.**
-dontwarn javax.annotation.**
-dontwarn sun.misc.Unsafe
