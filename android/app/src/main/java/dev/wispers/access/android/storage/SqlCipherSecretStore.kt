package dev.wispers.access.android.storage

import dev.wispers.access.sdk.SecretStore
import dev.wispers.access.sdk.SecretStoreException
import dev.wispers.access.sdk.ShareId

/**
 * The SDK's secret store on the SQLCipher database: a share's key material
 * encrypted at rest under a passphrase the Android Keystore wraps. The SDK
 * calls these from its own threads, synchronously, which Room allows off the
 * main thread.
 */
internal class SqlCipherSecretStore(private val dao: SecretDao) : SecretStore {

    override fun load(share: ShareId, key: String): ByteArray? = guard {
        dao.get(share, key)
    }

    override fun save(share: ShareId, key: String, value: ByteArray) = guard {
        dao.put(SecretEntity(share, key, value))
    }

    override fun delete(share: ShareId, key: String) = guard {
        dao.delete(share, key)
    }

    private inline fun <T> guard(block: () -> T): T = try {
        block()
    } catch (e: Exception) {
        throw SecretStoreException.Failed(e.message ?: e.javaClass.simpleName)
    }
}
