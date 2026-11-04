// SPDX-License-Identifier: MIT OR Apache-2.0
//! Token storage: the OS keychain (Windows Credential Manager, macOS Keychain, Secret
//! Service on Linux) or, where there is none (containers), an encrypted file.
//!
//! Which store is used comes from `FAMILIAR_TOKEN_STORE`:
//!
//! | Value | Store |
//! | --- | --- |
//! | `auto` (default) | the keychain if one answers, else the encrypted file if a passphrase is set |
//! | `keychain` | the keychain only |
//! | `file` | the encrypted file only |
//!
//! The file is `tokens.enc` in the data directory (`FAMILIAR_DATA_DIR`, or the platform's
//! local data directory under `Familiar`). Its key is derived with Argon2id from the
//! passphrase in `FAMILIAR_TOKEN_PASSPHRASE`, or read from the file named by
//! `FAMILIAR_TOKEN_PASSPHRASE_FILE` (a container secret), and it is sealed with
//! XChaCha20-Poly1305.

use crate::error::{AuthError, Result};
use crate::record::{TokenRecord, validate_account};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where sign-ins are kept.
pub trait TokenStore: Send + Sync {
    /// `keychain`, `file` or `memory`, for `fam auth status`.
    fn name(&self) -> &'static str;
    /// The record for an account, if it is signed in.
    fn load(&self, account: &str) -> Result<Option<TokenRecord>>;
    /// Saves (replaces) an account's record.
    fn save(&self, record: &TokenRecord) -> Result<()>;
    /// Removes an account's record. False if there was none.
    fn delete(&self, account: &str) -> Result<bool>;
    /// The signed-in accounts, sorted.
    fn accounts(&self) -> Result<Vec<String>>;
}

// ---------------------------------------------------------------------------
// Keychain

/// The keychain service name entries are stored under.
pub const SERVICE: &str = "familiar";
/// Windows Credential Manager holds at most 2,560 bytes per credential, so records are
/// split into chunks of this size.
const CHUNK: usize = 2048;
const INDEX: &str = "#index";
const HEAD: &[u8] = b"FAM1 ";

/// A flat secret store: names to bytes. The keychain, or memory in tests.
pub trait Secrets: Send + Sync {
    /// Reads a secret.
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>>;
    /// Writes a secret.
    fn set(&self, name: &str, value: &[u8]) -> Result<()>;
    /// Deletes a secret; false if there was none.
    fn remove(&self, name: &str) -> Result<bool>;
}

/// The OS keychain through the `keyring` crate.
pub struct Keyring {
    service: String,
}

impl Keyring {
    /// Entries under `service`.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, name).map_err(keychain_error)
    }

    /// Whether a keychain answers on this machine.
    pub fn available(&self) -> std::result::Result<(), String> {
        let entry = keyring::Entry::new(&self.service, INDEX).map_err(|e| e.to_string())?;
        match entry.get_secret() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn keychain_error(e: keyring::Error) -> AuthError {
    AuthError::config(format!("the OS keychain failed: {e}"))
}

impl Secrets for Keyring {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match self.entry(name)?.get_secret() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_error(e)),
        }
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<()> {
        self.entry(name)?.set_secret(value).map_err(keychain_error)
    }

    fn remove(&self, name: &str) -> Result<bool> {
        match self.entry(name)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(keychain_error(e)),
        }
    }
}

/// Secrets in memory, for tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, Vec<u8>>>);

impl MemorySecrets {
    /// The number of entries, for tests.
    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The largest entry, for tests of the chunk limit.
    pub fn largest(&self) -> usize {
        let m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.values().map(Vec::len).max().unwrap_or(0)
    }
}

impl Secrets for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned())
    }
    fn set(&self, name: &str, value: &[u8]) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.into(), value.to_vec());
        Ok(())
    }
    fn remove(&self, name: &str) -> Result<bool> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name)
            .is_some())
    }
}

/// A [`TokenStore`] over a [`Secrets`] store. Each record is JSON split into chunks:
/// `<account>` holds `FAM1 <n>\n` and the first chunk, `<account>#1` .. `#<n-1>` the rest.
/// The head is written last, so a crash mid-save leaves the old record readable.
/// `#index` lists the accounts, since keychains can't be listed portably.
pub struct SecretStore<S: Secrets> {
    secrets: S,
    name: &'static str,
    lock: Mutex<()>,
}

/// The OS keychain store.
pub type KeychainStore = SecretStore<Keyring>;
/// An in-memory store with the keychain's chunking, for tests.
pub type MemoryStore = SecretStore<MemorySecrets>;

impl KeychainStore {
    /// The keychain under the `familiar` service.
    pub fn keychain() -> Self {
        SecretStore::new(Keyring::new(SERVICE), "keychain")
    }
}

impl MemoryStore {
    /// An empty memory store.
    pub fn memory() -> Self {
        SecretStore::new(MemorySecrets::default(), "memory")
    }
}

impl<S: Secrets> SecretStore<S> {
    /// Wraps a secret store.
    pub fn new(secrets: S, name: &'static str) -> Self {
        Self {
            secrets,
            name,
            lock: Mutex::new(()),
        }
    }

    /// The underlying secrets, for tests.
    pub fn secrets(&self) -> &S {
        &self.secrets
    }

    fn chunk_name(account: &str, i: usize) -> String {
        format!("{account}#{i}")
    }

    fn head_count(head: &[u8]) -> Option<(usize, &[u8])> {
        let rest = head.strip_prefix(HEAD)?;
        let nl = rest.iter().position(|&b| b == b'\n')?;
        let n = std::str::from_utf8(&rest[..nl]).ok()?.parse().ok()?;
        Some((n, &rest[nl + 1..]))
    }

    fn index(&self) -> Result<Vec<String>> {
        Ok(match self.secrets.get(INDEX)? {
            Some(b) => serde_json::from_slice(&b).unwrap_or_default(),
            None => Vec::new(),
        })
    }

    fn set_index(&self, mut names: Vec<String>) -> Result<()> {
        names.sort();
        names.dedup();
        if names.is_empty() {
            self.secrets.remove(INDEX)?;
            return Ok(());
        }
        let json = serde_json::to_vec(&names).map_err(|e| AuthError::internal(e.to_string()))?;
        self.secrets.set(INDEX, &json)
    }

    fn old_count(&self, account: &str) -> Result<usize> {
        Ok(self
            .secrets
            .get(account)?
            .and_then(|h| Self::head_count(&h).map(|(n, _)| n))
            .unwrap_or(0))
    }
}

impl<S: Secrets> TokenStore for SecretStore<S> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn load(&self, account: &str) -> Result<Option<TokenRecord>> {
        validate_account(account).map_err(AuthError::usage)?;
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(head) = self.secrets.get(account)? else {
            return Ok(None);
        };
        let damaged = || {
            AuthError::internal(format!(
                "the stored sign-in for `{account}` is damaged; run `fam auth logout {account}` and sign in again"
            ))
        };
        let (n, first) = Self::head_count(&head).ok_or_else(damaged)?;
        let mut json = first.to_vec();
        for i in 1..n {
            let part = self
                .secrets
                .get(&Self::chunk_name(account, i))?
                .ok_or_else(damaged)?;
            json.extend_from_slice(&part);
        }
        serde_json::from_slice(&json)
            .map(Some)
            .map_err(|_| damaged())
    }

    fn save(&self, record: &TokenRecord) -> Result<()> {
        let account = record.account.as_str();
        validate_account(account).map_err(AuthError::usage)?;
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let json = serde_json::to_vec(record).map_err(|e| AuthError::internal(e.to_string()))?;
        let chunks: Vec<&[u8]> = json.chunks(CHUNK).collect();
        let old = self.old_count(account)?;
        for (i, c) in chunks.iter().enumerate().skip(1) {
            self.secrets.set(&Self::chunk_name(account, i), c)?;
        }
        let mut head = HEAD.to_vec();
        head.extend_from_slice(format!("{}\n", chunks.len()).as_bytes());
        head.extend_from_slice(chunks[0]);
        self.secrets.set(account, &head)?;
        for i in chunks.len()..old {
            self.secrets.remove(&Self::chunk_name(account, i))?;
        }
        let mut index = self.index()?;
        if !index.iter().any(|a| a == account) {
            index.push(account.to_string());
            self.set_index(index)?;
        }
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<bool> {
        validate_account(account).map_err(AuthError::usage)?;
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let old = self.old_count(account)?;
        let existed = self.secrets.remove(account)?;
        for i in 1..old {
            self.secrets.remove(&Self::chunk_name(account, i))?;
        }
        let index = self.index()?;
        if index.iter().any(|a| a == account) {
            self.set_index(index.into_iter().filter(|a| a != account).collect())?;
        }
        Ok(existed)
    }

    fn accounts(&self) -> Result<Vec<String>> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.index()?;
        v.sort();
        Ok(v)
    }
}

// ---------------------------------------------------------------------------
// Encrypted file

/// Argon2id cost: 19 MiB, 2 passes, 1 lane (OWASP's minimum recommendation).
const KDF_M_KIB: u32 = 19 * 1024;
const KDF_T: u32 = 2;
const KDF_P: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Envelope {
    format: String,
    version: u32,
    kdf: Kdf,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Kdf {
    alg: String,
    m_kib: u32,
    t: u32,
    p: u32,
    salt: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Plain {
    accounts: BTreeMap<String, TokenRecord>,
}

/// Tokens in one encrypted file, for machines without a keychain (containers, CI).
pub struct FileStore {
    path: PathBuf,
    passphrase: String,
    // The derived key for the file's salt, so each save doesn't rerun Argon2.
    key: Mutex<Option<(Kdf, [u8; 32])>>,
}

impl FileStore {
    /// A store at `path`, sealed with a key derived from `passphrase`.
    pub fn new(path: impl Into<PathBuf>, passphrase: impl Into<String>) -> Result<Self> {
        let passphrase = passphrase.into();
        if passphrase.trim().len() < 8 {
            return Err(AuthError::config(
                "the token passphrase must be at least 8 characters",
            ));
        }
        Ok(Self {
            path: path.into(),
            passphrase,
            key: Mutex::new(None),
        })
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn derive(&self, kdf: &Kdf) -> Result<[u8; 32]> {
        let mut cache = self.key.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, key)) = cache.as_ref() {
            if k == kdf {
                return Ok(*key);
            }
        }
        let salt = B64
            .decode(&kdf.salt)
            .map_err(|_| self.damaged("bad salt"))?;
        let params = argon2::Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32))
            .map_err(|e| self.damaged(&e.to_string()))?;
        let argon =
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let mut key = [0u8; 32];
        argon
            .hash_password_into(self.passphrase.as_bytes(), &salt, &mut key)
            .map_err(|e| AuthError::internal(format!("key derivation failed: {e}")))?;
        *cache = Some((kdf.clone(), key));
        Ok(key)
    }

    fn damaged(&self, why: &str) -> AuthError {
        AuthError::config(format!(
            "the token file {} is damaged ({why}); delete it and sign in again",
            self.path.display()
        ))
    }

    fn read(&self) -> Result<(Plain, Option<Kdf>)> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Plain::default(), None));
            }
            Err(e) => {
                return Err(AuthError::config(format!(
                    "can't read the token file {}: {e}",
                    self.path.display()
                )));
            }
        };
        let env: Envelope = serde_json::from_slice(&bytes)
            .map_err(|_| self.damaged("not a Familiar token file"))?;
        if env.format != "familiar-tokens" || env.version != 1 || env.kdf.alg != "argon2id" {
            return Err(self.damaged("unknown format"));
        }
        let key = self.derive(&env.kdf)?;
        let nonce = B64
            .decode(&env.nonce)
            .map_err(|_| self.damaged("bad nonce"))?;
        let ct = B64
            .decode(&env.ciphertext)
            .map_err(|_| self.damaged("bad ciphertext"))?;
        if nonce.len() != 24 {
            return Err(self.damaged("bad nonce"));
        }
        let cipher = XChaCha20Poly1305::new(&key.into());
        let plain = cipher
            .decrypt(XNonce::from_slice(&nonce), ct.as_slice())
            .map_err(|_| {
                AuthError::config(format!(
                    "can't decrypt the token file {}: wrong passphrase (FAMILIAR_TOKEN_PASSPHRASE)?",
                    self.path.display()
                ))
            })?;
        let plain: Plain =
            serde_json::from_slice(&plain).map_err(|_| self.damaged("bad contents"))?;
        Ok((plain, Some(env.kdf)))
    }

    fn write(&self, plain: &Plain, kdf: Option<Kdf>) -> Result<()> {
        let kdf = match kdf {
            Some(k) => k,
            None => Kdf {
                alg: "argon2id".into(),
                m_kib: KDF_M_KIB,
                t: KDF_T,
                p: KDF_P,
                salt: B64.encode(random::<16>()?),
            },
        };
        let key = self.derive(&kdf)?;
        let nonce = random::<24>()?;
        let json = serde_json::to_vec(plain).map_err(|e| AuthError::internal(e.to_string()))?;
        let ct = XChaCha20Poly1305::new(&key.into())
            .encrypt(XNonce::from_slice(&nonce), json.as_slice())
            .map_err(|_| AuthError::internal("encryption failed"))?;
        let env = Envelope {
            format: "familiar-tokens".into(),
            version: 1,
            kdf,
            nonce: B64.encode(nonce),
            ciphertext: B64.encode(ct),
        };
        let out =
            serde_json::to_vec_pretty(&env).map_err(|e| AuthError::internal(e.to_string()))?;
        write_private(&self.path, &out).map_err(|e| {
            AuthError::config(format!(
                "can't write the token file {}: {e}",
                self.path.display()
            ))
        })
    }
}

/// Writes a file only the user can read, atomically (temporary file, then rename).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("enc.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

pub(crate) fn random<const N: usize>() -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf)
        .map_err(|e| AuthError::internal(format!("no OS random source: {e}")))?;
    Ok(buf)
}

impl TokenStore for FileStore {
    fn name(&self) -> &'static str {
        "file"
    }

    fn load(&self, account: &str) -> Result<Option<TokenRecord>> {
        validate_account(account).map_err(AuthError::usage)?;
        Ok(self.read()?.0.accounts.remove(account))
    }

    fn save(&self, record: &TokenRecord) -> Result<()> {
        validate_account(&record.account).map_err(AuthError::usage)?;
        let (mut plain, kdf) = self.read()?;
        plain
            .accounts
            .insert(record.account.clone(), record.clone());
        self.write(&plain, kdf)
    }

    fn delete(&self, account: &str) -> Result<bool> {
        validate_account(account).map_err(AuthError::usage)?;
        let (mut plain, kdf) = self.read()?;
        let existed = plain.accounts.remove(account).is_some();
        if existed {
            self.write(&plain, kdf)?;
        }
        Ok(existed)
    }

    fn accounts(&self) -> Result<Vec<String>> {
        Ok(self.read()?.0.accounts.into_keys().collect())
    }
}

// ---------------------------------------------------------------------------
// Choosing a store

/// The data directory: `FAMILIAR_DATA_DIR`, or `<local data dir>/Familiar`.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("FAMILIAR_DATA_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    dirs::data_local_dir()
        .map(|d| d.join("Familiar"))
        .ok_or_else(|| AuthError::config("no data directory; set FAMILIAR_DATA_DIR"))
}

fn passphrase() -> Result<Option<String>> {
    if let Ok(p) = std::env::var("FAMILIAR_TOKEN_PASSPHRASE") {
        if !p.is_empty() {
            return Ok(Some(p));
        }
    }
    if let Some(f) = std::env::var_os("FAMILIAR_TOKEN_PASSPHRASE_FILE").filter(|f| !f.is_empty()) {
        let p = std::fs::read_to_string(&f).map_err(|e| {
            AuthError::config(format!(
                "can't read FAMILIAR_TOKEN_PASSPHRASE_FILE {}: {e}",
                Path::new(&f).display()
            ))
        })?;
        return Ok(Some(p.trim_end_matches(['\r', '\n']).to_string()));
    }
    Ok(None)
}

fn file_store() -> Result<Option<FileStore>> {
    match passphrase()? {
        Some(p) => Ok(Some(FileStore::new(data_dir()?.join("tokens.enc"), p)?)),
        None => Ok(None),
    }
}

/// Opens the store `FAMILIAR_TOKEN_STORE` asks for (see the module docs).
pub fn open_store() -> Result<Box<dyn TokenStore>> {
    let choice = std::env::var("FAMILIAR_TOKEN_STORE").unwrap_or_default();
    match choice.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => {
            let keychain = Keyring::new(SERVICE).available();
            match keychain {
                Ok(()) => Ok(Box::new(KeychainStore::keychain())),
                Err(why) => match file_store()? {
                    Some(f) => Ok(Box::new(f)),
                    None => Err(AuthError::config(format!(
                        "no OS keychain is available ({why}). To keep tokens in an encrypted file \
                         instead, set FAMILIAR_TOKEN_PASSPHRASE or FAMILIAR_TOKEN_PASSPHRASE_FILE"
                    ))),
                },
            }
        }
        "keychain" => {
            Keyring::new(SERVICE)
                .available()
                .map_err(|why| AuthError::config(format!("the OS keychain is not available: {why}")))?;
            Ok(Box::new(KeychainStore::keychain()))
        }
        "file" => file_store()?.map(|f| Box::new(f) as Box<dyn TokenStore>).ok_or_else(|| {
            AuthError::config(
                "FAMILIAR_TOKEN_STORE=file needs FAMILIAR_TOKEN_PASSPHRASE or FAMILIAR_TOKEN_PASSPHRASE_FILE",
            )
        }),
        other => Err(AuthError::config(format!(
            "FAMILIAR_TOKEN_STORE must be auto, keychain or file, not `{other}`"
        ))),
    }
}

/// Reads the JSON of a store file without decrypting, for tests that check nothing leaks.
#[doc(hidden)]
pub fn raw_file(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::sample;

    fn roundtrip(store: &dyn TokenStore) {
        assert!(store.load("a").unwrap().is_none());
        assert!(store.accounts().unwrap().is_empty());
        let mut r = sample("a");
        store.save(&r).unwrap();
        store.save(&sample("b")).unwrap();
        assert_eq!(store.load("a").unwrap().unwrap(), r);
        assert_eq!(store.accounts().unwrap(), ["a", "b"]);
        // Shorter record: stale chunks must go.
        r.refresh_token = "short".into();
        r.access_token = None;
        store.save(&r).unwrap();
        assert_eq!(store.load("a").unwrap().unwrap(), r);
        assert!(store.delete("a").unwrap());
        assert!(!store.delete("a").unwrap());
        assert!(store.load("a").unwrap().is_none());
        assert_eq!(store.accounts().unwrap(), ["b"]);
        assert!(store.load("Bad Name").is_err());
    }

    #[test]
    fn memory_store_chunks_within_the_windows_limit() {
        let s = MemoryStore::memory();
        let r = sample("a");
        s.save(&r).unwrap();
        // 3,000 + 2,400 token bytes plus metadata: three chunks, an index and nothing over 2,560.
        assert!(s.secrets().len() >= 4, "{}", s.secrets().len());
        assert!(s.secrets().largest() <= 2560);
        s.delete("a").unwrap();
        assert!(s.secrets().is_empty(), "chunks and index removed");
        roundtrip(&s);
        s.delete("b").unwrap();
        assert!(s.secrets().is_empty(), "everything removed");
    }

    #[test]
    fn file_store_roundtrip_and_wrong_passphrase() {
        let dir = std::env::temp_dir().join(format!("familiar-auth-test-{}", std::process::id()));
        let path = dir.join("tokens.enc");
        let _ = std::fs::remove_file(&path);
        let s = FileStore::new(&path, "correct horse battery").unwrap();
        roundtrip(&s);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("rtrt") && !raw.contains("a@example.com"),
            "plaintext leaked"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let wrong = FileStore::new(&path, "wrong passphrase").unwrap();
        let e = wrong.load("b").unwrap_err();
        assert!(e.message.contains("wrong passphrase"), "{e}");
        // A fresh store with the right passphrase reads it (as after a restart).
        let again = FileStore::new(&path, "correct horse battery").unwrap();
        assert_eq!(again.accounts().unwrap(), ["b"]);
        assert!(FileStore::new(&path, "short").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Run by hand on a desktop: `cargo test -p familiar-auth -- --ignored keychain`.
    #[test]
    #[ignore = "touches the real OS keychain"]
    fn keychain_roundtrip() {
        let s = SecretStore::new(Keyring::new("familiar-test"), "keychain");
        for a in s.accounts().unwrap() {
            s.delete(&a).unwrap();
        }
        roundtrip(&s);
        s.delete("b").unwrap();
    }
}
