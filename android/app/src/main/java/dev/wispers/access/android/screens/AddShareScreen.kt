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
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import dev.wispers.access.android.storage.restoreOrInitNode
import javax.inject.Inject
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@HiltViewModel
class AddShareViewModel @Inject constructor(
    private val repo: ShareRepository,
) : ViewModel() {

    enum class Tab { ENTER_CODE, SCAN_QR }

    enum class JoinStep(val label: String) {
        VALIDATING("Validating invitation code…"),
        INITIALIZING("Generating node identity…"),
        REGISTERING("Registering…"),
        ACTIVATING("Activating…"),
    }

    sealed interface Phase {
        data object Idle : Phase
        data class Joining(val completed: Set<JoinStep>, val current: JoinStep) : Phase
        data class Joined(val shareId: ShareId, val nickname: String) : Phase
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
        val parts = _state.value.code.trim().split("/")
        if (parts.size != 2 || parts.any(String::isBlank)) {
            _state.update { it.copy(error = "Code must be in the form regtok/activation.") }
            return
        }
        val (token, activation) = parts
        viewModelScope.launch { runJoin(token, activation) }
    }

    private suspend fun runJoin(token: String, activation: String) {
        var createdId: ShareId? = null
        try {
            startStep(JoinStep.VALIDATING)
            completeStep(JoinStep.VALIDATING)

            startStep(JoinStep.INITIALIZING)
            val id = repo.createShare()
            createdId = id
            val storage = repo.storageFor(id)
            val (node, _) = storage.restoreOrInitNode()
            completeStep(JoinStep.INITIALIZING)

            startStep(JoinStep.REGISTERING)
            node.register(token)
            completeStep(JoinStep.REGISTERING)

            startStep(JoinStep.ACTIVATING)
            node.activate(activation)
            repo.markConnected(id)
            completeStep(JoinStep.ACTIVATING)

            // Adopt the connectivity group's display name as the share's label, but make it
            // best-effort. The share is already joined and usable at this point, so a failure
            // fetching group info must not fail the join.
            runCatching { node.groupInfo().name }
                .getOrNull()
                ?.takeIf { it.isNotBlank() }
                ?.let { repo.setNickname(id, it) }

            val nickname = repo.getShare(id)?.nickname.orEmpty()
            _state.update { it.copy(phase = Phase.Joined(shareId = id, nickname = nickname)) }
        } catch (e: CancellationException) {
            createdId?.let { withContext(NonCancellable) { repo.deleteShare(it) } }
            throw e
        } catch (e: Exception) {
            createdId?.let { repo.deleteShare(it) }
            _state.update {
                it.copy(phase = Phase.Idle, error = e.message ?: "Failed to join share.")
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
    onOpenShare: (ShareId) -> Unit,
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
                    error = state.error,
                    onTabChange = viewModel::onTabChange,
                    onCodeChange = viewModel::onCodeChange,
                    onJoin = viewModel::onJoinClick,
                )
                is AddShareViewModel.Phase.Joining -> JoinProgress(phase = phase)
                is AddShareViewModel.Phase.Joined -> JoinSuccess(
                    phase = phase,
                    onOpen = { onOpenShare(phase.shareId) },
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
    error: String?,
    onTabChange: (AddShareViewModel.Tab) -> Unit,
    onCodeChange: (String) -> Unit,
    onJoin: () -> Unit,
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
            error = error,
            onCodeChange = onCodeChange,
            onJoin = onJoin,
        )
        AddShareViewModel.Tab.SCAN_QR -> ScanQrPlaceholder()
    }
}

@Composable
private fun EnterCodeContent(
    code: String,
    error: String?,
    onCodeChange: (String) -> Unit,
    onJoin: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Invitation code", style = MaterialTheme.typography.labelLarge)
        OutlinedTextField(
            value = code,
            onValueChange = onCodeChange,
            placeholder = { Text("regtok/activation") },
            singleLine = true,
            isError = error != null,
            modifier = Modifier.fillMaxWidth(),
        )
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
            Text("Join share")
        }
        Text(
            "Codes are issued by the person who set up the share.",
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

@Composable
private fun ScanQrPlaceholder() {
    Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Text("QR scanning coming soon.")
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
            label = if (phase.nickname.isBlank()) "Joined" else "Joined \"${phase.nickname}\"",
            status = StepStatus.DONE,
        )
        Button(
            onClick = onOpen,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Open share")
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
