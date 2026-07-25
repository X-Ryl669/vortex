# Vortex a3 release shrinking rules.
#
# The hard requirement here is noise-java: it instantiates DH/cipher/hash
# implementations reflectively (Noise.createDH("25519") → Class.forName),
# so R8 must keep the whole package or every handshake dies at runtime.
-keep class com.southernstorm.noise.** { *; }

# androidx.security:security-crypto rides on Tink, which registers key
# managers reflectively. Tink ships consumer rules, but keep the registry
# classes defensively — a broken EncryptedSharedPreferences would silently
# wipe the stored identity.
-keep class com.google.crypto.tink.** { *; }

# Tink references compile-only annotations (errorprone, javax.annotation)
# and an optional Google-API-client based KeysDownloader that Android
# builds never ship. They are absent at runtime by design — silence the
# missing-class errors instead of pulling in dead dependencies.
-dontwarn com.google.errorprone.annotations.**
-dontwarn javax.annotation.**
-dontwarn com.google.api.client.**
-dontwarn org.joda.time.**

# Keep crash stack traces readable in release logs.
-keepattributes SourceFile,LineNumberTable
