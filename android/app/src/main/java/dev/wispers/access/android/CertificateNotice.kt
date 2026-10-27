package dev.wispers.access.android

import android.os.Build
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.LifecycleResumeEffect
import dev.wispers.access.sdk.CertificateKind
import dev.wispers.access.sdk.Share

/**
 * What a forward-proxy app shows instead of its pages once the WebView has
 * refused one of the share's origins for its certificate. If the share has a
 * CA certificate, it offers to install it, and opens the app again once it's
 * installed. Otherwise it says the share's host has to fix the certificate.
 */
@Composable
fun CertificateNotice(share: Share, onRetry: () -> Unit) {
    val context = LocalContext.current
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
    val install = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {}
    val shareName = if (share.name.isBlank()) "This share" else "“${share.name}”"

    Notice {
        Text("This app's certificate isn't trusted", style = MaterialTheme.typography.titleLarge)
        val next = missing.firstOrNull()
        when {
            next != null -> {
                Text(
                    "$shareName comes with a certificate of “${next.description.subject}”, " +
                        "which you'll have to install to make this device trust the app's servers.",
                    style = MaterialTheme.typography.bodyLarge,
                )
                Text(
                    "Only install it if you trust this share. Once it's installed, the " +
                        "certificate's owner can make any website show up as secure.",
                    style = MaterialTheme.typography.bodyMedium,
                )
                if (ShareCertificates.installsDirectly) {
                    Button(onClick = { install.launch(ShareCertificates.installIntent(next)) }) {
                        Text("Install certificate")
                    }
                } else {
                    Text(
                        "Wispers Access saves it to Downloads and opens Settings. Tap " +
                            "“CA certificate” (search for it if it's not shown), then " +
                            "“Install anyway”, and pick “${next.fileName}”. Then come back here.",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    Button(
                        onClick = {
                            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q &&
                                ShareCertificates.saveToDownloads(context, next)
                            ) {
                                ShareCertificates.openInstallSettings(context)
                            } else {
                                Toast.makeText(context, "Couldn't save the certificate", Toast.LENGTH_SHORT).show()
                            }
                        },
                    ) {
                        Text("Save and open Settings")
                    }
                }
            }
            certificates.any { it.installable } -> Text(
                "The certificate is installed, but the app's servers still aren't trusted. " +
                    "Whoever runs the share has to fix this.",
                style = MaterialTheme.typography.bodyLarge,
            )
            certificates.isNotEmpty() -> Text(
                notInstallable(certificates.first(), shareName),
                style = MaterialTheme.typography.bodyLarge,
            )
            else -> Text(
                "This device doesn't trust the certificate of the app's servers, and " +
                    "the share has none to install. Whoever runs the share has to fix this.",
                style = MaterialTheme.typography.bodyLarge,
            )
        }
        OutlinedButton(onClick = onRetry) { Text("Try again") }
    }
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
