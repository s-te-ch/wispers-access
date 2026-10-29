package dev.wispers.access.android.screens

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.ImageAnalysis
import androidx.camera.mlkit.vision.MlKitAnalyzer
import androidx.camera.view.CameraController
import androidx.camera.view.LifecycleCameraController
import androidx.camera.view.PreviewView
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import dev.wispers.access.android.R
import dev.wispers.access.android.ui.theme.AccessOnViewfinder
import dev.wispers.access.android.ui.theme.AccessViewfinder

private enum class CameraAccess { ASKING, GRANTED, DENIED }

/**
 * The camera half of the add-a-share screen. A square live viewfinder that
 * hands every QR payload it decodes to [onScan], frame after frame. The caller
 * decides what counts as an invite and ignores repeats. Asks for camera
 * permission when first shown. Denied, it collapses to a one-line row pointing
 * at the app's settings, and picks up a grant made there on return.
 * [viewfinderSize] is the square's side. The denied row spans the full width.
 */
@Composable
fun QrScanPanel(onScan: (String) -> Unit, viewfinderSize: Dp) {
    val context = LocalContext.current
    var access by rememberSaveable {
        mutableStateOf(if (hasCameraPermission(context)) CameraAccess.GRANTED else CameraAccess.ASKING)
    }
    // Saved so a rotation while the dialog is up doesn't ask a second time.
    var asked by rememberSaveable { mutableStateOf(false) }
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        access = if (it) CameraAccess.GRANTED else CameraAccess.DENIED
    }
    LaunchedEffect(Unit) {
        if (access == CameraAccess.ASKING && !asked) {
            asked = true
            request.launch(Manifest.permission.CAMERA)
        }
    }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) {
        if (hasCameraPermission(context)) access = CameraAccess.GRANTED
    }

    when (access) {
        CameraAccess.GRANTED -> LiveViewfinder(onScan = onScan, modifier = Modifier.width(viewfinderSize))
        CameraAccess.ASKING -> ViewfinderFrame(modifier = Modifier.width(viewfinderSize))
        CameraAccess.DENIED -> CameraDeniedRow()
    }
}

@Composable
private fun LiveViewfinder(onScan: (String) -> Unit, modifier: Modifier) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val currentOnScan by rememberUpdatedState(onScan)
    val controller = remember { LifecycleCameraController(context) }
    val hasTorch = remember {
        context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_FLASH)
    }
    var torchOn by remember { mutableStateOf(false) }

    // Bound to the screen's lifecycle, so the camera stops when it is left or
    // backgrounded, and is released with the panel.
    DisposableEffect(controller, lifecycleOwner) {
        val executor = ContextCompat.getMainExecutor(context)
        val scanner = BarcodeScanning.getClient(
            BarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build(),
        )
        controller.setEnabledUseCases(CameraController.IMAGE_ANALYSIS)
        controller.setImageAnalysisAnalyzer(
            executor,
            MlKitAnalyzer(listOf(scanner), ImageAnalysis.COORDINATE_SYSTEM_ORIGINAL, executor) { result ->
                result.getValue(scanner)?.firstNotNullOfOrNull { it.rawValue }?.let(currentOnScan)
            },
        )
        controller.bindToLifecycle(lifecycleOwner)
        onDispose {
            controller.unbind()
            controller.clearImageAnalysisAnalyzer()
            scanner.close()
        }
    }
    // The torch goes out whenever the camera stops; keep the toggle in step.
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) { torchOn = false }

    ViewfinderFrame(modifier = modifier) {
        AndroidView(
            factory = {
                PreviewView(it).apply {
                    // A TextureView, so the preview clips to the rounded panel.
                    implementationMode = PreviewView.ImplementationMode.COMPATIBLE
                    scaleType = PreviewView.ScaleType.FILL_CENTER
                    this.controller = controller
                }
            },
            modifier = Modifier.fillMaxSize(),
        )
        if (hasTorch) {
            IconButton(
                onClick = {
                    torchOn = !torchOn
                    controller.enableTorch(torchOn)
                },
                modifier = Modifier
                    .align(Alignment.TopEnd)
                    .padding(14.dp)
                    .size(44.dp)
                    .background(
                        if (torchOn) MaterialTheme.colorScheme.primary else Color.White.copy(alpha = 0.14f),
                        CircleShape,
                    ),
            ) {
                Icon(
                    painter = painterResource(R.drawable.ic_flashlight),
                    contentDescription = if (torchOn) "Turn torch off" else "Turn torch on",
                    tint = if (torchOn) MaterialTheme.colorScheme.onPrimary else AccessOnViewfinder,
                )
            }
        }
    }
}

/** The dark square with the corner marks and the hint, around [content]. */
@Composable
private fun ViewfinderFrame(modifier: Modifier, content: @Composable BoxScope.() -> Unit = {}) {
    Box(
        modifier = modifier
            .aspectRatio(1f)
            .clip(RoundedCornerShape(24.dp))
            .background(AccessViewfinder),
    ) {
        content()
        ScanBrackets(
            modifier = Modifier
                .fillMaxSize(0.63f)
                .align(Alignment.Center),
        )
        Text(
            "Point at the QR code in the invite",
            color = AccessOnViewfinder,
            fontSize = 14.sp,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .padding(bottom = 18.dp),
        )
    }
}

/** The four corner marks, decoration only. Needs a square canvas. */
@Composable
private fun ScanBrackets(modifier: Modifier) {
    val color = MaterialTheme.colorScheme.primary
    Canvas(modifier) {
        val stroke = 4.dp.toPx()
        val inset = stroke / 2
        val arm = size.minDimension * 0.16f
        val radius = arm / 3
        val corner = Path().apply {
            moveTo(inset, inset + arm)
            lineTo(inset, inset + radius)
            quadraticTo(inset, inset, inset + radius, inset)
            lineTo(inset + arm, inset)
        }
        // The top-left mark, turned about the centre into the other corners.
        for (degrees in listOf(0f, 90f, 180f, 270f)) {
            rotate(degrees) {
                drawPath(corner, color, style = Stroke(width = stroke, cap = StrokeCap.Round))
            }
        }
    }
}

@Composable
private fun CameraDeniedRow() {
    val context = LocalContext.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(56.dp)
            .background(AccessViewfinder, RoundedCornerShape(24.dp))
            .padding(start = 20.dp, end = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            "Allow camera to scan",
            color = AccessOnViewfinder,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.weight(1f),
        )
        TextButton(
            onClick = {
                context.startActivity(
                    Intent(
                        Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                        Uri.fromParts("package", context.packageName, null),
                    ),
                )
            },
        ) {
            Text("Settings", color = MaterialTheme.colorScheme.primary)
        }
    }
}

private fun hasCameraPermission(context: Context): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
        PackageManager.PERMISSION_GRANTED
