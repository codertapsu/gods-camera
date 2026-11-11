package dev.godscamera.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.core.content.IntentCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.google.mlkit.vision.documentscanner.GmsDocumentScannerOptions
import com.google.mlkit.vision.documentscanner.GmsDocumentScanning
import com.google.mlkit.vision.documentscanner.GmsDocumentScanningResult

class MainActivity : ComponentActivity() {
    private val model: ScanViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        takeShared(intent)
        setContent {
            val context = LocalContext.current
            val colours = if (isSystemInDarkTheme()) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
            MaterialTheme(colorScheme = colours) {
                Surface(modifier = Modifier.fillMaxSize()) { ScanScreen(model) }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        takeShared(intent)
    }

    /** Pictures shared to the app from another app become pages. */
    private fun takeShared(intent: Intent?) {
        // Development only: the Mac's connection script hands the hub's token over
        // through the USB cable, so it need not be typed. A release build ignores
        // this, or any app on the device could point the scanner at a hub of its own.
        if (BuildConfig.DEBUG) {
            intent?.getStringExtra("hub_token")?.takeIf { it.length >= 16 }?.let {
                model.setToken(it)
                model.checkConnection()
            }
            // Development only: take the pictures in a folder of the app's own
            // storage as pages, so a check can run without anyone's photo library.
            intent?.getStringExtra("debug_pages")?.takeIf { name -> name.all { it.isLetterOrDigit() || it == '-' } }?.let { name ->
                val pictures = java.io.File(filesDir, name).listFiles { file -> file.extension.lowercase() in setOf("jpg", "jpeg", "png") }
                model.addPages(pictures.orEmpty().sortedBy { it.name }.map(Uri::fromFile))
            }
        }
        val shared: List<Uri> = when (intent?.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> emptyList()
        }
        model.addPages(shared)
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ScanScreen(model: ScanViewModel) {
    val state by model.state.collectAsStateWithLifecycle()
    val context = LocalContext.current
    LaunchedEffect(Unit) { model.checkConnection() }

    val pickPictures = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(50)) { uris ->
        model.addPages(uris)
    }
    val scanDocument = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { result ->
        val pages = GmsDocumentScanningResult.fromActivityResultIntent(result.data)?.pages.orEmpty()
        model.addPages(pages.map { it.imageUri })
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .safeDrawingPadding()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        // On a tablet the content keeps a readable width instead of stretching.
        Column(modifier = Modifier.widthIn(max = 720.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Text(stringResource(R.string.app_name), style = MaterialTheme.typography.headlineMedium)

            Card(modifier = Modifier.fillMaxWidth()) {
                Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(R.string.connection), style = MaterialTheme.typography.titleMedium)
                    OutlinedTextField(
                        value = state.address, onValueChange = model::setAddress, singleLine = true,
                        label = { Text(stringResource(R.string.hub_address)) }, modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = state.token, onValueChange = model::setToken, singleLine = true,
                        label = { Text(stringResource(R.string.hub_token)) },
                        visualTransformation = PasswordVisualTransformation(), modifier = Modifier.fillMaxWidth(),
                    )
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        OutlinedButton(onClick = model::checkConnection) { Text(stringResource(R.string.check_connection)) }
                        Text(
                            stringResource(
                                when (state.link) {
                                    Link.Connected -> R.string.connected
                                    Link.WrongToken -> R.string.wrong_token
                                    Link.Unreachable -> R.string.cannot_reach
                                    Link.Unknown -> R.string.not_connected
                                }
                            ),
                            color = if (state.link == Link.Connected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.error,
                        )
                    }
                }
            }

            Card(modifier = Modifier.fillMaxWidth()) {
                Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(stringResource(R.string.pages), style = MaterialTheme.typography.titleMedium)
                    Text(
                        if (state.pages.isEmpty()) stringResource(R.string.no_pages)
                        else pluralStringResource(R.plurals.page_count, state.pages.size, state.pages.size)
                    )
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Button(onClick = {
                            val options = GmsDocumentScannerOptions.Builder()
                                .setGalleryImportAllowed(true)
                                .setResultFormats(GmsDocumentScannerOptions.RESULT_FORMAT_JPEG)
                                .setScannerMode(GmsDocumentScannerOptions.SCANNER_MODE_FULL)
                                .build()
                            GmsDocumentScanning.getClient(options).getStartScanIntent(context as ComponentActivity)
                                .addOnSuccessListener { scanDocument.launch(IntentSenderRequest.Builder(it).build()) }
                                .addOnFailureListener {
                                    Toast.makeText(context, R.string.scanner_unavailable, Toast.LENGTH_LONG).show()
                                }
                        }) { Text(stringResource(R.string.scan_document)) }
                        OutlinedButton(onClick = {
                            pickPictures.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                        }) { Text(stringResource(R.string.add_pictures)) }
                        if (state.pages.isNotEmpty()) {
                            TextButton(onClick = model::clearPages) { Text(stringResource(R.string.clear_pages)) }
                        }
                    }
                    val busy = state.phase is Phase.Uploading || state.phase is Phase.Reading
                    Button(onClick = model::send, enabled = state.pages.isNotEmpty() && !busy, modifier = Modifier.fillMaxWidth()) {
                        Text(stringResource(R.string.send_to_mac))
                    }
                    when (val phase = state.phase) {
                        is Phase.Uploading -> {
                            Text(stringResource(R.string.uploading, phase.page, phase.of))
                            LinearProgressIndicator(progress = { phase.page.toFloat() / phase.of }, modifier = Modifier.fillMaxWidth())
                        }
                        is Phase.Reading -> {
                            Text(stringResource(R.string.reading, phase.percent))
                            LinearProgressIndicator(progress = { phase.percent / 100f }, modifier = Modifier.fillMaxWidth())
                        }
                        is Phase.Failed -> Text(stringResource(R.string.failed, phase.reason), color = MaterialTheme.colorScheme.error)
                        else -> {}
                    }
                }
            }

            (state.phase as? Phase.Done)?.let { done ->
                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text(stringResource(R.string.result), style = MaterialTheme.typography.titleMedium)
                        Text(
                            stringResource(R.string.result_summary, done.pictures, done.lines, done.repeated),
                            style = MaterialTheme.typography.bodySmall,
                        )
                        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            OutlinedButton(onClick = {
                                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                                clipboard.setPrimaryClip(ClipData.newPlainText(context.getString(R.string.result), done.text))
                                Toast.makeText(context, R.string.copied, Toast.LENGTH_SHORT).show()
                            }) { Text(stringResource(R.string.copy)) }
                            OutlinedButton(onClick = {
                                val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, done.text)
                                context.startActivity(Intent.createChooser(send, null))
                            }) { Text(stringResource(R.string.share)) }
                            TextButton(onClick = model::clearPages) { Text(stringResource(R.string.start_over)) }
                        }
                        SelectionContainer { Text(done.text) }
                    }
                }
            }
        }
    }
}
