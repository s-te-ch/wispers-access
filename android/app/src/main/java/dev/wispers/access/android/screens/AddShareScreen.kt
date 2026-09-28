package dev.wispers.access.android.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SecondaryTabRow
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.unit.dp
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.wispers.access.android.ShareManager
import dev.wispers.access.sdk.SdkException
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.Transport
import dev.wispers.access.sdk.validateInvite
import javax.inject.Inject
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

@HiltViewModel
class AddShareViewModel @Inject constructor(
    private val manager: ShareManager,
) : ViewModel() {

    enum class Tab { ENTER_CODE, SCAN_QR }

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
        val tab: Tab = Tab.ENTER_CODE,
        val code: String = "",
        val phase: Phase = Phase.Idle,
        val error: String? = null,
    )

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    fun onTabChange(tab: Tab) = _state.update { it.copy(tab = tab) }

    fun onCodeChange(code: String) = _state.update { it.copy(code = code, error = null) }

    fun onJoinClick() {
        if (_state.value.phase !is Phase.Idle) return
        val code = _state.value.code.trim()
        if (!validate(code)) return
        viewModelScope.launch { runJoin(code) }
    }

    /** Handles a scanned QR payload: joins on a valid invite code, errors otherwise. */
    fun onScanResult(contents: String) {
        if (_state.value.phase !is Phase.Idle) return
        val code = contents.trim()
        if (!validate(code)) return
        _state.update { it.copy(code = code, error = null) }
        viewModelScope.launch { runJoin(code) }
    }

    /** Which transport the code names, shown once it parses. */
    fun transportOf(code: String): Transport? =
        runCatching { validateInvite(code.trim()) }.getOrNull()

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
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            when (val phase = state.phase) {
                AddShareViewModel.Phase.Idle -> IdleContent(
                    tab = state.tab,
                    code = state.code,
                    transport = viewModel.transportOf(state.code),
                    error = state.error,
                    onTabChange = viewModel::onTabChange,
                    onCodeChange = viewModel::onCodeChange,
                    onJoin = viewModel::onJoinClick,
                    onScanResult = viewModel::onScanResult,
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

@Composable
private fun IdleContent(
    tab: AddShareViewModel.Tab,
    code: String,
    transport: Transport?,
    error: String?,
    onTabChange: (AddShareViewModel.Tab) -> Unit,
    onCodeChange: (String) -> Unit,
    onJoin: () -> Unit,
    onScanResult: (String) -> Unit,
) {
    SecondaryTabRow(selectedTabIndex = tab.ordinal) {
        Tab(
            selected = tab == AddShareViewModel.Tab.ENTER_CODE,
            onClick = { onTabChange(AddShareViewModel.Tab.ENTER_CODE) },
            text = { Text("Enter code") },
        )
        Tab(
            selected = tab == AddShareViewModel.Tab.SCAN_QR,
            onClick = { onTabChange(AddShareViewModel.Tab.SCAN_QR) },
            text = { Text("Scan QR") },
        )
    }
    when (tab) {
        AddShareViewModel.Tab.ENTER_CODE -> EnterCodeContent(
            code = code,
            transport = transport,
            error = error,
            onCodeChange = onCodeChange,
            onJoin = onJoin,
        )
        AddShareViewModel.Tab.SCAN_QR -> ScanQrContent(
            error = error,
            onScanResult = onScanResult,
        )
    }
}

@Composable
private fun EnterCodeContent(
    code: String,
    transport: Transport?,
    error: String?,
    onCodeChange: (String) -> Unit,
    onJoin: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Invitation code", style = MaterialTheme.typography.labelLarge)
        val clipboard = LocalClipboardManager.current
        OutlinedTextField(
            value = code,
            onValueChange = onCodeChange,
            placeholder = { Text("wax1_…") },
            singleLine = true,
            isError = error != null,
            trailingIcon = {
                TextButton(
                    onClick = { clipboard.getText()?.text?.let(onCodeChange) },
                ) {
                    Text("Paste")
                }
            },
            modifier = Modifier.fillMaxWidth(),
        )
        if (transport != null) {
            Text(
                when (transport) {
                    Transport.WISPERS_CONNECT -> "Via Wispers Connect"
                    Transport.IROH -> "Peer to peer (iroh)"
                    Transport.TAILSCALE -> "Via Tailscale"
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
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
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Join")
        }
        Text(
            "Codes are issued by the person sharing the app with you.",
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

@Composable
private fun ScanQrContent(
    error: String?,
    onScanResult: (String) -> Unit,
) {
    val scanLauncher = rememberLauncherForActivityResult(ScanContract()) { result ->
        // contents is null when the user backs out of the scanner.
        result.contents?.let(onScanResult)
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(
            onClick = {
                scanLauncher.launch(
                    ScanOptions().apply {
                        setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                        setPrompt("Scan the invite QR code")
                        setBeepEnabled(false)
                    },
                )
            },
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Open camera")
        }
        if (error != null) {
            Text(
                error,
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Text(
            "Point the camera at the QR code from the invite.",
            style = MaterialTheme.typography.bodySmall,
        )
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
