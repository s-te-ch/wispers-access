package dev.wispers.access.android

import androidx.room.Room
import androidx.test.platform.app.InstrumentationRegistry
import dev.wispers.access.android.storage.ShareDatabase
import dev.wispers.access.android.storage.SqlCipherSecretStore
import dev.wispers.access.sdk.Client
import dev.wispers.access.sdk.ClientConfig
import dev.wispers.access.sdk.Observer
import dev.wispers.access.sdk.RequiredCookie
import dev.wispers.access.sdk.Share
import dev.wispers.access.sdk.ShareState
import java.io.File
import java.net.Socket
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

/**
 * The SDK inside the app, on a device or emulator, against a real host node:
 * joins with the invite code passed as an instrumentation argument (skipped
 * without one), browses an app through the host-routed proxy with the auth
 * cookie, refreshes, checks and drops its connection, and leaves.
 *
 * Run with:
 *   ./gradlew :app:connectedDebugAndroidTest \
 *     -Pandroid.testInstrumentationRunnerArguments.inviteCode=wax1_…
 */
class SdkRoundTripTest {

    @Test
    fun joinsBrowsesRefreshesAndLeaves() = runBlocking {
        val code = InstrumentationRegistry.getArguments().getString("inviteCode")
        assumeTrue("no inviteCode argument; skipping the round trip", !code.isNullOrBlank())

        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val dir = File(context.cacheDir, "sdk-roundtrip-${System.nanoTime()}")
        // The app's secret store over an in-memory database: the DAO is what
        // the SDK talks to; SQLCipher only changes what is on disk.
        val db = Room.inMemoryDatabaseBuilder(context, ShareDatabase::class.java).build()
        val seen = ArrayList<Share>()
        val client = Client(
            ClientConfig(
                dataDir = dir.path,
                secrets = SqlCipherSecretStore(db.secretDao()),
                observer = object : Observer {
                    override fun onShareChanged(share: Share) { synchronized(seen) { seen.add(share) } }
                },
            )
        )
        try {
            val share = client.join(code!!)
            assertEquals(ShareState.LIVE, share.state)
            val app = share.apps.first()
            assertEquals(listOf(share.id), client.shares().map { it.id })
            assertTrue(synchronized(seen) { seen.any { it.id == share.id } })

            // The proxy: refused without the cookie, served with it.
            val cookie = RequiredCookie("__wispers_proxy_auth", "s3cret")
            val proxy = client.startHostRoutedProxy(0u, cookie)
            val host = "${app.id}.${share.label}.localhost"
            assertEquals(403, request(proxy.port().toInt(), host, null))
            assertEquals(200, request(proxy.port().toInt(), host, "${cookie.name}=${cookie.value}"))

            // Nothing changed on the host since the join.
            assertNull(client.refresh(share.id))

            // A live cached connection survives the resume check; a network
            // change drops it and the next request re-dials.
            client.checkConnections()
            assertEquals(200, request(proxy.port().toInt(), host, "${cookie.name}=${cookie.value}"))
            client.closeConnections()
            assertEquals(200, request(proxy.port().toInt(), host, "${cookie.name}=${cookie.value}"))

            client.leave(share.id)
            assertTrue(client.shares().isEmpty())
            assertNull(db.secretDao().get(share.id, "iroh_secret"))
        } finally {
            client.close()
            db.close()
            dir.deleteRecursively()
        }
    }

    /** One raw HTTP/1.1 request to the proxy, since only a browser resolves `*.localhost`. */
    private fun request(port: Int, host: String, cookie: String?): Int =
        Socket("127.0.0.1", port).use { socket ->
            val cookieLine = if (cookie != null) "Cookie: $cookie\r\n" else ""
            socket.getOutputStream().write(
                "GET /hello HTTP/1.1\r\nHost: $host\r\n${cookieLine}Connection: close\r\n\r\n".toByteArray()
            )
            val status = socket.getInputStream().bufferedReader().readLine()
            status.split(" ")[1].toInt()
        }
}
