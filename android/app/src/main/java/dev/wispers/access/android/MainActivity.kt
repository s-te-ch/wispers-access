package dev.wispers.access.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import dagger.hilt.android.AndroidEntryPoint
import dev.wispers.access.android.screens.AddShareScreen
import dev.wispers.access.android.screens.ShareDetailScreen
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
    const val SHARE_DETAIL = "share-detail/{shareId}"

    fun shareDetail(shareId: String) = "share-detail/$shareId"
}

@Composable
private fun AppNavHost() {
    val navController = rememberNavController()
    NavHost(navController = navController, startDestination = Route.SHARE_LIST) {
        composable(Route.SHARE_LIST) {
            ShareListScreen(
                onAddClick = { navController.navigate(Route.ADD_SHARE) },
                onShareClick = { id -> navController.navigate(Route.shareDetail(id.value)) },
            )
        }
        composable(Route.ADD_SHARE) {
            AddShareScreen(
                onBack = { navController.popBackStack() },
                onOpenShare = { id ->
                    navController.navigate(Route.shareDetail(id.value)) {
                        popUpTo(Route.SHARE_LIST)
                    }
                },
            )
        }
        composable(
            route = Route.SHARE_DETAIL,
            arguments = listOf(navArgument("shareId") { type = NavType.StringType }),
        ) {
            val context = LocalContext.current
            ShareDetailScreen(
                onBack = { navController.popBackStack() },
                onOpenShare = { id -> ShareActivity.launch(context, id) },
            )
        }
    }
}
