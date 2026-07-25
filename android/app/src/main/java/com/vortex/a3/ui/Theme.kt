package com.vortex.a3.ui

import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.ui.graphics.Color

// ---- Vortex design tokens (mirrors the Tauri/Vue side: emerald primary,
//      light + dark palettes via Material 3 colorScheme) ----
//
// Single source of truth: pass either [VortexDarkColors] or
// [VortexLightColors] into MaterialTheme (see VortexRoot). Every composable
// reads colors through `MaterialTheme.colorScheme.X`, so flipping the scheme
// rerenders the whole tree automatically — no custom CompositionLocal needed.

// Brand colors — identical in both themes (same as Tauri `--primary`).
private val BrandEmerald = Color(0xFF22C55E)
private val BrandOnEmerald = Color(0xFFFFFFFF)
private val BrandRed = Color(0xFFEF4444)

internal val VortexDarkColors = darkColorScheme(
    background = Color(0xFF18181B),
    onBackground = Color(0xFFFAFAFA),
    surface = Color(0xFF1C1C1F),
    onSurface = Color(0xFFFAFAFA),
    surfaceVariant = Color(0xFF1C1C1F),
    onSurfaceVariant = Color(0xFF8A8A8E),
    outline = Color(0xFF27272A),
    primary = BrandEmerald,
    onPrimary = BrandOnEmerald,
    secondary = BrandEmerald,
    onSecondary = BrandOnEmerald,
    error = BrandRed,
    onError = BrandOnEmerald,
    tertiary = Color(0xFFFBBF24),
)

internal val VortexLightColors = lightColorScheme(
    background = Color(0xFFFAFAFA),
    onBackground = Color(0xFF18181B),
    surface = Color(0xFFFFFFFF),
    onSurface = Color(0xFF18181B),
    surfaceVariant = Color(0xFFFFFFFF),
    onSurfaceVariant = Color(0xFF67676E),
    outline = Color(0xFFE4E4E7),
    primary = BrandEmerald,
    onPrimary = BrandOnEmerald,
    secondary = BrandEmerald,
    onSecondary = BrandOnEmerald,
    error = BrandRed,
    onError = BrandOnEmerald,
    tertiary = Color(0xFFD97706),
)
