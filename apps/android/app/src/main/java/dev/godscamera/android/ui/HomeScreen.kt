package dev.godscamera.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.util.Size
import android.widget.Toast
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.godscamera.android.Connection
import dev.godscamera.android.Link
import dev.godscamera.android.Phase
import dev.godscamera.android.R
import dev.godscamera.android.ScanState
import dev.godscamera.android.ScanViewModel
import dev.godscamera.android.SendProblem
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The first screen: the connection to the Mac, the two ways to add pages, and
 * the pages in hand with what becomes of them. On a tablet held sideways the
 * pages get a pane of their own; on a phone everything is one column.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(state: ScanState, model: ScanViewModel, onScan: () -> Unit, onChoosePhotos: () -> Unit) {
    // Wide enough for two panes: a tablet held sideways, or a large window.
    val wide = with(LocalDensity.current) { LocalWindowInfo.current.containerSize.width.toDp() } >= 840.dp
    // Pages are in hand and nothing is under way: the next thing to do is to send them.
    val readyToSend = state.pages.isNotEmpty() && state.phase is Phase.Idle
    Scaffold(
        // On a phone the pages run off the bottom of the screen, so the button
        // that sends them stays put at the bottom instead of following them.
        bottomBar = {
            if (!wide && readyToSend) {
                Surface(color = MaterialTheme.colorScheme.surface, shadowElevation = 8.dp) {
                    Column(modifier = Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 16.dp, vertical = 12.dp)) {
                        SendButton(state, model)
                    }
                }
            }
        },
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.app_name), fontWeight = FontWeight.SemiBold) },
                actions = {
                    IconButton(onClick = model::openSettings) {
                        Icon(Icons.Default.Settings, contentDescription = stringResource(R.string.settings))
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.surface),
            )
        },
    ) { padding ->
        Box(modifier = Modifier.padding(padding).fillMaxSize()) {
            if (wide) {
                Row(modifier = Modifier.fillMaxSize().padding(horizontal = 24.dp), horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                    Column(
                        modifier = Modifier.width(400.dp).fillMaxHeight().verticalScroll(rememberScrollState()).padding(bottom = 24.dp),
                        verticalArrangement = Arrangement.spacedBy(16.dp),
                    ) {
                        ConnectionCard(state.connection, model)
                        AddPagesCard(busy = state.phase.isBusy, onScan = onScan, onChoosePhotos = onChoosePhotos)
                    }
                    Surface(
                        modifier = Modifier.weight(1f).fillMaxHeight().padding(bottom = 24.dp),
                        shape = MaterialTheme.shapes.large,
                        color = MaterialTheme.colorScheme.surfaceContainerLow,
                    ) {
                        Column(modifier = Modifier.fillMaxSize()) {
                            Column(modifier = Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(24.dp)) {
                                PagesPane(state, model)
                            }
                            // The button that sends the pages stays at the foot of the pane, however many pages there are.
                            if (readyToSend) {
                                Column(modifier = Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = 24.dp)) {
                                    SendButton(state, model)
                                }
                            }
                        }
                    }
                }
            } else {
                Column(
                    modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    ConnectionCard(state.connection, model)
                    AddPagesCard(busy = state.phase.isBusy, onScan = onScan, onChoosePhotos = onChoosePhotos)
                    PagesPane(state, model)
                }
            }
        }
    }
}

// --- The connection ---

/** How this device stands with the Mac, with the one thing to do about it. */
@Composable
private fun ConnectionCard(connection: Connection, model: ScanViewModel) {
    val colours = MaterialTheme.colorScheme
    val good = connection.canSend
    Card(
        modifier = Modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.large,
        colors = CardDefaults.cardColors(containerColor = if (good) colours.primaryContainer else colours.surfaceContainerHigh),
    ) {
        Column(modifier = Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                Box(
                    modifier = Modifier.size(48.dp).clip(CircleShape).background(if (good) colours.primary else colours.surfaceContainerHighest),
                    contentAlignment = Alignment.Center,
                ) {
                    when (connection) {
                        is Connection.Searching -> CircularProgressIndicator(modifier = Modifier.size(24.dp), strokeWidth = 3.dp)
                        is Connection.NotFound, is Connection.Refused ->
                            Icon(Icons.Default.Warning, contentDescription = null, tint = colours.error)
                        else -> Icon(AppIcons.Mac, contentDescription = null, tint = if (good) colours.onPrimary else colours.onSurfaceVariant)
                    }
                }
                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(connectionTitle(connection), style = MaterialTheme.typography.titleLarge)
                    Text(
                        connectionBody(connection), style = MaterialTheme.typography.bodyMedium,
                        color = if (good) colours.onPrimaryContainer else colours.onSurfaceVariant,
                    )
                }
            }
            when (connection) {
                Connection.NotPaired ->
                    Button(onClick = model::openConnect, modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp)) {
                        Text(stringResource(R.string.connect_to_mac), style = MaterialTheme.typography.titleMedium)
                    }
                is Connection.NotFound ->
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilledTonalButton(onClick = model::retryConnection) { Text(stringResource(R.string.try_again)) }
                        TextButton(onClick = model::openConnect) { Text(stringResource(R.string.connect_another)) }
                    }
                is Connection.Refused ->
                    Button(onClick = model::openConnect) { Text(stringResource(R.string.connect_again)) }
                is Connection.Development ->
                    if (connection.link != Link.Connected) {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            FilledTonalButton(onClick = model::retryConnection) { Text(stringResource(R.string.try_again)) }
                            TextButton(onClick = model::openConnect) { Text(stringResource(R.string.connect_to_mac)) }
                        }
                    }
                else -> {}
            }
        }
    }
}

@Composable
private fun connectionTitle(connection: Connection): String = when (connection) {
    Connection.NotPaired -> stringResource(R.string.not_paired_title)
    is Connection.Searching -> stringResource(R.string.searching_title, connection.mac)
    is Connection.Connected -> stringResource(R.string.connected_title, connection.mac)
    is Connection.NotFound -> stringResource(R.string.not_found_title, connection.mac)
    is Connection.Refused -> stringResource(R.string.refused_title, connection.mac)
    is Connection.Development -> stringResource(R.string.dev_title)
}

@Composable
private fun connectionBody(connection: Connection): String = when (connection) {
    Connection.NotPaired -> stringResource(R.string.not_paired_body)
    is Connection.Searching -> stringResource(R.string.searching_body)
    is Connection.Connected -> stringResource(R.string.connected_body)
    is Connection.NotFound -> stringResource(R.string.not_found_body)
    is Connection.Refused -> stringResource(R.string.refused_body)
    is Connection.Development -> stringResource(
        when (connection.link) {
            Link.Connected -> R.string.dev_connected
            Link.WrongToken -> R.string.dev_wrong_token
            Link.Unreachable -> R.string.dev_unreachable
            Link.Unknown -> R.string.dev_unknown
        }
    )
}

// --- Adding pages ---

@Composable
private fun AddPagesCard(busy: Boolean, onScan: () -> Unit, onChoosePhotos: () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(stringResource(R.string.add_pages), style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(start = 4.dp))
        BigAction(
            icon = AppIcons.Scan, title = stringResource(R.string.scan_document),
            hint = stringResource(R.string.scan_document_hint), primary = true, enabled = !busy, onClick = onScan,
        )
        BigAction(
            icon = AppIcons.Photos, title = stringResource(R.string.choose_photos),
            hint = stringResource(R.string.choose_photos_hint), primary = false, enabled = !busy, onClick = onChoosePhotos,
        )
    }
}

/** A large button with a picture, a name and a line that says what it does. */
@Composable
private fun BigAction(icon: ImageVector, title: String, hint: String, primary: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val colours = MaterialTheme.colorScheme
    Surface(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.fillMaxWidth().alpha(if (enabled) 1f else 0.5f),
        shape = MaterialTheme.shapes.large,
        color = if (primary) colours.primary else colours.secondaryContainer,
        contentColor = if (primary) colours.onPrimary else colours.onSecondaryContainer,
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 20.dp, vertical = 18.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Icon(icon, contentDescription = null, modifier = Modifier.size(34.dp))
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(title, style = MaterialTheme.typography.titleLarge)
                Text(hint, style = MaterialTheme.typography.bodyMedium)
            }
        }
    }
}

// --- The pages, and what becomes of them ---

@Composable
private fun PagesPane(state: ScanState, model: ScanViewModel) {
    when (val phase = state.phase) {
        is Phase.Done -> ResultView(phase, onScanAnother = model::clearPages)
        is Phase.Failed -> ProblemView(phase, canSend = state.connection.canSend, model = model)
        else -> PagesView(state, model)
    }
}

/** The large button that sends the pages, with a line under it when there is no Mac to send them to. */
@Composable
private fun SendButton(state: ScanState, model: ScanViewModel) {
    Button(
        onClick = model::send, enabled = state.connection.canSend,
        modifier = Modifier.fillMaxWidth().heightIn(min = 60.dp), shape = MaterialTheme.shapes.large,
    ) {
        Icon(Icons.AutoMirrored.Filled.Send, contentDescription = null)
        Spacer(Modifier.width(12.dp))
        Text(stringResource(R.string.send_to_mac), style = MaterialTheme.typography.titleLarge)
    }
    if (!state.connection.canSend) {
        Text(
            stringResource(R.string.send_needs_mac), style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.fillMaxWidth().padding(top = 8.dp), textAlign = TextAlign.Center,
        )
    }
}

/** The pages as rows of small pictures that fill the width, however wide it is. */
@Composable
private fun PagesGrid(pages: List<Uri>, removable: Boolean, onRemove: (Int) -> Unit) {
    BoxWithConstraints(modifier = Modifier.fillMaxWidth()) {
        val gap = 12.dp
        val columns = ((maxWidth + gap) / (104.dp + gap)).toInt().coerceIn(2, 8)
        Column(verticalArrangement = Arrangement.spacedBy(gap)) {
            pages.indices.chunked(columns).forEach { row ->
                Row(horizontalArrangement = Arrangement.spacedBy(gap)) {
                    row.forEach { index ->
                        PageTile(
                            uri = pages[index], number = index + 1, removable = removable, onRemove = { onRemove(index) },
                            modifier = Modifier.weight(1f),
                        )
                    }
                    // A short last row keeps its pictures the same size as the rows above.
                    repeat(columns - row.size) { Spacer(Modifier.weight(1f)) }
                }
            }
        }
    }
}

@Composable
private fun PagesView(state: ScanState, model: ScanViewModel) {
    val busy = state.phase.isBusy
    if (state.pages.isEmpty()) {
        Column(
            modifier = Modifier.fillMaxWidth().padding(vertical = 40.dp),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Icon(AppIcons.Page, contentDescription = null, modifier = Modifier.size(64.dp), tint = MaterialTheme.colorScheme.outline)
            Text(stringResource(R.string.no_pages_title), style = MaterialTheme.typography.headlineSmall)
            Text(
                stringResource(R.string.no_pages_body), style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center,
                color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.widthIn(max = 460.dp),
            )
        }
        return
    }
    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(modifier = Modifier.weight(1f)) {
                Text(stringResource(R.string.pages), style = MaterialTheme.typography.headlineSmall)
                Text(
                    pluralStringResource(R.plurals.page_count, state.pages.size, state.pages.size),
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            if (!busy) TextButton(onClick = model::clearPages) { Text(stringResource(R.string.remove_all)) }
        }
        // What is under way is shown above the pages, where it is in view however many pages there are.
        when (val phase = state.phase) {
            is Phase.Uploading -> Progress(stringResource(R.string.uploading, phase.page, phase.of), (phase.page - 1).toFloat() / phase.of)
            is Phase.Reading -> Progress(stringResource(R.string.reading, phase.percent), phase.percent / 100f)
            else -> {}
        }
        PagesGrid(state.pages, removable = !busy, onRemove = model::removePage)
    }
}

@Composable
private fun Progress(label: String, fraction: Float) {
    Card(shape = MaterialTheme.shapes.large, colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer)) {
        Column(modifier = Modifier.fillMaxWidth().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(label, style = MaterialTheme.typography.titleMedium)
            LinearProgressIndicator(
                progress = { fraction.coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth().height(8.dp),
                trackColor = MaterialTheme.colorScheme.surface,
            )
        }
    }
}

/** One page: a small picture of it, its number, and a way to take it out. */
@Composable
private fun PageTile(uri: Uri, number: Int, removable: Boolean, onRemove: () -> Unit, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val picture by produceState<ImageBitmap?>(initialValue = null, uri) {
        value = withContext(Dispatchers.IO) { loadSmallPicture(context, uri) }
    }
    Box(
        modifier = modifier.aspectRatio(3f / 4f).clip(MaterialTheme.shapes.medium)
            .background(MaterialTheme.colorScheme.surfaceContainerHighest),
    ) {
        picture?.let {
            Image(it, contentDescription = stringResource(R.string.page_number, number), contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        }
        // The marks on a picture are dark in the light and in the dark theme alike: the pictures are of paper.
        Surface(
            modifier = Modifier.align(Alignment.BottomStart).padding(8.dp), shape = CircleShape,
            color = Color.Black.copy(alpha = 0.65f), contentColor = Color.White,
        ) {
            Text("$number", style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(horizontal = 10.dp, vertical = 3.dp))
        }
        if (removable) {
            // A small mark in a corner that is easy to hit: the area that takes the tap is larger than the mark.
            Box(
                modifier = Modifier.align(Alignment.TopEnd).size(48.dp).clip(CircleShape).clickable(onClick = onRemove),
                contentAlignment = Alignment.Center,
            ) {
                Surface(
                    modifier = Modifier.size(28.dp), shape = CircleShape,
                    color = Color.Black.copy(alpha = 0.65f), contentColor = Color.White,
                ) {
                    Box(contentAlignment = Alignment.Center) {
                        Icon(Icons.Default.Close, contentDescription = stringResource(R.string.remove_page, number), modifier = Modifier.size(16.dp))
                    }
                }
            }
        }
    }
}

/** A picture small enough for a tile, or null if it cannot be read. */
private fun loadSmallPicture(context: Context, uri: Uri): ImageBitmap? {
    val side = 480
    val bitmap: Bitmap? = runCatching {
        if (uri.scheme == "content") context.contentResolver.loadThumbnail(uri, Size(side, side), null) else null
    }.getOrNull() ?: runCatching {
        // Read the size first, then decode at a fraction of it.
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
        var sample = 1
        while (bounds.outWidth / (sample * 2) >= side && bounds.outHeight / (sample * 2) >= side) sample *= 2
        val options = BitmapFactory.Options().apply { inSampleSize = sample }
        context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, options) }
    }.getOrNull()
    return bitmap?.asImageBitmap()
}

// --- The result ---

@Composable
private fun ResultView(done: Phase.Done, onScanAnother: () -> Unit) {
    val context = LocalContext.current
    val clipLabel = stringResource(R.string.result_title)
    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(Icons.Default.CheckCircle, contentDescription = null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(36.dp))
            Column {
                Text(stringResource(R.string.result_title), style = MaterialTheme.typography.headlineSmall)
                Text(
                    stringResource(R.string.result_summary, done.pictures, done.lines, done.repeated),
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        FlowRowOfActions {
            Button(onClick = {
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                clipboard.setPrimaryClip(ClipData.newPlainText(clipLabel, done.text))
                Toast.makeText(context, R.string.copied, Toast.LENGTH_SHORT).show()
            }, enabled = done.text.isNotBlank(), modifier = Modifier.heightIn(min = 52.dp)) {
                Icon(AppIcons.Copy, contentDescription = null)
                Spacer(Modifier.width(10.dp))
                Text(stringResource(R.string.copy), style = MaterialTheme.typography.titleMedium)
            }
            FilledTonalButton(onClick = {
                val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, done.text)
                context.startActivity(Intent.createChooser(send, null))
            }, enabled = done.text.isNotBlank(), modifier = Modifier.heightIn(min = 52.dp)) {
                Icon(Icons.Default.Share, contentDescription = null)
                Spacer(Modifier.width(10.dp))
                Text(stringResource(R.string.share), style = MaterialTheme.typography.titleMedium)
            }
            OutlinedButton(onClick = onScanAnother, modifier = Modifier.heightIn(min = 52.dp)) {
                Text(stringResource(R.string.scan_another), style = MaterialTheme.typography.titleMedium)
            }
        }
        Surface(shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.surface, modifier = Modifier.fillMaxWidth()) {
            SelectionContainer {
                Text(
                    done.text.ifBlank { stringResource(R.string.result_empty) },
                    style = MaterialTheme.typography.bodyLarge, modifier = Modifier.padding(20.dp),
                )
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun FlowRowOfActions(content: @Composable () -> Unit) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) { content() }
}

// --- When it did not work ---

@Composable
private fun ProblemView(failed: Phase.Failed, canSend: Boolean, model: ScanViewModel) {
    val (title, body) = when (failed.problem) {
        SendProblem.MacUnreachable -> R.string.failed_unreachable_title to R.string.failed_unreachable_body
        SendProblem.MacRefused -> R.string.failed_refused_title to R.string.failed_refused_body
        SendProblem.ReadingFailed -> R.string.failed_reading_title to R.string.failed_reading_body
    }
    Card(
        shape = MaterialTheme.shapes.large,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer, contentColor = MaterialTheme.colorScheme.onErrorContainer),
    ) {
        Column(modifier = Modifier.fillMaxWidth().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Icon(Icons.Default.Warning, contentDescription = null)
                Text(stringResource(title), style = MaterialTheme.typography.titleLarge)
            }
            Text(stringResource(body), style = MaterialTheme.typography.bodyLarge)
            failed.detail?.takeIf { it.isNotBlank() }?.let {
                Text(stringResource(R.string.failed_detail, it), style = MaterialTheme.typography.bodySmall)
            }
            FlowRowOfActions {
                when (failed.problem) {
                    SendProblem.MacRefused ->
                        Button(onClick = model::openConnect, colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error)) {
                            Text(stringResource(R.string.connect_again))
                        }
                    else ->
                        Button(
                            onClick = { if (canSend) model.send() else model.retryConnection() },
                            colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error),
                        ) { Text(stringResource(R.string.try_again)) }
                }
                TextButton(onClick = model::dismissProblem) { Text(stringResource(R.string.back_to_pages)) }
            }
        }
    }
}
