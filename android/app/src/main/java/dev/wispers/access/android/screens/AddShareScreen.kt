package dev.wispers.access.android.screens

import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.min
import androidx.compose.ui.unit.sp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.wispers.access.android.ShareManager
import dev.wispers.access.sdk.SdkException
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.validateInvite
import javax.inject.Inject
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

@HiltViewModel
class AddShareViewModel @Inject constructor(
    private val manager: ShareManager,
) : ViewModel() {

    enum class JoinStep(val label: String) {
        VALIDATING("Validating invitation code…"),
        JOINING("Joining through the host…"),
    }

    sealed interface Phase {
        data object Idle : Phase
        data class Joining(val completed: Set<JoinStep>, val current: JoinStep) : Phase
        data class Joined(val share: Share) : Phase
    }

    data class State(
        val code: String = "",
        /** The code was found on the clipboard, not entered by the user. */
        val fromClipboard: Boolean = false,
        val phase: Phase = Phase.Idle,
        val error: String? = null,
    )

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    /**
     * The last QR payload seen. The camera keeps decoding a code for as long as
     * it is in frame, so a repeat is ignored, also after its join failed.
     * Clearing the field forgets it.
     */
    private var lastScan: String? = null

    fun onCodeChange(code: String) {
        if (code.isEmpty()) lastScan = null
        _state.update { it.copy(code = code, fromClipboard = false, error = null) }
    }

    /** Fills the empty field with the clipboard's text if that is an invite. */
    fun onClipboardText(text: String) {
        val code = text.trim()
        val s = _state.value
        if (s.phase !is Phase.Idle || s.code.isNotEmpty() || !isInvite(code)) return
        _state.update { it.copy(code = code, fromClipboard = true, error = null) }
    }

    fun onJoinClick() {
        if (_state.value.phase !is Phase.Idle) return
        val code = _state.value.code.trim()
        // The keyboard's Go key skips the disabled Join button.
        if (code.isEmpty() || !validate(code)) return
        viewModelScope.launch { runJoin(code) }
    }

    /** Handles a scanned QR payload. An invite joins at once, anything else is ignored. */
    fun onScan(payload: String) {
        if (_state.value.phase !is Phase.Idle || payload == lastScan) return
        lastScan = payload
        val code = payload.trim()
        if (!isInvite(code)) return
        _state.update { it.copy(code = code, fromClipboard = false, error = null) }
        viewModelScope.launch { runJoin(code) }
    }

    private fun isInvite(code: String): Boolean = runCatching { validateInvite(code) }.isSuccess

    /** Pre-validates so a bad code shows its real reason inline, without the progress steps. */
    private fun validate(code: String): Boolean {
        try {
            validateInvite(code)
            return true
        } catch (e: SdkException) {
            _state.update { it.copy(error = e.message ?: "Invite codes look like wax1_…") }
            return false
        }
    }

    private suspend fun runJoin(code: String) {
        startStep(JoinStep.VALIDATING)
        completeStep(JoinStep.VALIDATING)
        startStep(JoinStep.JOINING)
        try {
            // The SDK registers, activates and fetches the share, and rolls a
            // failed join back so nothing is left behind.
            val share = manager.join(code)
            completeStep(JoinStep.JOINING)
            _state.update { it.copy(phase = Phase.Joined(share)) }
        } catch (e: Exception) {
            _state.update {
                it.copy(phase = Phase.Idle, error = e.message ?: "Failed to join.")
            }
        }
    }

    private fun startStep(step: JoinStep) {
        _state.update { s ->
            val completed = (s.phase as? Phase.Joining)?.completed ?: emptySet()
            s.copy(phase = Phase.Joining(completed = completed, current = step))
        }
    }

    private fun completeStep(step: JoinStep) {
        _state.update { s ->
            val joining = s.phase as? Phase.Joining ?: return@update s
            s.copy(phase = joining.copy(completed = joining.completed + step))
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AddShareScreen(
    onBack: () -> Unit,
    onOpenShare: (Share) -> Unit,
    viewModel: AddShareViewModel = hiltViewModel(),
) {
    val state by viewModel.state.collectAsState()
    // Here rather than in the idle form, so a failed join's return to it
    // doesn't read the clipboard (and show the system's toast) again.
    ClipboardPrefill(onText = viewModel::onClipboardText)

    Scaffold(
        containerColor = MaterialTheme.colorScheme.background,
        topBar = {
            TopAppBar(
                title = { Text("Add a share") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                // The app bar defaults to surface (white) which clashes with the
                // background-colored page; blend it in like the list screen does.
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.background,
                ),
            )
        },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding)
                .consumeWindowInsets(innerPadding)
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            when (val phase = state.phase) {
                AddShareViewModel.Phase.Idle -> IdleContent(
                    code = state.code,
                    fromClipboard = state.fromClipboard,
                    error = state.error,
                    onCodeChange = viewModel::onCodeChange,
                    onJoin = viewModel::onJoinClick,
                    onScan = viewModel::onScan,
                )
                is AddShareViewModel.Phase.Joining -> JoinProgress(phase = phase)
                is AddShareViewModel.Phase.Joined -> JoinSuccess(
                    phase = phase,
                    onOpen = { onOpenShare(phase.share) },
                    onBackToList = onBack,
                )
            }
        }
    }
}

/**
 * Offers the clipboard's text to [onText] whenever the window gains focus, so
 * on open and on return from the app the invite was copied in. Focus rather than
 * resume, because Android only hands the clipboard to the focused app. Each
 * clip is read once, since every read shows the system's "pasted" toast.
 */
@Composable
private fun ClipboardPrefill(onText: (String) -> Unit) {
    val context = LocalContext.current
    val windowInfo = LocalWindowInfo.current
    val currentOnText by rememberUpdatedState(onText)
    var lastClip by remember { mutableLongStateOf(-1L) }
    LaunchedEffect(windowInfo) {
        snapshotFlow { windowInfo.isWindowFocused }.filter { it }.collect {
            val clipboard = context.getSystemService(ClipboardManager::class.java)
            val description = clipboard.primaryClipDescription ?: return@collect
            if (description.timestamp == lastClip || !description.hasMimeType("text/*")) return@collect
            lastClip = description.timestamp
            clipboardText(context)?.let(currentOnText)
        }
    }
}

/** The camera on top, the code field below. A scan and Join end in the same join. */
@Composable
private fun IdleContent(
    code: String,
    fromClipboard: Boolean,
    error: String?,
    onCodeChange: (String) -> Unit,
    onJoin: () -> Unit,
    onScan: (String) -> Unit,
) {
    BoxWithConstraints(modifier = Modifier.fillMaxSize().imePadding()) {
        // Square, but leaves room for the code field and Join on short
        // screens and above the keyboard.
        val panelSize = min(maxWidth, maxHeight * 0.55f)
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState()),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            LabelledDivider("SCAN THE QR CODE", modifier = Modifier.padding(bottom = 18.dp))
            QrScanPanel(onScan = onScan, viewfinderSize = panelSize)
            LabelledDivider(
                "OR PASTE THE INVITE CODE",
                modifier = Modifier.padding(top = 26.dp, bottom = 18.dp),
            )
            CodeEntry(
                code = code,
                fromClipboard = fromClipboard,
                error = error,
                onCodeChange = onCodeChange,
                onJoin = onJoin,
            )
        }
    }
}

@Composable
private fun LabelledDivider(label: String, modifier: Modifier) {
    Row(
        modifier = modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        HorizontalDivider(modifier = Modifier.weight(1f), color = MaterialTheme.colorScheme.outline)
        Text(
            label,
            style = MaterialTheme.typography.labelMedium,
            fontWeight = FontWeight.Bold,
            letterSpacing = 1.6.sp,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        HorizontalDivider(modifier = Modifier.weight(1f), color = MaterialTheme.colorScheme.outline)
    }
}

@Composable
private fun CodeEntry(
    code: String,
    fromClipboard: Boolean,
    error: String?,
    onCodeChange: (String) -> Unit,
    onJoin: () -> Unit,
) {
    val context = LocalContext.current
    Column(
        modifier = Modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        OutlinedTextField(
            value = code,
            onValueChange = onCodeChange,
            placeholder = { Text("wax1_…") },
            singleLine = true,
            isError = error != null,
            shape = RoundedCornerShape(16.dp),
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Ascii,
                imeAction = ImeAction.Go,
            ),
            keyboardActions = KeyboardActions(onGo = { onJoin() }),
            trailingIcon = {
                if (code.isEmpty()) {
                    TextButton(onClick = { clipboardText(context)?.let { onCodeChange(it.trim()) } }) {
                        Text("Paste", color = MaterialTheme.colorScheme.secondary)
                    }
                } else {
                    IconButton(onClick = { onCodeChange("") }) {
                        Icon(Icons.Filled.Close, contentDescription = "Clear")
                    }
                }
            },
            modifier = Modifier.fillMaxWidth(),
        )
        if (fromClipboard) {
            Row(
                modifier = Modifier.padding(horizontal = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(
                    Icons.Filled.Check,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.secondary,
                    modifier = Modifier.size(18.dp),
                )
                Text("Pasted from your clipboard", style = MaterialTheme.typography.bodyMedium)
            }
        }
        if (error != null) {
            Text(
                error,
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Button(
            onClick = onJoin,
            enabled = code.isNotBlank(),
            modifier = Modifier
                .fillMaxWidth()
                .height(56.dp),
        ) {
            Text("Join")
        }
    }
}

@Composable
private fun JoinProgress(phase: AddShareViewModel.Phase.Joining) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        AddShareViewModel.JoinStep.entries.forEach { step ->
            val status = when {
                step in phase.completed -> StepStatus.DONE
                step == phase.current -> StepStatus.RUNNING
                else -> StepStatus.PENDING
            }
            StepRow(label = step.label, status = status)
        }
    }
}

@Composable
private fun JoinSuccess(
    phase: AddShareViewModel.Phase.Joined,
    onOpen: () -> Unit,
    onBackToList: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        AddShareViewModel.JoinStep.entries.forEach { step ->
            StepRow(label = step.label, status = StepStatus.DONE)
        }
        StepRow(
            label = if (phase.share.name.isBlank()) "Joined" else "Joined \"${phase.share.name}\"",
            status = StepStatus.DONE,
        )
        Button(
            onClick = onOpen,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text(if (phase.share.apps.size == 1) "Open app" else "Show apps")
        }
        OutlinedButton(
            onClick = onBackToList,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Back to list")
        }
    }
}

private enum class StepStatus { PENDING, RUNNING, DONE }

@Composable
private fun StepRow(label: String, status: StepStatus) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(modifier = Modifier.size(24.dp), contentAlignment = Alignment.Center) {
            when (status) {
                StepStatus.DONE -> Icon(
                    imageVector = Icons.Filled.CheckCircle,
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                )
                StepStatus.RUNNING -> CircularProgressIndicator(
                    modifier = Modifier.size(20.dp),
                    strokeWidth = 2.dp,
                )
                StepStatus.PENDING -> Box(
                    modifier = Modifier
                        .size(20.dp)
                        .border(
                            width = 2.dp,
                            color = MaterialTheme.colorScheme.outline,
                            shape = CircleShape,
                        ),
                )
            }
        }
        Text(label, style = MaterialTheme.typography.bodyLarge)
    }
}

/** The clipboard's text, if it holds any. */
private fun clipboardText(context: Context): String? =
    context.getSystemService(ClipboardManager::class.java)
        .primaryClip
        ?.takeIf { it.itemCount > 0 }
        ?.getItemAt(0)
        ?.coerceToText(context)
        ?.toString()
