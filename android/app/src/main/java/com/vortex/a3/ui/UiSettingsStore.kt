package com.vortex.a3.ui

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** UI theme. Dark by default — matches the Linux side's default. */
enum class ThemeMode(val code: String) {
    Dark("dark"), Light("light");
    companion object {
        fun fromCode(c: String?): ThemeMode = if (c == "light") Light else Dark
    }
}

/**
 * Persisted per-device UI preferences (locale + theme), pulled out of
 * MainActivity so the activity stays focused on lifecycle + wiring.
 *
 * Backed by the shared `vortex_ui_settings` prefs. Exposes observable
 * [locale] / [theme] the Compose tree collects, plus setters that persist
 * with a change-timestamp (so cross-device last-writer-wins can adopt or
 * override). These stay device-local — they are intentionally NOT synced
 * over the wire (see VortexService's AppState provider).
 */
class UiSettingsStore(context: Context) {
    private val prefs =
        context.getSharedPreferences("vortex_ui_settings", Context.MODE_PRIVATE)

    private val _locale = MutableStateFlow(VortexLocale.English)
    val locale: StateFlow<VortexLocale> = _locale.asStateFlow()

    private val _theme = MutableStateFlow(ThemeMode.Dark)
    val theme: StateFlow<ThemeMode> = _theme.asStateFlow()

    /** Seed [locale] + [theme] from prefs. Call once at startup. */
    fun load() {
        // Locale priority (matches the Tauri side):
        //   1. explicit override in prefs, 2. system locale, 3. English.
        val code = prefs.getString("locale", null)
        _locale.value = when {
            code != null -> VortexLocale.fromCode(code)
            else -> {
                val sys = java.util.Locale.getDefault().language.lowercase()
                when {
                    sys.startsWith("uz") -> VortexLocale.Uzbek
                    sys.startsWith("ru") -> VortexLocale.Russian
                    else -> VortexLocale.English
                }
            }
        }
        _theme.value = ThemeMode.fromCode(prefs.getString("theme", null))
    }

    fun setLocale(loc: VortexLocale) {
        prefs.edit()
            .putString("locale", loc.code)
            .putLong("locale_changed_at", nowSec())
            .apply()
        _locale.value = loc
    }

    fun setTheme(mode: ThemeMode) {
        prefs.edit()
            .putString("theme", mode.code)
            .putLong("theme_changed_at", nowSec())
            .apply()
        _theme.value = mode
    }

    private fun nowSec() = System.currentTimeMillis() / 1000L
}
