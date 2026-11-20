package dev.wispers.access.android

import android.os.Build
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import dev.wispers.access.android.ui.theme.AccessOnline
import dev.wispers.access.android.ui.theme.AccessPrimaryDark
import dev.wispers.access.android.ui.theme.AccessPrimaryLight
import dev.wispers.access.sdk.CertificateKind
import dev.wispers.access.sdk.Share

/**
 * What a forward-proxy app shows instead of its pages once the WebView has
 * refused one of the share's origins for its certificate. If the share has a
 * CA certificate, it walks the user through installing it, and opens the app
 * again once it's installed. Otherwise it says the share's host has to fix the
 * certificate.
 */
@Composable
fun CertificateNotice(share: Share, onRetry: () -> Unit) {
    val certificates = remember(share) { ShareCertificates.of(share) }
    var missing by remember(share) {
        mutableStateOf(certificates.filter { it.installable && !it.isInstalled() })
    }
    // Back from Settings or the system's dialog: open the app again once
    // nothing is missing any more.
    LifecycleResumeEffect(share) {
        val stillMissing = missing.filterNot { it.isInstalled() }
        if (missing.isNotEmpty() && stillMissing.isEmpty()) onRetry()
        missing = stillMissing
        onPauseOrDispose {}
    }
    val shareName = if (share.name.isBlank()) "This share" else "“${share.name}”"

    Notice {
        val next = missing.firstOrNull()
        if (next != null) {
            InstallSteps(share, next, onRetry)
            return@Notice
        }
        Text("This app's certificate isn't trusted", style = MaterialTheme.typography.titleLarge)
        Text(
            when {
                certificates.any { it.installable } ->
                    "The certificate is installed, but the app's servers still aren't trusted. " +
                        "Whoever runs the share has to fix this."
                certificates.isNotEmpty() -> notInstallable(certificates.first(), shareName)
                else ->
                    "This device doesn't trust the certificate of the app's servers, and " +
                        "the share has none to install. Whoever runs the share has to fix this."
            },
            style = MaterialTheme.typography.bodyLarge,
        )
        OutlinedButton(onClick = onRetry) { Text("Try again") }
    }
}

/**
 * Installing [certificate] as numbered steps, since Android 11 it spans
 * the app, Downloads and Settings. Before that, the system's own dialog does
 * it all.
 */
@Composable
private fun InstallSteps(share: Share, certificate: ShareCertificates.Certificate, onRetry: () -> Unit) {
    val context = LocalContext.current
    val install = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {}
    var saved by rememberSaveable(share.id) { mutableStateOf(false) }
    val owner = share.name.ifBlank { null }

    Text(
        owner?.let { "Install $it's certificate" } ?: "Install the share's certificate",
        style = MaterialTheme.typography.titleLarge,
    )
    Text(
        "This app's servers use a certificate from “${certificate.description.subject}”. " +
            "This device has to trust it before the app can open.",
        style = MaterialTheme.typography.bodyLarge,
    )
    if (ShareCertificates.installsDirectly) {
        Text(
            "Android asks you to confirm and to name the certificate. Then the app opens by itself.",
            style = MaterialTheme.typography.bodyMedium,
        )
        Button(onClick = { install.launch(ShareCertificates.installIntent(certificate)) }) {
            Text("Install certificate")
        }
    } else {
        Column(verticalArrangement = Arrangement.spacedBy(18.dp)) {
            Step(1, "Save the certificate", done = saved) {
                StepText("Wispers Access saves “${certificate.fileName}” to Downloads and opens Settings.")
                val onSave = {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q &&
                        ShareCertificates.saveToDownloads(context, certificate)
                    ) {
                        saved = true
                        ShareCertificates.openInstallSettings(context)
                    } else {
                        Toast.makeText(context, "Couldn't save the certificate", Toast.LENGTH_SHORT).show()
                    }
                }
                if (saved) {
                    OutlinedButton(onClick = onSave) { Text("Save again") }
                } else {
                    Button(onClick = onSave) { Text("Save and open Settings") }
                }
            }
            Step(2, "Install it") {
                StepText("In Settings, tap “CA certificate”. If it isn't shown, search for it. Then tap “Install anyway”.")
            }
            Step(3, "Pick the file") {
                StepText("Choose “${certificate.fileName}” from Downloads.")
            }
            Step(4, "Come back here") {
                StepText("The app opens by itself once the certificate is installed.")
            }
        }
    }
    Text(
        "Only do this if you trust ${owner ?: "this share"}. Once installed, the certificate " +
            "lets its owner make any website look secure.",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    TextButton(onClick = onRetry) { Text("Check again") }
}

/** One numbered step of [InstallSteps], with a checkmark once done. */
@Composable
private fun Step(number: Int, title: String, done: Boolean = false, content: @Composable () -> Unit) {
    Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        Box(
            modifier = Modifier
                .size(28.dp)
                .background(if (done) AccessOnline else AccessPrimaryLight, CircleShape)
                .clearAndSetSemantics {},
            contentAlignment = Alignment.Center,
        ) {
            if (done) {
                Icon(Icons.Filled.Check, contentDescription = null, tint = Color.White, modifier = Modifier.size(16.dp))
            } else {
                Text("$number", style = MaterialTheme.typography.labelLarge, color = AccessPrimaryDark)
            }
        }
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                title,
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.semantics {
                    contentDescription = if (done) "Step $number, done. $title" else "Step $number. $title"
                },
            )
            content()
        }
    }
}

@Composable
private fun StepText(text: String) {
    Text(text, style = MaterialTheme.typography.bodyMedium)
}

/** Why a share's certificate that isn't a CA's can't help on Android. */
private fun notInstallable(certificate: ShareCertificates.Certificate, shareName: String): String {
    val names = certificate.description.names.joinToString(", ").ifEmpty { certificate.description.subject }
    return when (certificate.description.kind) {
        CertificateKind.SELF_SIGNED_SERVER ->
            "$shareName comes with a certificate for $names, which Android can't install, " +
                "as it installs only CA certificates. Whoever runs the share can make one."
        else ->
            "$shareName comes with a certificate for $names instead of its CA's. Whoever " +
                "runs the share has to add the CA's certificate."
    }
}
