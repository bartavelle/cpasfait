package cpasfait.app

import cpasfait.core.Remote
import cpasfait.core.RemoteException
import cpasfait.core.RemoteProvider
import net.schmizz.sshj.DefaultConfig
import net.schmizz.sshj.SSHClient
import net.schmizz.sshj.common.KeyType
import net.schmizz.sshj.sftp.OpenMode
import net.schmizz.sshj.sftp.Response
import net.schmizz.sshj.sftp.SFTPClient
import net.schmizz.sshj.sftp.SFTPException
import net.schmizz.sshj.transport.verification.OpenSSHKnownHosts
import org.bouncycastle.crypto.generators.Ed25519KeyPairGenerator
import org.bouncycastle.crypto.params.Ed25519KeyGenerationParameters
import org.bouncycastle.crypto.params.Ed25519PrivateKeyParameters
import org.bouncycastle.crypto.util.OpenSSHPrivateKeyUtil
import org.bouncycastle.crypto.util.OpenSSHPublicKeyUtil
import java.io.Closeable
import java.io.File
import java.security.PublicKey
import java.security.SecureRandom
import java.util.Base64
import java.util.EnumSet
import java.util.UUID

/** This app's SSH key (ed25519), made on first use. Add its public key to
 * the server's `authorized_keys`. */
class SshKeys(dir: File) {
    private val dir = File(dir, "ssh")
    val privateKey = File(this.dir, "id_ed25519")
    private val publicKey = File(this.dir, "id_ed25519.pub")
    val knownHosts = File(this.dir, "known_hosts")

    /** `ssh-ed25519 AAAA… cpasfait-android`. */
    fun public(): String {
        if (!privateKey.exists()) generate()
        return publicKey.readText().trim()
    }

    private fun generate() {
        dir.mkdirs()
        val gen = Ed25519KeyPairGenerator()
        gen.init(Ed25519KeyGenerationParameters(SecureRandom()))
        val priv = gen.generateKeyPair().private as Ed25519PrivateKeyParameters
        val blob = Base64.getMimeEncoder(70, "\n".toByteArray())
            .encodeToString(OpenSSHPrivateKeyUtil.encodePrivateKey(priv))
        val tmp = File(dir, "id_ed25519.tmp")
        tmp.writeText("-----BEGIN OPENSSH PRIVATE KEY-----\n$blob\n-----END OPENSSH PRIVATE KEY-----\n")
        tmp.setReadable(false, false)
        tmp.setReadable(true, true)
        val pub = Base64.getEncoder().encodeToString(OpenSSHPublicKeyUtil.encodePublicKey(priv.generatePublicKey()))
        publicKey.writeText("ssh-ed25519 $pub cpasfait-android\n")
        tmp.renameTo(privateKey)
    }
}

/** Known hosts, trusting a host's key the first time (like ssh's
 * `StrictHostKeyChecking=accept-new`); a changed key is refused. */
private class TofuKnownHosts(file: File) : OpenSSHKnownHosts(file) {
    override fun hostKeyUnverifiableAction(hostname: String, key: PublicKey): Boolean {
        val entry = OpenSSHKnownHosts.HostEntry(null, hostname, KeyType.fromKey(key), key)
        entries().add(entry)
        file.parentFile?.mkdirs()
        write(entry)
        return true
    }
}

/** `rsync:[user@]host[:port]:path` → its parts (the path is relative to the
 * home directory unless absolute). */
data class SshTarget(val user: String, val host: String, val port: Int, val path: String) {
    companion object {
        fun parse(spec: String): SshTarget? {
            val target = spec.removePrefix("rsync:").takeIf { it != spec } ?: return null
            val (userHost, rest) = target.split(":", limit = 2).takeIf { it.size == 2 } ?: return null
            var path = rest
            var port = 22
            val portAndPath = rest.split(":", limit = 2)
            if (portAndPath.size == 2 && portAndPath[0].isNotEmpty() && portAndPath[0].all { it.isDigit() }) {
                port = portAndPath[0].toInt()
                path = portAndPath[1]
            }
            val at = userHost.lastIndexOf('@')
            if (at <= 0) return null
            path = path.removePrefix("~/").trimEnd('/').ifEmpty { "." }
            return SshTarget(userHost.substring(0, at), userHost.substring(at + 1), port, path)
        }
    }
}

/** A remote directory over SFTP: the same files as the desktops' rsync. */
class SftpRemote(private val target: SshTarget, private val keys: SshKeys) : Remote, Closeable {
    private var ssh: SSHClient? = null
    private var sftp: SFTPClient? = null

    @Synchronized
    private fun client(): SFTPClient {
        sftp?.let { return it }
        keys.public() // make sure there is a key
        // Not AndroidConfig: it lacks ECDSA host keys and rsa-sha2; the full Bouncy Castle is installed.
        val c = SSHClient(DefaultConfig())
        c.addHostKeyVerifier(TofuKnownHosts(keys.knownHosts))
        c.connectTimeout = 20_000
        c.timeout = 60_000
        c.connect(target.host, target.port)
        try {
            c.authPublickey(target.user, c.loadKeys(keys.privateKey.path))
            val s = c.newSFTPClient()
            ssh = c
            sftp = s
            return s
        } catch (e: Exception) {
            c.close()
            throw e
        }
    }

    private fun full(rel: String): String = if (target.path == ".") rel else "${target.path}/$rel"

    private fun <T> remote(what: String, f: () -> T): T =
        try {
            f()
        } catch (e: RemoteException) {
            throw e
        } catch (e: Exception) {
            CpasfaitApp.log("sftp $what", e)
            throw RemoteException.Failed("${target.host}: ${e.message ?: e.javaClass.simpleName}")
        }

    private fun missing(e: SFTPException) = e.statusCode == Response.StatusCode.NO_SUCH_FILE

    override fun list(prefix: String): List<String> = remote("list $prefix") {
        val sftp = client()
        val out = mutableListOf<String>()
        fun walk(rel: String) {
            val entries = try {
                sftp.ls(if (rel.isEmpty()) target.path else full(rel))
            } catch (e: SFTPException) {
                if (missing(e)) return else throw e
            }
            for (e in entries) {
                if (e.name == "." || e.name == "..") continue
                val child = if (rel.isEmpty()) e.name else "$rel/${e.name}"
                when {
                    e.isDirectory -> walk(child)
                    e.isRegularFile && !e.name.startsWith(".") -> out.add(child)
                }
            }
        }
        walk(prefix.trim('/'))
        out.sorted()
    }

    override fun get(path: String): ByteArray = remote("get $path") {
        try {
            client().open(full(path), EnumSet.of(OpenMode.READ)).use { f ->
                val len = f.length().toInt()
                val buf = ByteArray(len)
                var off = 0
                while (off < len) {
                    val n = f.read(off.toLong(), buf, off, len - off)
                    if (n < 0) break
                    off += n
                }
                if (off == len) buf else buf.copyOf(off)
            }
        } catch (e: SFTPException) {
            if (missing(e)) throw RemoteException.NotFound() else throw e
        }
    }

    override fun putNew(path: String, data: ByteArray) = remote("put $path") {
        val sftp = client()
        val dest = full(path)
        if (sftp.statExistence(dest) != null) throw RemoteException.AlreadyExists()
        val dir = dest.substringBeforeLast('/', ".")
        sftp.mkdirs(dir)
        val tmp = "$dir/.tmp-${UUID.randomUUID()}"
        sftp.open(tmp, EnumSet.of(OpenMode.WRITE, OpenMode.CREAT, OpenMode.EXCL)).use { f ->
            f.write(0, data, 0, data.size)
        }
        // A plain SFTP rename does not overwrite: create-only, atomically.
        try {
            sftp.rename(tmp, dest)
        } catch (e: SFTPException) {
            runCatching { sftp.rm(tmp) }
            if (sftp.statExistence(dest) != null) throw RemoteException.AlreadyExists()
            throw e
        }
    }

    @Synchronized
    override fun close() {
        runCatching { sftp?.close() }
        runCatching { ssh?.close() }
        sftp = null
        ssh = null
    }
}

/** Serves `rsync:` remotes over SFTP (the core handles the others). Close
 * it after use: it keeps one connection per remote. */
class SftpProvider(private val keys: SshKeys) : RemoteProvider, Closeable {
    private val open = mutableMapOf<String, SftpRemote>()

    @Synchronized
    override fun open(spec: String): Remote? {
        if (!spec.startsWith("rsync:")) return null
        val target = SshTarget.parse(spec)
            ?: throw RemoteException.Failed("expected rsync:user@host:path, got $spec")
        return open.getOrPut(spec) { SftpRemote(target, keys) }
    }

    @Synchronized
    override fun close() {
        open.values.forEach { it.close() }
        open.clear()
    }
}
