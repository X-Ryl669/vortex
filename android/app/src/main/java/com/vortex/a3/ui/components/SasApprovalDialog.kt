package com.vortex.a3.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight as FW
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.vortex.a3.core.pairing.PairingOrchestrator
import com.vortex.a3.core.pairing.SasEmoji
import com.vortex.a3.ui.str

/**
 * Manual SAS-approval dialog shown when the peer side has accepted
 * an inbound pairing request. The user compares the six-digit code
 * shown here with the one displayed on the laptop's Vortex UI and
 * approves only if they match.
 */
@Composable
fun SasApprovalDialog(
    outcome: PairingOrchestrator.HandshakeOutcome,
    onApprove: () -> Unit,
    onReject: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = { /* explicit decision required */ },
        containerColor = MaterialTheme.colorScheme.surface,
        title = {
            Text(str("sas.title"), color = MaterialTheme.colorScheme.onSurface, fontWeight = FW.SemiBold)
        },
        text = {
            Column {
                Text(
                    str("sas.body"),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyMedium,
                )
                Spacer(modifier = Modifier.height(20.dp))
                // The 6-digit SAS rendered as 3 emoji + names — identical mapping
                // to the laptop, so the user just checks the same three appear on
                // both screens (friendlier + safer than comparing digits).
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterHorizontally),
                ) {
                    SasEmoji.glyphs(outcome.sasString).forEach { g ->
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Box(
                                modifier = Modifier
                                    .size(78.dp, 84.dp)
                                    .background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(18.dp))
                                    .border(1.dp, MaterialTheme.colorScheme.outline, RoundedCornerShape(18.dp)),
                                contentAlignment = Alignment.Center,
                            ) {
                                Text(g.emoji, fontSize = 38.sp)
                            }
                            Spacer(modifier = Modifier.height(7.dp))
                            Text(
                                g.name,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                style = MaterialTheme.typography.labelMedium,
                                fontWeight = FW.SemiBold,
                            )
                        }
                    }
                }
            }
        },
        confirmButton = {
            Button(
                onClick = onApprove,
                colors = ButtonDefaults.buttonColors(
                    containerColor = MaterialTheme.colorScheme.primary,
                    contentColor = MaterialTheme.colorScheme.onPrimary,
                ),
            ) { Text(str("sas.approve"), fontWeight = FW.Medium) }
        },
        dismissButton = {
            TextButton(onClick = onReject) {
                Text(str("sas.reject"), color = MaterialTheme.colorScheme.error)
            }
        },
    )
}
