package dev.godscamera.android

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
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.IntentCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.google.mlkit.vision.documentscanner.GmsDocumentScannerOptions
import com.google.mlkit.vision.documentscanner.GmsDocumentScanning
import com.google.mlkit.vision.documentscanner.GmsDocumentScanningResult
import dev.godscamera.android.ui.AppIcons
import dev.godscamera.android.ui.ConnectScreen
import dev.godscamera.android.ui.GodsCameraTheme
import dev.godscamera.android.ui.HomeScreen
import dev.godscamera.android.ui.RemoteScreen
import dev.godscamera.android.ui.SettingsSheet

class MainActivity : ComponentActivity() {
    private val model: ScanViewModel by viewModels()
    private val remote: RemoteViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // Only at a fresh start. When the screen is turned, Android makes the
        // activity anew with the same intent, and what it carried was already taken.
        if (savedInstanceState == null) takeIntent(intent)
        // The Remote tab reaches the computer the scanner is connected to.
        remote.attach(model::hubClient)
        setContent {
            GodsCameraTheme {
                Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surface) { App(model, remote) }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        model.setInFront(true)
    }

    override fun onStop() {
        model.setInFront(false)
        super.onStop()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        takeIntent(intent)
    }

    /** Pictures shared to the app from another app become pages. */
    private fun takeIntent(intent: Intent?) {
        if (BuildConfig.DEBUG) takeDevelopmentExtras(intent)
        val shared: List<Uri> = when (intent?.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> emptyList()
        }
        model.addPages(shared)
    }

    /**
     * Development only: ways to drive the app from a computer through `adb`,
     * so a check can run without touching the screen or anyone's photos. A
     * release build has none of this, or any app on the device could point
     * the scanner at a hub of its own.
     */
    private fun takeDevelopmentExtras(intent: Intent?) {
        intent ?: return
        // The hub's token, handed over through the debugging connection.
        intent.getStringExtra("hub_token")?.takeIf { it.length >= 16 }?.let(model::useDevelopmentHub)
        // Forget the Mac this device is connected to.
        if (intent.getBooleanExtra("forget_mac", false)) model.forgetMac()
        // Connect with a code, as if the Mac had been chosen and the code typed.
        val address = intent.getStringExtra("pair_address")
        val code = intent.getStringExtra("pair_code")
        if (address != null && code != null) model.debugPair(address, code)
        // The pictures in a folder of the app's own storage, as pages.
        intent.getStringExtra("debug_pages")?.takeIf { name -> name.all { it.isLetterOrDigit() || it == '-' } }?.let { name ->
            val pictures = java.io.File(filesDir, name).listFiles { file -> file.extension.lowercase() in setOf("jpg", "jpeg", "png") }
            model.addPages(pictures.orEmpty().sortedBy { it.name }.map(Uri::fromFile))
        }
        // Send the pages as soon as a Mac is there to take them.
        if (intent.getBooleanExtra("debug_send", false)) model.debugSendWhenReady()
        // Show one screen, for pictures of the app.
        intent.getStringExtra("debug_screen")?.let { screen ->
            when (screen) {
                // A made-up computer's camera: nothing is asked of a real one.
                "remote" -> remote.showSample()
                else -> {
                    remote.tab.value = Tab.Scan
                    model.debugShow(screen)
                }
            }
        }
    }
}

@Composable
fun App(model: ScanViewModel, remote: RemoteViewModel) {
    val state by model.state.collectAsStateWithLifecycle()
    val remoteUi by remote.ui.collectAsStateWithLifecycle()
    val tab by remote.tab.collectAsStateWithLifecycle()
    val context = LocalContext.current

    val choosePhotos = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(50)) { uris ->
        model.addPages(uris)
    }
    val scanDocument = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { result ->
        val pages = GmsDocumentScanningResult.fromActivityResultIntent(result.data)?.pages.orEmpty()
        model.addPages(pages.map { it.imageUri })
    }

    if (state.pairing != PairFlow.Hidden) {
        ConnectScreen(state, model)
    } else {
        // Two tabs at the foot: the computer's camera, and the scanner.
        Scaffold(
            contentWindowInsets = WindowInsets(0, 0, 0, 0),
            bottomBar = {
                NavigationBar {
                    NavigationBarItem(
                        selected = tab == Tab.Remote,
                        onClick = { remote.tab.value = Tab.Remote },
                        icon = { Icon(AppIcons.Live, contentDescription = null) },
                        label = { Text(stringResource(R.string.tab_remote)) },
                    )
                    NavigationBarItem(
                        selected = tab == Tab.Scan,
                        onClick = { remote.tab.value = Tab.Scan },
                        icon = { Icon(AppIcons.Scan, contentDescription = null) },
                        label = { Text(stringResource(R.string.tab_scan)) },
                    )
                }
            },
        ) { padding ->
            Box(modifier = Modifier.padding(bottom = padding.calculateBottomPadding())) {
                when (tab) {
                    Tab.Remote -> RemoteScreen(remoteUi, remote, onConnect = model::openConnect, onSettings = model::openSettings)
                    Tab.Scan -> HomeScreen(
                        state = state,
                        model = model,
                        onScan = {
                            val options = GmsDocumentScannerOptions.Builder()
                                .setGalleryImportAllowed(true)
                                .setResultFormats(GmsDocumentScannerOptions.RESULT_FORMAT_JPEG)
                                .setScannerMode(GmsDocumentScannerOptions.SCANNER_MODE_FULL)
                                .build()
                            GmsDocumentScanning.getClient(options).getStartScanIntent(context as ComponentActivity)
                                .addOnSuccessListener { scanDocument.launch(IntentSenderRequest.Builder(it).build()) }
                                .addOnFailureListener { Toast.makeText(context, R.string.scanner_unavailable, Toast.LENGTH_LONG).show() }
                        },
                        onChoosePhotos = { choosePhotos.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
                    )
                }
            }
        }
    }
    if (state.settingsOpen) {
        val coreVersion = remember { runCatching { uniffi.gc_mobile.coreVersion() }.getOrDefault("?") }
        SettingsSheet(
            macName = state.connection.macName,
            coreVersion = coreVersion,
            onConnect = model::openConnect,
            onForget = model::forgetMac,
            onClose = model::closeSettings,
        )
    }
}
