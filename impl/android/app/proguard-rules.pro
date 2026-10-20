# JNA and the UniFFI bindings use reflection on these; keep them if minification is enabled.
-keep class com.sun.jna.** { *; }
-keep class org.ligatureprotocol.core.** { *; }
-dontwarn java.awt.**
