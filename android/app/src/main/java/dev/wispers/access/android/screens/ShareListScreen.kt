package dev.wispers.access.android.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AssistChip
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
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
import dev.wispers.access.android.storage.Share
import dev.wispers.access.android.storage.ShareRepository
import javax.inject.Inject
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.stateIn

@HiltViewModel
class ShareListViewModel @Inject constructor(
    repo: ShareRepository,
) : ViewModel() {

    val shares: StateFlow<List<Share>> = repo.observeShares()
        .stateIn(
            scope = viewModelScope,
            started = SharingStarted.WhileSubscribed(5_000),
            initialValue = emptyList(),
        )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShareListScreen(
    onAddClick: () -> Unit,
    viewModel: ShareListViewModel = hiltViewModel(),
) {
    val shares by viewModel.shares.collectAsState()

    Scaffold(
        floatingActionButton = {
            ExtendedFloatingActionButton(
                onClick = onAddClick,
                text = { Text("Add a share") },
                icon = { Text("+") },
            )
        },
    ) { innerPadding ->
        if (shares.isEmpty()) {
            EmptyShareList(modifier = Modifier.fillMaxSize(), contentPadding = innerPadding)
        } else {
            ShareList(shares = shares, contentPadding = innerPadding)
        }
    }
}

@Composable
private fun EmptyShareList(modifier: Modifier, contentPadding: PaddingValues) {
    Box(modifier = modifier, contentAlignment = Alignment.Center) {
        Text("No shares yet. Tap “Add a share” to get started.")
    }
}

@Composable
private fun ShareList(shares: List<Share>, contentPadding: PaddingValues) {
    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = contentPadding,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        items(shares, key = { it.id.value }) { share ->
            AssistChip(
                onClick = { /* TODO: open share */ },
                label = { Text(share.nickname.ifBlank { "(unnamed share)" }) },
            )
        }
    }
}
