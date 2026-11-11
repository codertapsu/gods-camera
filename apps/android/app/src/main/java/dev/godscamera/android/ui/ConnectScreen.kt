package dev.godscamera.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.godscamera.android.CODE_LENGTH
import dev.godscamera.android.FoundMac
import dev.godscamera.android.PairFlow
import dev.godscamera.android.PairProblem
import dev.godscamera.android.R
import dev.godscamera.android.ScanState
import dev.godscamera.android.ScanViewModel

/**
 * Connecting to a Mac, one step at a time: choose the Mac, enter the code it
 * shows, done. The same three steps the Mac's own screen lists.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConnectScreen(state: ScanState, model: ScanViewModel) {
    val flow = state.pairing
    BackHandler(onBack = model::backInConnect)
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.connect_to_mac), fontWeight = FontWeight.SemiBold) },
                navigationIcon = {
                    IconButton(onClick = model::backInConnect) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = stringResource(R.string.back))
                    }
                },
            )
        },
    ) { padding ->
        Box(modifier = Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(
                modifier = Modifier.widthIn(max = 600.dp).fillMaxWidth().verticalScroll(rememberScrollState())
                    .padding(horizontal = 24.dp).padding(bottom = 24.dp),
                verticalArrangement = Arrangement.spacedBy(20.dp),
            ) {
                when (flow) {
                    is PairFlow.Choosing -> if (flow.typing) TypeAddress(flow, model) else ChooseMac(state.macs, model)
                    is PairFlow.Code -> EnterCode(flow, model)
                    is PairFlow.Done -> Connected(flow.macName, onDone = model::closeConnect)
                    PairFlow.Hidden -> {}
                }
            }
        }
    }
}

/** What to do on the Mac first, then the Macs in sight. */
@Composable
private fun ChooseMac(macs: List<FoundMac>, model: ScanViewModel) {
    Surface(shape = MaterialTheme.shapes.large, color = MaterialTheme.colorScheme.surfaceContainerLow) {
        Column(modifier = Modifier.fillMaxWidth().padding(20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Step(1, stringResource(R.string.connect_step_1))
            Step(2, stringResource(R.string.connect_step_2))
            Step(3, stringResource(R.string.connect_step_3))
        }
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(stringResource(R.string.macs_nearby), style = MaterialTheme.typography.titleMedium)
        CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp)
    }
    if (macs.isEmpty()) {
        Text(
            stringResource(R.string.looking_for_macs), style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
    macs.forEach { mac ->
        Surface(
            onClick = { model.chooseMac(mac) }, shape = MaterialTheme.shapes.large,
            color = MaterialTheme.colorScheme.secondaryContainer, contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Row(
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 18.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                Icon(AppIcons.Mac, contentDescription = null, modifier = Modifier.size(32.dp))
                Text(mac.name, style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
                Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null)
            }
        }
    }
    TextButton(onClick = model::typeAddressInstead) { Text(stringResource(R.string.enter_address_instead)) }
}

@Composable
private fun Step(number: Int, text: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        Surface(shape = CircleShape, color = MaterialTheme.colorScheme.primary, contentColor = MaterialTheme.colorScheme.onPrimary) {
            Box(modifier = Modifier.size(28.dp), contentAlignment = Alignment.Center) {
                Text("$number", style = MaterialTheme.typography.labelLarge)
            }
        }
        Text(text, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.padding(top = 2.dp))
    }
}

/** For when the Mac cannot be found by itself: type where it is. */
@Composable
private fun TypeAddress(flow: PairFlow.Choosing, model: ScanViewModel) {
    val focus = remember { FocusRequester() }
    Text(stringResource(R.string.address_title), style = MaterialTheme.typography.headlineSmall)
    OutlinedTextField(
        value = flow.address, onValueChange = model::setTypedAddress, singleLine = true, enabled = !flow.checking,
        label = { Text(stringResource(R.string.address_label)) },
        supportingText = {
            Text(
                when (flow.problem) {
                    PairProblem.BadAddress -> stringResource(R.string.address_bad)
                    PairProblem.Unreachable -> stringResource(R.string.address_unreachable)
                    else -> stringResource(R.string.address_hint)
                }
            )
        },
        isError = flow.problem != null,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go, autoCorrectEnabled = false),
        keyboardActions = KeyboardActions(onGo = { model.submitTypedAddress() }),
        modifier = Modifier.fillMaxWidth().focusRequester(focus),
    )
    Button(onClick = model::submitTypedAddress, enabled = !flow.checking && flow.address.isNotBlank(), modifier = Modifier.heightIn(min = 52.dp)) {
        if (flow.checking) {
            CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp, color = MaterialTheme.colorScheme.onPrimary)
            Spacer(Modifier.width(12.dp))
        }
        Text(stringResource(R.string.continue_action), style = MaterialTheme.typography.titleMedium)
    }
    LaunchedEffect(Unit) { focus.requestFocus() }
}

/** Six boxes for the six digits. The code is tried as soon as the sixth is in. */
@Composable
private fun EnterCode(flow: PairFlow.Code, model: ScanViewModel) {
    Column(modifier = Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(20.dp)) {
        Icon(AppIcons.Mac, contentDescription = null, modifier = Modifier.size(56.dp), tint = MaterialTheme.colorScheme.primary)
        Text(stringResource(R.string.code_title, flow.macName), style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
        CodeField(digits = flow.digits, working = flow.working, isError = flow.problem != null, onChange = model::setCode, onDone = model::submitCode)
        when {
            flow.working ->
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    CircularProgressIndicator(modifier = Modifier.size(22.dp), strokeWidth = 3.dp)
                    Text(stringResource(R.string.connecting), style = MaterialTheme.typography.bodyLarge)
                }
            flow.problem != null ->
                Text(
                    stringResource(
                        when (flow.problem) {
                            PairProblem.NotOffering -> R.string.code_not_offered
                            PairProblem.Unreachable, PairProblem.BadAddress -> R.string.code_unreachable
                            PairProblem.TooManyDevices -> R.string.code_too_many
                            PairProblem.WrongCode -> R.string.code_wrong
                        }
                    ),
                    style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center,
                )
            else ->
                Text(
                    stringResource(R.string.code_hint), style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center,
                )
        }
    }
}

/**
 * The field stays ready for typing the whole time, also while a code is being
 * tried: a field that is switched off loses the keyboard, and after a wrong
 * code the person would have to tap the boxes before typing again. What is
 * typed while a code is being tried is simply not taken.
 */
@Composable
private fun CodeField(digits: String, working: Boolean, isError: Boolean, onChange: (String) -> Unit, onDone: () -> Unit) {
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    val label = stringResource(R.string.code_field)
    val colours = MaterialTheme.colorScheme
    BasicTextField(
        // The cursor always stays at the end: the boxes fill from left to right.
        value = TextFieldValue(digits, selection = TextRange(digits.length)),
        onValueChange = { onChange(it.text) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onDone() }),
        modifier = Modifier.focusRequester(focus).semantics { contentDescription = label },
        decorationBox = {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                repeat(CODE_LENGTH) { index ->
                    // A wider gap in the middle, as the Mac shows the code: 123 456.
                    if (index == CODE_LENGTH / 2) Spacer(Modifier.width(10.dp))
                    val next = index == digits.length && !working
                    Surface(
                        shape = MaterialTheme.shapes.small,
                        color = colours.surfaceContainerHighest,
                        border = BorderStroke(
                            2.dp,
                            when {
                                isError -> colours.error
                                next -> colours.primary
                                else -> colours.surfaceContainerHighest
                            },
                        ),
                        modifier = Modifier.width(48.dp).height(62.dp),
                    ) {
                        Box(contentAlignment = Alignment.Center) {
                            Text(digits.getOrNull(index)?.toString().orEmpty(), style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.SemiBold)
                        }
                    }
                }
            }
        },
    )
    // At the start, and again after a code that did not work, the keyboard is there.
    LaunchedEffect(isError) {
        focus.requestFocus()
        keyboard?.show()
    }
}

@Composable
private fun Connected(macName: String, onDone: () -> Unit) {
    Column(
        modifier = Modifier.fillMaxWidth().padding(top = 32.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Icon(Icons.Default.CheckCircle, contentDescription = null, modifier = Modifier.size(88.dp), tint = MaterialTheme.colorScheme.primary)
        Text(stringResource(R.string.paired_title, macName), style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center)
        Text(
            stringResource(R.string.paired_body), style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Button(onClick = onDone, modifier = Modifier.widthIn(min = 200.dp).heightIn(min = 56.dp)) {
            Text(stringResource(R.string.done), style = MaterialTheme.typography.titleMedium)
        }
    }
}
