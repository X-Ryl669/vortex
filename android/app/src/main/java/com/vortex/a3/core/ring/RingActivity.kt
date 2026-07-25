package com.vortex.a3.core.ring

import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.vortex.a3.ui.UiSettingsStore
import com.vortex.a3.ui.VortexLocale

/**
 * KDE-Connect-style full-screen "Found it!" page. Shown over the lockscreen
 * (screen forced on) while the find-my-phone ring plays; tapping ANYWHERE
 * silences the ring and closes. Reached via the ring notification's
 * full-screen intent, plus a best-effort direct start from [RingController].
 */
class RingActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        }
        RingController.attachFoundScreen(this)
        // One-shot locale read (same store the main UI uses) — this page can
        // appear on a locked phone, so it resolves language itself.
        val store = UiSettingsStore(applicationContext).apply { load() }
        val (title, button) = when (store.locale.value) {
            VortexLocale.Uzbek -> "Telefon shu yerda!" to "Topdim!"
            VortexLocale.Russian -> "Телефон здесь!" to "Нашёл!"
            else -> "Your phone is here!" to "Found it!"
        }
        setContent { FoundItScreen(title, button, onFound = ::stopAndClose) }
    }

    private fun stopAndClose() {
        RingController.stop(applicationContext)
        finish()
    }

    override fun onDestroy() {
        RingController.detachFoundScreen(this)
        super.onDestroy()
    }
}

@Composable
private fun FoundItScreen(title: String, button: String, onFound: () -> Unit) {
    // The whole screen is the button — a lost-phone tap shouldn't need aim.
    val pulse by rememberInfiniteTransition(label = "ring-pulse").animateFloat(
        initialValue = 1f,
        targetValue = 1.1f,
        animationSpec = infiniteRepeatable(
            tween(550, easing = FastOutSlowInEasing),
            RepeatMode.Reverse,
        ),
        label = "ring-pulse-scale",
    )
    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(Color(0xFF15161B))
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                onClick = onFound,
            ),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Text(title, color = Color(0xFFF2F4F6), fontSize = 22.sp)
        Spacer(Modifier.height(44.dp))
        Box(
            modifier = Modifier
                .size(264.dp)
                .scale(pulse)
                .background(Color(0xFF1AE76F), CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                button,
                color = Color(0xFF04220F),
                fontSize = 36.sp,
                fontWeight = FontWeight.Bold,
            )
        }
        Spacer(Modifier.height(44.dp))
        Text(
            "•  •  •",
            color = Color(0xFF8A8D93),
            fontSize = 16.sp,
        )
    }
}
