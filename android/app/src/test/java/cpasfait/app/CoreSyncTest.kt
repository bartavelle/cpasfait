package cpasfait.app

import cpasfait.core.Cpasfait
import cpasfait.core.PlaceArg
import org.apache.sshd.common.file.virtualfs.VirtualFileSystemFactory
import org.apache.sshd.server.SshServer
import org.apache.sshd.server.keyprovider.SimpleGeneratorHostKeyProvider
import org.apache.sshd.sftp.server.SftpSubsystemFactory
import org.bouncycastle.jce.provider.BouncyCastleProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.security.Security

/** The Rust core (built for this machine) syncing through the Kotlin SFTP
 * remote: the calls from Rust back into Kotlin, as on the phone. */
class CoreSyncTest {
    @get:Rule
    val tmp = TemporaryFolder()

    @Test
    fun syncOverSftp() {
        Security.removeProvider("BC")
        Security.insertProviderAt(BouncyCastleProvider(), 1)
        val root = tmp.newFolder("home")
        val server = SshServer.setUpDefaultServer().apply {
            port = 0
            keyPairProvider = SimpleGeneratorHostKeyProvider(File(tmp.root, "hostkey.ser").toPath())
            publickeyAuthenticator = org.apache.sshd.server.auth.pubkey.PublickeyAuthenticator { u, _, _ -> u == "me" }
            subsystemFactories = listOf(SftpSubsystemFactory())
            fileSystemFactory = VirtualFileSystemFactory(root.toPath())
            start()
        }
        try {
            val dir = tmp.newFolder("phone")
            val core = Cpasfait.open(dir.path, "Europe/Paris")
            val tab = core.tabs().first().id
            core.addTask(tab, PlaceArg.LastChildOf(null), "From the phone")
            assertEquals("2026-12-01 09:30", core.setDue(core.rows(tab, false).first().id, "2026-12-01 9:30"))
            core.setRemote(tab, "rsync:me@127.0.0.1:${server.port}:perso")
            val keys = SshKeys(dir)
            val sum = SftpProvider(keys).use { core.sync(it) }
            assertEquals(emptyList<String>(), sum.errors)
            assertTrue(sum.pushed > 0u)
            val files = File(root, "perso/$tab").walkTopDown().filter { it.isFile }.map { it.relativeTo(root).path }.toList()
            assertTrue(files.toString(), files.any { it.startsWith("perso/$tab/keys/") })
            assertTrue(files.toString(), files.any { it.startsWith("perso/$tab/ops/") })
            // Nothing new: a second sync sends nothing.
            assertEquals(0uL, SftpProvider(keys).use { core.sync(it) }.pushed)
        } finally {
            server.stop(true)
        }
    }
}
