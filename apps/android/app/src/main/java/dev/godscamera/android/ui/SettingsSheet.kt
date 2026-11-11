package dev.godscamera.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.godscamera.android.BuildConfig
import dev.godscamera.android.R

/** The few settings there are: the Mac this device is connected to, and a way to let go of it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsSheet(macName: String?, coreVersion: String, onConnect: () -> Unit, onForget: () -> Unit, onClose: () -> Unit) {
    var confirming by remember { mutableStateOf(false) }
    ModalBottomSheet(onDismissRequest = onClose, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp).padding(bottom = 32.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Text(stringResource(R.string.settings), style = MaterialTheme.typography.headlineSmall)
            Text(stringResource(R.string.settings_mac), style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.primary)
            if (macName == null) {
                Text(stringResource(R.string.settings_no_mac), style = MaterialTheme.typography.bodyLarge)
                Button(onClick = onConnect) { Text(stringResource(R.string.connect_to_mac)) }
            } else {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    Icon(AppIcons.Mac, contentDescription = null, modifier = Modifier.size(32.dp))
                    Text(macName, style = MaterialTheme.typography.titleLarge)
                }
                OutlinedButton(onClick = { confirming = true }, colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error)) {
                    Text(stringResource(R.string.forget_mac))
                }
            }
            Text(
                stringResource(R.string.about, BuildConfig.VERSION_NAME, coreVersion),
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    if (confirming && macName != null) {
        AlertDialog(
            onDismissRequest = { confirming = false },
            title = { Text(stringResource(R.string.forget_title, macName)) },
            text = { Text(stringResource(R.string.forget_body)) },
            confirmButton = {
                TextButton(onClick = {
                    confirming = false
                    onForget()
                }) { Text(stringResource(R.string.forget_confirm), color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { confirming = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}
