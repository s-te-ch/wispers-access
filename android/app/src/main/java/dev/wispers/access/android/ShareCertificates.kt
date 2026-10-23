package dev.wispers.access.android

import android.content.ActivityNotFoundException
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.os.Build
import android.provider.MediaStore
import android.provider.Settings
import android.security.KeyChain
import android.util.Base64
import android.util.Log
import androidx.annotation.RequiresApi
import dev.wispers.access.sdk.CertificateDescription
import dev.wispers.access.sdk.CertificateKind
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.describeCertificate
import java.security.KeyStore

/**
 * A share's certificates for its `https` origins, and what it takes to
 * install them on the device. Since Android 11, apps can't install a CA: the
 * user does, in Settings, from a file. Before that, [KeyChain] still could.
 */
object ShareCertificates {
    /** One of a share's certificates, as the SDK describes it. */
    data class Certificate(val der: ByteArray, val description: CertificateDescription) {
        /** Android installs only certificates marked CA. */
        val installable get() = description.kind == CertificateKind.CA

        /** Whether the user has installed it, from MDM or by hand. */
        fun isInstalled(): Boolean = runCatching {
            val store = KeyStore.getInstance("AndroidCAStore").apply { load(null) }
            store.aliases().asSequence()
                .filter { it.startsWith("user:") }
                .any { store.getCertificate(it)?.encoded?.contentEquals(der) == true }
        }.getOrElse {
            Log.w(TAG, "could not read the user CA store", it)
            false
        }

        /** The name it is saved under in Downloads. */
        val fileName get() = description.subject.replace(Regex("[^A-Za-z0-9 ._()-]"), "_").trim() + ".crt"
    }

    /** The share's certificates that parse. */
    fun of(share: Share): List<Certificate> = share.caCertificates.mapNotNull { pem ->
        val description = describeCertificate(pem) ?: return@mapNotNull null
        val base64 = pem.lines().filterNot { it.startsWith("-----") }.joinToString("")
        Certificate(Base64.decode(base64, Base64.DEFAULT), description)
    }

    /** Whether one of the share's certificates can be installed and isn't. */
    fun anyMissing(share: Share): Boolean = of(share).any { it.installable && !it.isInstalled() }

    /** Whether the app can hand the certificate to the system to install. */
    val installsDirectly get() = Build.VERSION.SDK_INT < Build.VERSION_CODES.R

    /** For [installsDirectly]: the system's own install dialog. */
    fun installIntent(certificate: Certificate): Intent = KeyChain.createInstallIntent().apply {
        putExtra(KeyChain.EXTRA_CERTIFICATE, certificate.der)
        putExtra(KeyChain.EXTRA_NAME, certificate.description.subject)
    }

    /**
     * Saves the certificate to Downloads, where Settings' installer finds it,
     * replacing a copy saved before. False if that failed.
     */
    @RequiresApi(Build.VERSION_CODES.Q)
    fun saveToDownloads(context: Context, certificate: Certificate): Boolean = runCatching {
        val resolver = context.contentResolver
        val collection = MediaStore.Downloads.EXTERNAL_CONTENT_URI
        // The app sees only the files it saved itself.
        val existing = resolver.query(
            collection,
            arrayOf(MediaStore.Downloads._ID),
            "${MediaStore.Downloads.DISPLAY_NAME} = ?",
            arrayOf(certificate.fileName),
            null,
        )?.use { if (it.moveToFirst()) it.getLong(0) else null }
        val uri = if (existing != null) {
            android.content.ContentUris.withAppendedId(collection, existing)
        } else {
            resolver.insert(
                collection,
                ContentValues().apply {
                    put(MediaStore.Downloads.DISPLAY_NAME, certificate.fileName)
                    put(MediaStore.Downloads.MIME_TYPE, "application/x-x509-ca-cert")
                },
            )
        } ?: return false
        resolver.openOutputStream(uri, "wt")?.use { it.write(certificate.der) } != null
    }.getOrElse {
        Log.w(TAG, "could not save ${certificate.fileName} to Downloads", it)
        false
    }

    /**
     * Opens Settings at its search for "CA certificate", whose first hit is
     * the installer (Pixel fills the search in; other makers may not, so the
     * notice says what to search for). Settings' security page if there is
     * no search.
     */
    fun openInstallSettings(context: Context) {
        val search = Intent(Settings.ACTION_APP_SEARCH_SETTINGS).putExtra("query", "CA certificate")
        try {
            context.startActivity(search)
        } catch (_: ActivityNotFoundException) {
            context.startActivity(Intent(Settings.ACTION_SECURITY_SETTINGS))
        }
    }

    private const val TAG = "ShareCertificates"
}
