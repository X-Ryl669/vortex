package com.vortex.a3.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Headphones
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight as FW
import androidx.compose.ui.unit.dp
import com.vortex.a3.core.earbuds.BluetoothDeviceRow
import com.vortex.a3.ui.PickerState
import com.vortex.a3.ui.str

/**
 * Bluetooth picker modal — shows discovered devices so the user can
 * tap one to save it as the home-screen earbuds row.
 */
@Composable
fun EarbudsPickerDialog(
    pickerState: PickerState,
    onPick: (BluetoothDeviceRow) -> Unit,
    onRescan: () -> Unit,
    onClose: () -> Unit,
) {
    val statusLine = when {
        pickerState.scanning -> str("earbuds.scanning")
        pickerState.rows.isEmpty() -> str("earbuds.scan_empty")
        else -> str("earbuds.scan_done")
    }
    AlertDialog(
        onDismissRequest = onClose,
        containerColor = MaterialTheme.colorScheme.surface,
        title = { Text(str("earbuds.add"), color = MaterialTheme.colorScheme.onSurface, fontWeight = FW.SemiBold) },
        text = {
            Column {
                Text(
                    str("earbuds.add_modal_hint"),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyMedium,
                )
                Spacer(modifier = Modifier.height(10.dp))
                Text(
                    statusLine,
                    color = if (pickerState.rows.isEmpty() && !pickerState.scanning)
                        MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.primary,
                    style = MaterialTheme.typography.bodySmall,
                    fontWeight = FW.Medium,
                )
                Spacer(modifier = Modifier.height(10.dp))
                // Cap the list height so the dialog stays usable on
                // small screens. The list itself is scrollable.
                LazyColumn(
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(260.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    items(pickerState.rows.size) { i ->
                        val row = pickerState.rows[i]
                        EarbudsPickerRow(row = row, onPick = { onPick(row) })
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onRescan, enabled = !pickerState.scanning) {
                Text(str("earbuds.rescan"), color = MaterialTheme.colorScheme.primary)
            }
        },
        dismissButton = {
            TextButton(onClick = onClose) {
                Text(str("scan.close"), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        },
    )
}

@Composable
fun EarbudsPickerRow(
    row: BluetoothDeviceRow,
    onPick: () -> Unit,
) {
    val iconTint = if (row.isAudio) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant
    val iconBg = if (row.isAudio)
        MaterialTheme.colorScheme.primary.copy(alpha = 0.15f)
    else
        MaterialTheme.colorScheme.background.copy(alpha = 0.6f)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(MaterialTheme.colorScheme.background.copy(alpha = 0.4f))
            .border(width = 1.dp, color = MaterialTheme.colorScheme.outline, shape = RoundedCornerShape(10.dp))
            .clickable { onPick() }
            .padding(horizontal = 10.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(
            modifier = Modifier
                .size(34.dp)
                .clip(RoundedCornerShape(8.dp))
                .background(iconBg),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = Icons.Outlined.Headphones,
                contentDescription = null,
                tint = iconTint,
                modifier = Modifier.size(18.dp),
            )
        }
        Column(modifier = Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    row.name,
                    color = MaterialTheme.colorScheme.onSurface,
                    fontWeight = FW.Medium,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                )
                if (row.connected) {
                    Box(
                        modifier = Modifier
                            .size(6.dp)
                            .clip(CircleShape)
                            .background(MaterialTheme.colorScheme.primary),
                    )
                }
            }
            val sub = buildString {
                append(row.address)
                if (row.connected) append(" • ").append(str("earbuds.now_connected"))
                row.rssi?.let { append(" • ").append(it).append(" dBm") }
            }
            Text(
                sub,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall,
                fontFamily = FontFamily.Monospace,
                maxLines = 1,
            )
        }
    }
}
