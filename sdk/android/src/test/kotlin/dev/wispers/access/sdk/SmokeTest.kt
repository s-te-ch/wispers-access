package dev.wispers.access.sdk

import java.io.File
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The bindings reach the Rust side: on the host JVM, through JNA, against the
 * macOS build of the library (see `jna.library.path` in build.gradle.kts). A
 * client in a scratch directory starts offline, answers, rejects a bad invite
 * with the SDK's own exception, and accepts foreign callbacks.
 */
class SmokeTest {
    @Test
    fun aClientStartsOfflineThroughTheBindings() {
        val dir = Files.createTempDirectory("wispers-access-sdk").toFile()
        try {
            val client = Client(ClientConfig(dataDir = dir.path, secrets = null, observer = null))
            assertTrue(client.shares().isEmpty())
            assertNull(client.share("nope"))
            assertThrows(SdkException.InvalidInvite::class.java) { validateInvite("nope") }
            client.close()
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun foreignCallbacksPlugIn() {
        class MemorySecrets : SecretStore {
            val items = HashMap<String, ByteArray>()
            override fun load(share: ShareId, key: String) = items["$share/$key"]
            override fun save(share: ShareId, key: String, value: ByteArray) { items["$share/$key"] = value }
            override fun delete(share: ShareId, key: String) { items.remove("$share/$key") }
        }
        class Changes : Observer {
            val seen = ArrayList<Share>()
            override fun onShareChanged(share: Share) { seen.add(share) }
        }
        val dir = Files.createTempDirectory("wispers-access-sdk").toFile()
        try {
            val client = Client(ClientConfig(dataDir = dir.path, secrets = MemorySecrets(), observer = Changes()))
            assertEquals(0, client.shares().size)
            client.close()
        } finally {
            dir.deleteRecursively()
        }
    }
}
