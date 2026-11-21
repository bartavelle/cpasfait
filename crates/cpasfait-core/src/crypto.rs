//! End-to-end encryption of everything stored on a remote (see
//! `docs/format.md`, "Encryption").
//!
//! - Each shared tab has one or more *epochs*, each with a random 256-bit key.
//!   Segments are sealed with XChaCha20-Poly1305 under an epoch key, the
//!   remote path being authenticated data (a file cannot be moved or renamed).
//! - Epoch keys are distributed in age files encrypted to the members' age
//!   (X25519) or SSH public keys.
//! - A new epoch carries an HMAC of itself under its parent's key, so only
//!   someone who already has a key of the tab can introduce one: the storage
//!   provider cannot slip in a key of its own.

use crate::error::{Error, Result};
use crate::ids::TabId;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// A symmetric epoch key.
pub type Key = [u8; 32];

pub fn random_key() -> Key {
    let mut k = [0; 32];
    getrandom::fill(&mut k).expect("OS random number generator");
    k
}

fn crypto_err(e: impl std::fmt::Display) -> Error {
    Error::Crypto(e.to_string())
}

// ------------------------------------------------------------- segments

const SEG_MAGIC: &[u8; 4] = b"CPF1";
const SEG_HEADER: usize = 4 + 16 + 24;

/// Encrypt a remote file: `"CPF1" | epoch id | nonce | ciphertext+tag`,
/// authenticating the header and `path`.
pub fn seal(key: &Key, epoch: Uuid, path: &str, plaintext: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).expect("OS random number generator");
    let mut out = Vec::with_capacity(SEG_HEADER + plaintext.len() + 16);
    out.extend_from_slice(SEG_MAGIC);
    out.extend_from_slice(epoch.as_bytes());
    out.extend_from_slice(&nonce);
    let aad = [&out[..], path.as_bytes()].concat();
    let ct = XChaCha20Poly1305::new(key.into())
        .encrypt(
            &nonce.into(),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .expect("encryption cannot fail");
    out.extend_from_slice(&ct);
    out
}

/// The epoch whose key decrypts a file made by [`seal`].
pub fn sealed_epoch(data: &[u8]) -> Result<Uuid> {
    if data.len() < SEG_HEADER + 16 || &data[..4] != SEG_MAGIC {
        return Err(Error::Crypto("not an encrypted cpasfait file".into()));
    }
    Ok(Uuid::from_slice(&data[4..20]).expect("16 bytes"))
}

/// Decrypt a file made by [`seal`], checking that it was made for `path`.
pub fn open(key: &Key, path: &str, data: &[u8]) -> Result<Vec<u8>> {
    sealed_epoch(data)?;
    let (header, ct) = data.split_at(SEG_HEADER);
    let nonce: [u8; 24] = header[20..].try_into().expect("24 bytes");
    let aad = [header, path.as_bytes()].concat();
    XChaCha20Poly1305::new(key.into())
        .decrypt(&nonce.into(), Payload { msg: ct, aad: &aad })
        .map_err(|_| Error::Crypto(format!("{path}: decryption failed (corrupted or tampered with)")))
}

// --------------------------------------------------------- epoch chain

fn epoch_hmac(prev_key: &Key, tab: TabId, epoch: Uuid, key: &Key) -> Hmac<Sha256> {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(prev_key).expect("any key length");
    m.update(b"cpasfait epoch v1\0");
    m.update(tab.0.as_bytes());
    m.update(epoch.as_bytes());
    m.update(key);
    m
}

/// Proof, by someone holding the parent epoch's key, that `epoch` (with `key`)
/// is a legitimate next epoch of `tab`.
pub fn epoch_mac(prev_key: &Key, tab: TabId, epoch: Uuid, key: &Key) -> Vec<u8> {
    epoch_hmac(prev_key, tab, epoch, key).finalize().into_bytes().to_vec()
}

pub fn verify_epoch_mac(prev_key: &Key, tab: TabId, epoch: Uuid, key: &Key, mac: &[u8]) -> bool {
    epoch_hmac(prev_key, tab, epoch, key).verify_slice(mac).is_ok()
}

// ----------------------------------------------------------- recipients

/// Canonical form of a member's public key: an age X25519 recipient
/// (`age1…`) or an SSH public key (`ssh-ed25519 AAAA…`, `ssh-rsa AAAA…`,
/// a trailing comment is dropped).
pub fn parse_recipient(s: &str) -> Result<String> {
    let s = s.trim();
    if let Ok(r) = s.parse::<age::x25519::Recipient>() {
        return Ok(r.to_string());
    }
    if s.starts_with("ssh-") {
        return s
            .parse::<age::ssh::Recipient>()
            .map(|r| r.to_string())
            .map_err(|e| Error::Invalid(format!("unsupported SSH public key: {e:?}")));
    }
    Err(Error::Invalid(format!(
        "{s:?} is not an age public key (age1…) or an SSH public key (ssh-ed25519 …)"
    )))
}

fn recipient(s: &str) -> Result<Box<dyn age::Recipient + Send>> {
    if let Ok(r) = s.parse::<age::x25519::Recipient>() {
        return Ok(Box::new(r));
    }
    s.parse::<age::ssh::Recipient>()
        .map(|r| Box::new(r) as _)
        .map_err(|_| Error::Invalid(format!("bad recipient {s:?}")))
}

/// age-encrypt `plaintext` to all `recipients`.
pub fn wrap(recipients: &[String], plaintext: &[u8]) -> Result<Vec<u8>> {
    let rs = recipients.iter().map(|r| recipient(r)).collect::<Result<Vec<_>>>()?;
    let enc =
        age::Encryptor::with_recipients(rs.iter().map(|r| r.as_ref() as &dyn age::Recipient)).map_err(crypto_err)?;
    let mut out = vec![];
    let mut w = enc.wrap_output(&mut out)?;
    w.write_all(plaintext)?;
    w.finish()?;
    Ok(out)
}

// ------------------------------------------------------------- identity

/// This user's secret key(s): an age identity file (X25519 keys, as made by
/// `age-keygen`) or an unencrypted SSH private key.
pub struct Identity {
    ids: Vec<Box<dyn age::Identity + Send + Sync>>,
    recipient: String,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("recipient", &self.recipient)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// A new random X25519 identity, in memory only.
    pub fn generate() -> Identity {
        Self::from_x25519(vec![age::x25519::Identity::generate()], None)
    }

    fn from_x25519(keys: Vec<age::x25519::Identity>, path: Option<PathBuf>) -> Identity {
        let recipient = keys[0].to_public().to_string();
        Identity {
            ids: keys.into_iter().map(|k| Box::new(k) as _).collect(),
            recipient,
            path,
        }
    }

    /// The public key other members invite (the first one of the file).
    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn load(path: &Path) -> Result<Identity> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Crypto(format!("cannot read identity {}: {e}", path.display())))?;
        let shown = path.display();
        if text.contains("-----BEGIN") {
            let id = age::ssh::Identity::from_buffer(text.as_bytes(), Some(shown.to_string()))
                .map_err(|e| Error::Crypto(format!("{shown}: {e}")))?;
            match &id {
                age::ssh::Identity::Unencrypted(_) => {}
                age::ssh::Identity::Encrypted(_) => {
                    return Err(Error::Crypto(format!(
                        "{shown}: passphrase-protected SSH keys are not supported yet"
                    )));
                }
                age::ssh::Identity::Unsupported(_) => {
                    return Err(Error::Crypto(format!("{shown}: unsupported SSH key type")));
                }
            }
            let r = age::ssh::Recipient::try_from(id.clone()).map_err(|e| Error::Crypto(format!("{shown}: {e:?}")))?;
            return Ok(Identity {
                ids: vec![Box::new(id)],
                recipient: r.to_string(),
                path: Some(path.to_owned()),
            });
        }
        let mut keys = vec![];
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            keys.push(line.parse::<age::x25519::Identity>().map_err(|_| {
                Error::Crypto(format!(
                    "{shown}: line {} is not an age X25519 secret key (age plugins and \
                     passphrase-protected identity files are not supported yet)",
                    n + 1
                ))
            })?);
        }
        if keys.is_empty() {
            return Err(Error::Crypto(format!("{shown}: no key in identity file")));
        }
        Ok(Self::from_x25519(keys, Some(path.to_owned())))
    }

    /// Generate a new identity and save it to `path` (mode 0600; fails if it exists).
    pub fn create(path: &Path) -> Result<Identity> {
        use age::secrecy::ExposeSecret;
        let key = age::x25519::Identity::generate();
        let text = format!(
            "# created by cpasfait: {}\n# public key: {}\n{}\n",
            jiff::Timestamp::now(),
            key.to_public(),
            key.to_string().expose_secret()
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(path)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        Ok(Self::from_x25519(vec![key], Some(path.to_owned())))
    }

    /// The identity at [`crate::paths::identity_path`], if there is one.
    pub fn load_default() -> Result<Option<Identity>> {
        crate::paths::identity_path()?.map(|p| Self::load(&p)).transpose()
    }

    /// The default identity, created in the data directory if there is none.
    /// The flag tells whether it was just created.
    pub fn load_or_create_default() -> Result<(Identity, bool)> {
        match Self::load_default()? {
            Some(id) => Ok((id, false)),
            None => Ok((Self::create(&crate::paths::own_identity_path())?, true)),
        }
    }

    /// Decrypt an age file. `None` if it is not encrypted to this identity.
    pub fn unwrap(&self, data: &[u8]) -> Result<Option<Vec<u8>>> {
        let d = age::Decryptor::new_buffered(data).map_err(crypto_err)?;
        match d.decrypt(self.ids.iter().map(|i| i.as_ref() as &dyn age::Identity)) {
            Ok(mut r) => {
                let mut out = vec![];
                r.read_to_end(&mut out).map_err(crypto_err)?;
                Ok(Some(out))
            }
            Err(age::DecryptError::NoMatchingKeys) => Ok(None),
            Err(e) => Err(crypto_err(e)),
        }
    }
}

// ---------------------------------------------------------------- utils

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open() {
        let k = random_key();
        let e = Uuid::now_v7();
        let data = seal(&k, e, "t/ops/d/1-2.seg", b"hello");
        assert_eq!(sealed_epoch(&data).unwrap(), e);
        assert_eq!(open(&k, "t/ops/d/1-2.seg", &data).unwrap(), b"hello");
        // moved, wrong key, flipped bit
        assert!(open(&k, "t/ops/d/3-4.seg", &data).is_err());
        assert!(open(&random_key(), "t/ops/d/1-2.seg", &data).is_err());
        let mut bad = data.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(&k, "t/ops/d/1-2.seg", &bad).is_err());
        assert!(sealed_epoch(b"{\"plain\": true}").is_err());
    }

    #[test]
    fn wrap_unwrap() {
        let (a, b, c) = (Identity::generate(), Identity::generate(), Identity::generate());
        let data = wrap(&[a.recipient().into(), b.recipient().into()], b"secret").unwrap();
        assert_eq!(a.unwrap(&data).unwrap().unwrap(), b"secret");
        assert_eq!(b.unwrap(&data).unwrap().unwrap(), b"secret");
        assert_eq!(c.unwrap(&data).unwrap(), None);
    }

    #[test]
    fn identity_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("id.txt");
        let id = Identity::create(&p).unwrap();
        assert!(Identity::create(&p).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let again = Identity::load(&p).unwrap();
        assert_eq!(again.recipient(), id.recipient());
        let data = wrap(&[id.recipient().into()], b"x").unwrap();
        assert_eq!(again.unwrap(&data).unwrap().unwrap(), b"x");
        std::fs::write(dir.path().join("bad"), "hello\n").unwrap();
        assert!(Identity::load(&dir.path().join("bad")).is_err());
    }

    #[test]
    fn recipients() {
        let id = Identity::generate();
        assert_eq!(
            parse_recipient(&format!("  {} ", id.recipient())).unwrap(),
            id.recipient()
        );
        let ssh = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHsKLqeplhpW+uObz5dvMgjz1OxfM/XXUB+VHtZ6isGN alice@host";
        let r = parse_recipient(ssh).unwrap();
        assert_eq!(r, ssh.rsplit_once(' ').unwrap().0);
        assert!(wrap(&[r, id.recipient().into()], b"x").is_ok());
        assert!(parse_recipient("age1nope").is_err());
        assert!(parse_recipient("bob").is_err());
    }

    #[test]
    fn epoch_macs() {
        let (p, k, tab, e) = (random_key(), random_key(), TabId::new(), Uuid::now_v7());
        let mac = epoch_mac(&p, tab, e, &k);
        assert!(verify_epoch_mac(&p, tab, e, &k, &mac));
        assert!(!verify_epoch_mac(&random_key(), tab, e, &k, &mac));
        assert!(!verify_epoch_mac(&p, tab, e, &random_key(), &mac));
        assert_eq!(unhex(&hex(&mac)).unwrap(), mac);
    }
}
