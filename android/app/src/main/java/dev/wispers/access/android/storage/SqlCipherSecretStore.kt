package dev.wispers.access.android.storage

import dev.wispers.access.sdk.SecretScope
import dev.wispers.access.sdk.SecretStore
import dev.wispers.access.sdk.SecretStoreException

/**
 * The SDK's secret store on the SQLCipher database: a share's key material
 * encrypted at rest under a passphrase the Android Keystore wraps. The SDK
 * calls these from its own threads, synchronously, which Room allows off the
 * main thread.
 */
internal class SqlCipherSecretStore(private val dao: SecretDao) : SecretStore {

    override fun load(scope: SecretScope, key: String): ByteArray? = guard {
        dao.get(scope.shareColumnValue(), key)
    }

    override fun save(scope: SecretScope, key: String, value: ByteArray) = guard {
        dao.put(SecretEntity(scope.shareColumnValue(), key, value))
    }

    override fun delete(scope: SecretScope, key: String) = guard {
        dao.delete(scope.shareColumnValue(), key)
    }

    /**
     * What the `share` column holds for a scope - the share's id, or a
     * reserved word for the client's own secrets, which no share id (a UUID)
     * can be.
     */
    private fun SecretScope.shareColumnValue(): String = when (this) {
        is SecretScope.Share -> id
        SecretScope.Client -> CLIENT_SHARE_COLUMN_VALUE
    }

    private companion object {
        const val CLIENT_SHARE_COLUMN_VALUE = "client"
    }

    private inline fun <T> guard(block: () -> T): T = try {
        block()
    } catch (e: Exception) {
        throw SecretStoreException.Failed(e.message ?: e.javaClass.simpleName)
    }
}
