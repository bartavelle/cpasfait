package cpasfait.app

import cpasfait.core.RemoteException
import org.apache.sshd.common.file.virtualfs.VirtualFileSystemFactory
import org.apache.sshd.server.SshServer
import org.apache.sshd.server.keyprovider.SimpleGeneratorHostKeyProvider
import org.apache.sshd.sftp.server.SftpSubsystemFactory
import org.bouncycastle.jce.provider.BouncyCastleProvider
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.security.PublicKey
import java.security.Security

/** SftpRemote against an SFTP server rooted in a temporary directory. */
class SftpTest {
    @get:Rule
    val tmp = TemporaryFolder()

    private lateinit var server: SshServer
    private lateinit var root: File
    private lateinit var keys: SshKeys
    private val accepted = mutableListOf<PublicKey>()

    @Before
    fun start() {
        Security.removeProvider("BC")
        Security.insertProviderAt(BouncyCastleProvider(), 1)
        root = tmp.newFolder("home")
        keys = SshKeys(tmp.newFolder("app"))
        server = serverWith(File(tmp.root, "hostkey.ser"))
    }

    private fun serverWith(hostKey: File): SshServer = SshServer.setUpDefaultServer().apply {
        port = 0
        keyPairProvider = SimpleGeneratorHostKeyProvider(hostKey.toPath())
        publickeyAuthenticator = org.apache.sshd.server.auth.pubkey.PublickeyAuthenticator { user, key, _ ->
            accepted.add(key)
            user == "me" && key.algorithm.startsWith("Ed")
        }
        subsystemFactories = listOf(SftpSubsystemFactory())
        fileSystemFactory = VirtualFileSystemFactory(root.toPath())
        start()
    }

    @After
    fun stop() {
        server.stop(true)
    }

    private fun remote(path: String = "shared/perso") =
        SftpRemote(SshTarget.parse("rsync:me@127.0.0.1:${server.port}:$path")!!, keys)

    @Test
    fun targets() {
        assertEquals(SshTarget("me", "host", 22, "perso"), SshTarget.parse("rsync:me@host:perso"))
        assertEquals(SshTarget("me", "host", 22, "perso"), SshTarget.parse("rsync:me@host:~/perso/"))
        assertEquals(SshTarget("me", "host", 2222, "/srv/x"), SshTarget.parse("rsync:me@host:2222:/srv/x"))
        assertEquals(SshTarget("me", "host", 22, "."), SshTarget.parse("rsync:me@host:"))
        assertEquals(null, SshTarget.parse("rsync:host:path")) // no user
        assertEquals(null, SshTarget.parse("gdrive:abc"))
    }

    @Test
    fun filesGoThereOnceAndComeBack() {
        assertTrue(keys.public().startsWith("ssh-ed25519 AAAA"))
        remote().use { r ->
            assertEquals(emptyList<String>(), r.list(""))
            r.putNew("tab/ops/dev/1-2.seg", byteArrayOf(1, 2, 3))
            r.putNew("tab/keys/e/k.age", "key".toByteArray())
            try {
                r.putNew("tab/ops/dev/1-2.seg", byteArrayOf(9))
                fail("overwrote")
            } catch (_: RemoteException.AlreadyExists) {
            }
            assertEquals(listOf("tab/keys/e/k.age", "tab/ops/dev/1-2.seg"), r.list(""))
            assertEquals(listOf("tab/ops/dev/1-2.seg"), r.list("tab/ops"))
            assertEquals(emptyList<String>(), r.list("other"))
            assertArrayEquals(byteArrayOf(1, 2, 3), r.get("tab/ops/dev/1-2.seg"))
            try {
                r.get("tab/nope")
                fail("found a missing file")
            } catch (_: RemoteException.NotFound) {
            }
        }
        // On the server: no temporary files left.
        val files = root.walkTopDown().filter { it.isFile }.map { it.relativeTo(root).path }.toList().sorted()
        assertEquals(listOf("shared/perso/tab/keys/e/k.age", "shared/perso/tab/ops/dev/1-2.seg"), files)
        // The host key was recorded, then is checked.
        assertTrue(keys.knownHosts.readText().contains("127.0.0.1"))
        remote().use { r -> assertEquals(2, r.list("").size) }
    }

    @Test
    fun aChangedHostKeyIsRefused() {
        remote().use { it.list("") }
        server.stop(true)
        server = serverWith(File(tmp.root, "other-hostkey.ser"))
        // Same port is not guaranteed: record the old key for the new port too.
        val line = keys.knownHosts.readLines().first { it.isNotBlank() }
        keys.knownHosts.appendText(line.replaceFirst(Regex("^\\S+"), "[127.0.0.1]:${server.port}") + "\n")
        try {
            remote().use { it.list("") }
            fail("connected to a host with another key")
        } catch (e: RemoteException.Failed) {
            assertTrue(e.reason, e.reason.contains("127.0.0.1"))
        }
    }
}
