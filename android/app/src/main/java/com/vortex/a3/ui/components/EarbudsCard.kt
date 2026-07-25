package com.vortex.a3.ui.components

import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.outlined.Headphones
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.text.font.FontWeight as FW
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.vortex.a3.core.earbuds.SwitchState
import com.vortex.a3.ui.str

/**
 * The earbuds slot on the home screen. Renders one of two layouts
 * depending on whether the user has picked a device:
 *
 *  - No saved row → `EarbudsAddPlaceholder` ("+" tile)
 *  - Saved row → the real card. Long-press opens a Remove confirm.
 *    Battery + green dot. No manual swap control — smart-switch moves
 *    the buds automatically. Card dims with "Switching…" while a
 *    flow is in flight (UI is NOT dimmed during the optimistic-done
 *    window — `isSwitching` excludes AlmostDone for that reason).
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun EarbudsCard(
    modifier: Modifier = Modifier,
    name: String?,
    battery: Int?,
    connected: Boolean,
    onLocal: Boolean,
    canRemove: Boolean,
    switchState: SwitchState,
    onOpenPicker: () -> Unit,
    onRemoveSaved: () -> Unit,
) {
    if (name == null) {
        EarbudsAddPlaceholder(modifier = modifier, onOpenPicker = onOpenPicker)
        return
    }

    var menuOpen by remember { mutableStateOf(false) }
    val isSwitching = switchState !is SwitchState.Idle &&
        switchState !is SwitchState.Failed &&
        switchState !is SwitchState.AlmostDone
    val tintColor = MaterialTheme.colorScheme.primary
    val caption = when {
        !connected -> str("earbuds.not_connected")
        onLocal -> str("earbuds.on_local")
        else -> str("earbuds.on_peer")
    }
    val cardInteraction = remember { MutableInteractionSource() }
    val pressed by cardInteraction.collectIsPressedAsState()
    val scale by animateFloatAsState(
        targetValue = if (pressed) 0.97f else 1f,
        animationSpec = spring(dampingRatio = Spring.DampingRatioMediumBouncy, stiffness = Spring.StiffnessMediumLow),
        label = "earbuds-card-press",
    )
    val contentAlpha = if (isSwitching) 0.55f else 1f
    Column(
        modifier = modifier
            .scale(scale)
            .height(CardHeight)
            .shadow(elevation = 6.dp, shape = CardCorner, clip = false)
            .clip(CardCorner)
            .background(MaterialTheme.colorScheme.surface)
            .combinedClickable(
                interactionSource = cardInteraction,
                indication = ripple(bounded = true, color = MaterialTheme.colorScheme.primary.copy(alpha = 0.4f)),
                enabled = !isSwitching,
                onClick = {},
                onLongClick = { if (canRemove) menuOpen = true },
            )
            .border(
                width = 1.dp,
                color = MaterialTheme.colorScheme.outline,
                shape = CardCorner,
            )
            .padding(16.dp),
    ) {
        CardHeader(
            icon = Icons.Outlined.Headphones,
            iconTint = tintColor,
            iconBg = MaterialTheme.colorScheme.primary.copy(alpha = 0.15f),
            statusDot = if (connected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Spacer(modifier = Modifier.height(14.dp))
        Text(
            name.takeIf { it.isNotBlank() } ?: str("device.earbuds"),
            color = MaterialTheme.colorScheme.onSurface.copy(alpha = contentAlpha),
            fontWeight = FW.SemiBold,
            style = MaterialTheme.typography.bodyLarge,
            maxLines = 1,
        )
        val captionText = when {
            isSwitching -> str("switch.in_progress")
            else -> caption
        }
        Text(
            captionText,
            color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = contentAlpha),
            style = MaterialTheme.typography.bodySmall,
            maxLines = 2,
        )
        Spacer(modifier = Modifier.weight(1f))
        BatteryRow(battery)
    }

    if (menuOpen) {
        AlertDialog(
            onDismissRequest = { menuOpen = false },
            containerColor = MaterialTheme.colorScheme.surface,
            title = { Text(str("earbuds.remove"), color = MaterialTheme.colorScheme.onSurface, fontWeight = FW.SemiBold) },
            text = {
                Text(
                    str("earbuds.remove_body"),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyMedium,
                )
            },
            confirmButton = {
                Button(
                    onClick = {
                        menuOpen = false
                        onRemoveSaved()
                    },
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.error,
                        contentColor = MaterialTheme.colorScheme.onPrimary,
                    ),
                ) { Text(str("earbuds.remove")) }
            },
            dismissButton = {
                TextButton(onClick = { menuOpen = false }) {
                    Text(str("switch.cancel"), color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            },
        )
    }
}

@Composable
fun EarbudsAddPlaceholder(
    modifier: Modifier = Modifier,
    onOpenPicker: () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scale by animateFloatAsState(
        targetValue = if (pressed) 0.97f else 1f,
        animationSpec = spring(dampingRatio = Spring.DampingRatioMediumBouncy, stiffness = Spring.StiffnessMediumLow),
        label = "earbuds-placeholder-press",
    )
    Column(
        modifier = modifier
            .scale(scale)
            .height(CardHeight)
            .shadow(elevation = 6.dp, shape = CardCorner, clip = false)
            .clip(CardCorner)
            .background(MaterialTheme.colorScheme.surface)
            .clickable(
                interactionSource = interaction,
                indication = ripple(bounded = true, color = MaterialTheme.colorScheme.primary.copy(alpha = 0.4f)),
            ) { onOpenPicker() }
            .border(width = 1.dp, color = MaterialTheme.colorScheme.outline, shape = CardCorner)
            .padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Box(
            modifier = Modifier
                .size(48.dp)
                .clip(RoundedCornerShape(14.dp))
                .background(MaterialTheme.colorScheme.primary.copy(alpha = 0.12f)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = Icons.Filled.Add,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.primary,
                modifier = Modifier.size(22.dp),
            )
        }
        Spacer(modifier = Modifier.height(8.dp))
        Text(
            str("earbuds.add"),
            color = MaterialTheme.colorScheme.onSurface,
            fontWeight = FW.SemiBold,
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(
            str("earbuds.add_hint"),
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
            textAlign = TextAlign.Center,
        )
    }
}
