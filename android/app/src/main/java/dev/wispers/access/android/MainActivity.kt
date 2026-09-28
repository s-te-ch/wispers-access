package dev.wispers.access.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
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
import dev.wispers.access.android.demo.DemoMode
import dev.wispers.access.android.screens.AddShareScreen
import dev.wispers.access.android.screens.ShareDetailScreen
import dev.wispers.access.android.screens.ShareListScreen
import dev.wispers.access.android.ui.theme.WispersAccessTheme
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.ShareState
import javax.inject.Inject

@AndroidEntryPoint
class MainActivity : ComponentActivity() {

    @Inject lateinit var manager: ShareManager

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        DemoMode.maybeActivate(this)
        if (DemoMode.active) manager.reload()
        // The app theme is always light, so pick dark bar icons explicitly instead
        // of letting edge-to-edge follow the system dark-mode setting.
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.light(
                android.graphics.Color.TRANSPARENT,
                android.graphics.Color.TRANSPARENT,
            ),
            navigationBarStyle = SystemBarStyle.light(
                android.graphics.Color.TRANSPARENT,
                android.graphics.Color.TRANSPARENT,
            ),
        )
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
            val context = LocalContext.current
            ShareListScreen(
                onAddClick = { navController.navigate(Route.ADD_SHARE) },
                onShareClick = { share -> navController.navigate(Route.shareDetail(share.id)) },
                onAppClick = { key -> BrowseActivity.launch(context, key) },
            )
        }
        composable(Route.ADD_SHARE) {
            val context = LocalContext.current
            AddShareScreen(
                onBack = { navController.popBackStack() },
                onOpenShare = { share ->
                    // Park the nav stack on the detail screen so leaving the
                    // WebView doesn't land back on the completed join flow.
                    navController.navigate(Route.shareDetail(share.id)) {
                        popUpTo(Route.SHARE_LIST)
                    }
                    // A share with one app opens it; several stay on detail.
                    share.onlyApp()?.let { BrowseActivity.launch(context, it) }
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
                onOpenApp = { key -> BrowseActivity.launch(context, key) },
            )
        }
    }
}

/** The one app to open straight away, if a live share has exactly one. */
fun Share.onlyApp(): BrowseKey? =
    if (state == ShareState.LIVE && apps.size == 1) BrowseKey(id, apps[0].id) else null
