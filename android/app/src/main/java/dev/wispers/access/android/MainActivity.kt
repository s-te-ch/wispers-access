package dev.wispers.access.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.Composable
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import dagger.hilt.android.AndroidEntryPoint
import dev.wispers.access.android.screens.AddShareScreen
import dev.wispers.access.android.screens.ShareListScreen
import dev.wispers.access.android.ui.theme.WispersAccessTheme

@AndroidEntryPoint
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            WispersAccessTheme {
                AppNavHost()
            }
        }
    }
}

private object Route {
    const val SHARE_LIST = "share-list"
    const val ADD_SHARE = "add-share"
}

@Composable
private fun AppNavHost() {
    val navController = rememberNavController()
    NavHost(navController = navController, startDestination = Route.SHARE_LIST) {
        composable(Route.SHARE_LIST) {
            ShareListScreen(onAddClick = { navController.navigate(Route.ADD_SHARE) })
        }
        composable(Route.ADD_SHARE) {
            AddShareScreen(
                onBack = { navController.popBackStack() },
                onOpenShare = { /* TODO: navigate to share-detail */ navController.popBackStack() },
            )
        }
    }
}
