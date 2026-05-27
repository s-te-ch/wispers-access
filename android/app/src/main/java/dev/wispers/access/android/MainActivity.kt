package dev.wispers.access.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import dagger.hilt.android.AndroidEntryPoint
import dev.wispers.access.android.screens.ShareListScreen
import dev.wispers.access.android.ui.theme.WispersAccessTheme

@AndroidEntryPoint
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            WispersAccessTheme {
                ShareListScreen()
            }
        }
    }
}
