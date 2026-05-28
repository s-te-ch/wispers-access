package dev.wispers.access.android.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SecondaryTabRow
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dagger.hilt.android.lifecycle.HiltViewModel
import dev.wispers.access.android.storage.ShareRepository
import dev.wispers.access.android.storage.restoreOrInitNode
import javax.inject.Inject
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

@HiltViewModel
class AddShareViewModel @Inject constructor(
    private val repo: ShareRepository,
) : ViewModel() {

    enum class Tab { ENTER_CODE, SCAN_QR }

    data class State(
        val tab: Tab = Tab.ENTER_CODE,
        val code: String = "",
        val busy: Boolean = false,
        val error: String? = null,
        val joined: Boolean = false,
    )

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    fun onTabChange(tab: Tab) = _state.update { it.copy(tab = tab) }

    fun onCodeChange(code: String) = _state.update { it.copy(code = code, error = null) }

    fun onJoinClick() {
        val parts = _state.value.code.trim().split("/")
        if (parts.size != 2 || parts.any(String::isBlank)) {
            _state.update { it.copy(error = "Code must be in the form regtok/activation.") }
            return
        }
        val (token, activation) = parts
        viewModelScope.launch {
            _state.update { it.copy(busy = true, error = null) }
            val id = repo.createShare()
            val result = runCatching {
                val storage = repo.storageFor(id)
                val (node, _) = storage.restoreOrInitNode()
                node.register(token)
                node.activate(activation)
                repo.markConnected(id)
            }
            if (result.isSuccess) {
                _state.update { it.copy(busy = false, joined = true) }
            } else {
                repo.deleteShare(id)
                _state.update {
                    it.copy(
                        busy = false,
                        error = result.exceptionOrNull()?.message ?: "Failed to join share.",
                    )
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AddShareScreen(
    onBack: () -> Unit,
    onJoined: () -> Unit,
    viewModel: AddShareViewModel = hiltViewModel(),
) {
    val state by viewModel.state.collectAsState()

    LaunchedEffect(state.joined) {
        if (state.joined) onJoined()
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Add a share") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
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
            CodeEntryTabs(selected = state.tab, onSelected = viewModel::onTabChange)
            when (state.tab) {
                AddShareViewModel.Tab.ENTER_CODE -> EnterCodeContent(
                    code = state.code,
                    error = state.error,
                    busy = state.busy,
                    onCodeChange = viewModel::onCodeChange,
                    onJoin = viewModel::onJoinClick,
                )
                AddShareViewModel.Tab.SCAN_QR -> ScanQrPlaceholder()
            }
        }
    }
}

@Composable
private fun CodeEntryTabs(
    selected: AddShareViewModel.Tab,
    onSelected: (AddShareViewModel.Tab) -> Unit,
) {
    SecondaryTabRow(selectedTabIndex = selected.ordinal) {
        Tab(
            selected = selected == AddShareViewModel.Tab.ENTER_CODE,
            onClick = { onSelected(AddShareViewModel.Tab.ENTER_CODE) },
            text = { Text("Enter code") },
        )
        Tab(
            selected = selected == AddShareViewModel.Tab.SCAN_QR,
            onClick = { onSelected(AddShareViewModel.Tab.SCAN_QR) },
            text = { Text("Scan QR") },
        )
    }
}

@Composable
private fun EnterCodeContent(
    code: String,
    error: String?,
    busy: Boolean,
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
            Text(error, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
        }
        Button(
            onClick = onJoin,
            enabled = !busy && code.isNotBlank(),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text(if (busy) "Joining…" else "Join share")
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
