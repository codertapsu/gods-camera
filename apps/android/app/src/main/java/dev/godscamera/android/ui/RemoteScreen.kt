package dev.godscamera.android.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Face
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.repeatOnLifecycle
import dev.godscamera.android.R
import dev.godscamera.android.Reach
import dev.godscamera.android.RemoteUi
import dev.godscamera.android.RemoteViewModel
import dev.godscamera.android.remoteMessage

/**
 * The Remote tab: the computer's Camera screen in the hand. The picture
 * others see, and under it the controls of a call: the face swap, recording
 * a loop, and playing it or going back to live; then the faces to choose
 * from. The camera, the loop and the swap are used together here as they
 * are on the computer (plan, section 6.12).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RemoteScreen(ui: RemoteUi, model: RemoteViewModel, onConnect: () -> Unit, onSettings: () -> Unit) {
    // The computer is asked how things stand only while this is in view and the app is in front.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(lifecycle, model) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) { model.follow() }
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.remote_title), fontWeight = FontWeight.SemiBold) },
                actions = {
                    IconButton(onClick = onSettings) { Icon(Icons.Default.Settings, contentDescription = stringResource(R.string.settings)) }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.surface),
            )
        },
    ) { padding ->
        Box(modifier = Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(
                modifier = Modifier.widthIn(max = 760.dp).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 24.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                when (ui.reach) {
                    Reach.NotConnected -> Notice(stringResource(R.string.remote_not_connected), stringResource(R.string.connect_to_mac), onConnect)
                    Reach.NotOnThisComputer -> Notice(stringResource(R.string.remote_not_here), null, null)
                    Reach.Unreachable -> Notice(stringResource(R.string.remote_unreachable), null, null)
                    Reach.Unknown, Reach.Available -> {}
                }
                Picture(ui)
                Controls(ui, model)
                Status(ui, model)
                Faces(ui, model)
            }
        }
    }
}

@Composable
private fun Notice(words: String, action: String?, onAction: (() -> Unit)?) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer)) {
        Column(modifier = Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(words, style = MaterialTheme.typography.bodyLarge)
            if (action != null && onAction != null) Button(onClick = onAction) { Text(action) }
        }
    }
}

/** The picture others see, with what is happening drawn on it. */
@Composable
private fun Picture(ui: RemoteUi) {
    val state = ui.state
    Box(
        modifier = Modifier.fillMaxWidth().aspectRatio(16f / 9f).clip(MaterialTheme.shapes.large).background(Color.Black),
        contentAlignment = Alignment.Center,
    ) {
        val picture = ui.picture
        if (picture != null) {
            Image(picture, contentDescription = stringResource(R.string.remote_picture), contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        } else {
            Text(stringResource(R.string.remote_no_picture), color = Color.White.copy(alpha = 0.8f), textAlign = TextAlign.Center, modifier = Modifier.padding(24.dp))
        }
        if (state != null) {
            Row(modifier = Modifier.align(Alignment.TopStart).padding(10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                when (state.mode) {
                    "looping" -> Chip(stringResource(R.string.chip_looping), Color(0xFFD13438))
                    "live" -> Chip(stringResource(R.string.chip_live), Color(0xFF1E9E4A))
                    else -> Chip(stringResource(R.string.chip_no_picture), Color(0xFF707070))
                }
                if (state.swapping && state.mode == "live") Chip(stringResource(R.string.chip_ai_face), Color(0xFF8A4FD8))
                if (state.recording) Chip(stringResource(R.string.chip_recording, state.recorded_seconds.toInt()), Color(0xFFE07A10))
            }
            if (state.watched) {
                Text(
                    stringResource(R.string.remote_watched),
                    color = Color.White,
                    style = MaterialTheme.typography.labelMedium,
                    modifier = Modifier.align(Alignment.BottomEnd).padding(10.dp).background(Color.Black.copy(alpha = 0.45f), CircleShape).padding(horizontal = 10.dp, vertical = 4.dp),
                )
            }
        }
    }
}

@Composable
private fun Chip(words: String, colour: Color) {
    Text(
        words,
        color = Color.White,
        style = MaterialTheme.typography.labelSmall,
        fontWeight = FontWeight.Bold,
        modifier = Modifier.background(colour.copy(alpha = 0.92f), CircleShape).padding(horizontal = 9.dp, vertical = 4.dp),
    )
}

/** The three controls of a call, large enough for a thumb. */
@Composable
private fun Controls(ui: RemoteUi, model: RemoteViewModel) {
    val state = ui.state
    val ready = ui.reach == Reach.Available && state != null && !ui.asking
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.fillMaxWidth()) {
        val swapOn = state?.swap_wanted == true
        val chosen = ui.faces.firstOrNull { it.id == state?.selected_face }
        val swapColours = if (swapOn) ButtonDefaults.buttonColors() else ButtonDefaults.filledTonalButtonColors()
        Button(
            onClick = { model.swap(!swapOn) },
            enabled = ready,
            colors = swapColours,
            modifier = Modifier.weight(1f).height(64.dp),
            shape = MaterialTheme.shapes.medium,
        ) {
            val avatar = chosen?.let { ui.facePictures[it.id] }
            if (avatar != null) {
                Image(avatar, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.size(28.dp).clip(CircleShape))
            } else {
                Icon(Icons.Default.Face, contentDescription = null)
            }
            Spacer(Modifier.width(8.dp))
            Text(stringResource(R.string.remote_face_swap), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        val recording = state?.recording == true
        Button(
            onClick = model::record,
            enabled = ready && (state?.can_record == true || recording),
            colors = if (recording) ButtonDefaults.buttonColors(containerColor = Color(0xFFD13438), contentColor = Color.White) else ButtonDefaults.filledTonalButtonColors(),
            modifier = Modifier.weight(1f).height(64.dp),
            shape = MaterialTheme.shapes.medium,
        ) {
            Icon(if (recording) AppIcons.Stop else AppIcons.Record, contentDescription = null)
            Spacer(Modifier.width(8.dp))
            Text(stringResource(if (recording) R.string.remote_stop else R.string.remote_record), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        val looping = state?.looping == true
        FilledTonalButton(
            onClick = model::loop,
            enabled = ready && state?.can_loop == true,
            modifier = Modifier.weight(1f).height(64.dp),
            shape = MaterialTheme.shapes.medium,
        ) {
            Icon(if (looping) AppIcons.Live else Icons.Default.PlayArrow, contentDescription = null)
            Spacer(Modifier.width(8.dp))
            Text(stringResource(if (looping) R.string.remote_back_to_live else R.string.remote_play), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** One line: what went wrong with the last thing asked, or why the swap is not on the face. */
@Composable
private fun Status(ui: RemoteUi, model: RemoteViewModel) {
    val message = ui.message
    val problem = ui.state?.swap_problem?.let(::remoteMessage)
    when {
        message != null -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(Icons.Default.Warning, contentDescription = null, tint = MaterialTheme.colorScheme.error)
            Text(
                ui.messageDetail?.let { stringResource(message, it) } ?: stringResource(message),
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = model::dismissMessage) { Text(stringResource(R.string.done)) }
        }
        problem != null -> Text(stringResource(problem), color = MaterialTheme.colorScheme.onSurfaceVariant)
        ui.state?.swapping == true -> Text(stringResource(R.string.remote_swapping), color = MaterialTheme.colorScheme.onSurfaceVariant)
        else -> {}
    }
}

/** The faces on the computer, to choose the one shown. */
@Composable
private fun Faces(ui: RemoteUi, model: RemoteViewModel) {
    if (ui.reach != Reach.Available) return
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(stringResource(R.string.remote_faces), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        if (ui.faces.isEmpty()) {
            Text(stringResource(R.string.remote_no_faces), color = MaterialTheme.colorScheme.onSurfaceVariant)
            return
        }
        LazyRow(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            items(ui.faces, key = { it.id }) { face ->
                val chosen = face.id == ui.state?.selected_face
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    modifier = Modifier.width(88.dp).clip(RoundedCornerShape(16.dp)).clickable(enabled = !ui.asking) { model.chooseFace(face.id) }.padding(6.dp),
                ) {
                    val picture = ui.facePictures[face.id]
                    val ring = if (chosen) MaterialTheme.colorScheme.primary else Color.Transparent
                    Box(modifier = Modifier.size(68.dp).border(3.dp, ring, CircleShape).padding(5.dp).clip(CircleShape).background(MaterialTheme.colorScheme.surfaceVariant)) {
                        if (picture != null) Image(picture, contentDescription = null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
                    }
                    Spacer(Modifier.height(6.dp))
                    Text(face.name, style = MaterialTheme.typography.labelMedium, maxLines = 2, textAlign = TextAlign.Center, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}
