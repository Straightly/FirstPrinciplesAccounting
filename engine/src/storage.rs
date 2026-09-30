//! Encrypted single-file storage boundary — Impl Spec §3.
//!
//! `book.data.enc` holds the complete serialized event log (Impl Spec §3.1);
//! there is no separate reference-state blob because [`crate::engine::EngineState::replay`]
//! rebuilds every projection from the log alone (Theorem T1: nothing above
//! this module depends on the storage medium). `book.keystore.json` holds the
//! book key wrapped for the owner's passphrase (Impl Spec §5.4). Every
//! mutation batch rewrites the whole file via atomic replacement (temp +
//! fsync + rename) under a writer lock, then commits the book folder to a
//! local git repository used for backup/point-in-time recovery only.
//! `book.data.enc` remains the sole accounting source of truth (§3.1), while
//! a successful API mutation also requires its local Git checkpoint.

use crate::domain::EventRecord;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;
use rand::rngs::SysRng;
use rand::TryRng;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// Public (Impl Plan M9): `backend/src/books.rs` copies these two files
/// verbatim for backup/restore, without decrypting them — the filenames
/// are the only thing it needs from this module for that.
pub const DATA_FILE: &str = "book.data.enc";
pub const KEYSTORE_FILE: &str = "book.keystore.json";
const LOCK_FILE: &str = "book.lock";
const META_FILE: &str = "book.json";
const GITIGNORE_FILE: &str = ".gitignore";
const TMP_SUFFIX: &str = ".tmp";
const BOOK_KEY_LEN: usize = 32;
const GCM_NONCE_LEN: usize = 12;

fn fill_random(bytes: &mut [u8]) -> Result<(), StorageError> {
    SysRng
        .try_fill_bytes(bytes)
        .map_err(|e| StorageError::Crypto(format!("secure random generation failed: {e}")))
}

#[derive(Debug)]
pub enum StorageError {
    Io(String),
    /// Wrong passphrase, corrupt ciphertext, or an AEAD auth-tag mismatch.
    Crypto(String),
    /// Malformed JSON, unsupported keystore version/kdf, or a truncated file.
    Corrupt(String),
    /// Another writer currently holds `book.lock`.
    LockHeld,
    Git(String),
    /// The engine rejected an event during replay after load.
    Replay(crate::error::EngineError),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Io(m) => write!(f, "storage I/O error: {m}"),
            StorageError::Crypto(m) => write!(f, "storage crypto error: {m}"),
            StorageError::Corrupt(m) => write!(f, "storage corrupt: {m}"),
            StorageError::LockHeld => write!(f, "writer lock is already held"),
            StorageError::Git(m) => write!(f, "git backup error: {m}"),
            StorageError::Replay(e) => write!(f, "replay error: {e}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> StorageError {
        StorageError::Io(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Hex encoding (avoids pulling in a base64 dependency for a few short fields)
// ---------------------------------------------------------------------------

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, StorageError> {
    if !s.len().is_multiple_of(2) {
        return Err(StorageError::Corrupt("odd-length hex string".into()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| StorageError::Corrupt("invalid hex string".into()))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Book key wrapping — Impl Spec §5.4
// ---------------------------------------------------------------------------

/// Argon2id cost parameters. `PRODUCTION` follows the OWASP interactive
/// minimum; `TEST_FAST` trades security for speed so tests can open/close a
/// book many times without spending real wall-clock time on key derivation.
#[derive(Debug, Clone, Copy)]
pub struct Argon2Profile {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Argon2Profile {
    pub const PRODUCTION: Argon2Profile = Argon2Profile {
        m_cost_kib: 19_456,
        t_cost: 2,
        p_cost: 1,
    };

    pub const TEST_FAST: Argon2Profile = Argon2Profile {
        m_cost_kib: 8,
        t_cost: 1,
        p_cost: 1,
    };
}

/// `book.keystore.json` — the book key wrapped by a passphrase-derived key.
/// Never contains a plaintext key or passphrase (Impl Spec §5.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeystoreFile {
    pub version: u32,
    pub kdf: String,
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt_hex: String,
    pub wrap_nonce_hex: String,
    pub wrapped_key_hex: String,
}

/// Impl Spec §5.4: later providers (OS keystore, KMS, HSM) implement this
/// trait without changing the engine or the on-disk format.
pub trait BookKeyProvider: Send + Sync {
    fn wrap(&self, book_key: &[u8; BOOK_KEY_LEN]) -> Result<KeystoreFile, StorageError>;
    fn unwrap(&self, keystore: &KeystoreFile) -> Result<[u8; BOOK_KEY_LEN], StorageError>;
}

/// v1's only provider: Argon2id-derived wrapping key over the owner's
/// passphrase, AES-256-GCM for the wrap itself.
pub struct PassphraseKeyProvider {
    passphrase: String,
    profile: Argon2Profile,
}

impl PassphraseKeyProvider {
    pub fn new(passphrase: impl Into<String>) -> PassphraseKeyProvider {
        PassphraseKeyProvider {
            passphrase: passphrase.into(),
            profile: Argon2Profile::PRODUCTION,
        }
    }

    pub fn with_profile(
        passphrase: impl Into<String>,
        profile: Argon2Profile,
    ) -> PassphraseKeyProvider {
        PassphraseKeyProvider {
            passphrase: passphrase.into(),
            profile,
        }
    }

    fn derive_wrap_key(
        &self,
        salt: &[u8],
        profile: Argon2Profile,
    ) -> Result<[u8; 32], StorageError> {
        let params = Params::new(profile.m_cost_kib, profile.t_cost, profile.p_cost, Some(32))
            .map_err(|e| StorageError::Crypto(format!("bad argon2 params: {e}")))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut out = [0u8; 32];
        argon2
            .hash_password_into(self.passphrase.as_bytes(), salt, &mut out)
            .map_err(|e| StorageError::Crypto(format!("argon2 derivation failed: {e}")))?;
        Ok(out)
    }
}

impl BookKeyProvider for PassphraseKeyProvider {
    fn wrap(&self, book_key: &[u8; BOOK_KEY_LEN]) -> Result<KeystoreFile, StorageError> {
        let mut salt = [0u8; 16];
        fill_random(&mut salt)?;
        let wrap_key = self.derive_wrap_key(&salt, self.profile)?;
        let cipher = Aes256Gcm::new_from_slice(&wrap_key)
            .map_err(|e| StorageError::Crypto(format!("bad wrap key: {e}")))?;
        let mut nonce_bytes = [0u8; GCM_NONCE_LEN];
        fill_random(&mut nonce_bytes)?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, book_key.as_slice())
            .map_err(|_| StorageError::Crypto("book key wrap failed".into()))?;
        Ok(KeystoreFile {
            version: 1,
            kdf: "argon2id".into(),
            m_cost_kib: self.profile.m_cost_kib,
            t_cost: self.profile.t_cost,
            p_cost: self.profile.p_cost,
            salt_hex: to_hex(&salt),
            wrap_nonce_hex: to_hex(&nonce_bytes),
            wrapped_key_hex: to_hex(&ciphertext),
        })
    }

    fn unwrap(&self, keystore: &KeystoreFile) -> Result<[u8; BOOK_KEY_LEN], StorageError> {
        if keystore.kdf != "argon2id" {
            return Err(StorageError::Corrupt(format!(
                "unsupported kdf {}",
                keystore.kdf
            )));
        }
        let salt = from_hex(&keystore.salt_hex)?;
        let profile = Argon2Profile {
            m_cost_kib: keystore.m_cost_kib,
            t_cost: keystore.t_cost,
            p_cost: keystore.p_cost,
        };
        let wrap_key = self.derive_wrap_key(&salt, profile)?;
        let cipher = Aes256Gcm::new_from_slice(&wrap_key)
            .map_err(|e| StorageError::Crypto(format!("bad wrap key: {e}")))?;
        let nonce_bytes = from_hex(&keystore.wrap_nonce_hex)?;
        if nonce_bytes.len() != GCM_NONCE_LEN {
            return Err(StorageError::Corrupt("wrap nonce has wrong length".into()));
        }
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = from_hex(&keystore.wrapped_key_hex)?;
        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_slice())
            .map_err(|_| StorageError::Crypto("wrong passphrase or corrupt keystore".into()))?;
        plaintext
            .try_into()
            .map_err(|_| StorageError::Corrupt("unwrapped key has wrong length".into()))
    }
}

// ---------------------------------------------------------------------------
// Event log encryption
// ---------------------------------------------------------------------------

fn encrypt_events(
    book_key: &[u8; BOOK_KEY_LEN],
    events: &[EventRecord],
) -> Result<Vec<u8>, StorageError> {
    let plaintext = serde_json::to_vec(events).map_err(|e| StorageError::Corrupt(e.to_string()))?;
    let cipher = Aes256Gcm::new_from_slice(book_key)
        .map_err(|e| StorageError::Crypto(format!("bad book key: {e}")))?;
    let mut nonce_bytes = [0u8; GCM_NONCE_LEN];
    fill_random(&mut nonce_bytes)?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_slice())
        .map_err(|_| StorageError::Crypto("book encryption failed".into()))?;
    let mut out = Vec::with_capacity(GCM_NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt_events(
    book_key: &[u8; BOOK_KEY_LEN],
    data: &[u8],
) -> Result<Vec<EventRecord>, StorageError> {
    if data.len() < GCM_NONCE_LEN {
        return Err(StorageError::Corrupt("book file too short".into()));
    }
    let (nonce_bytes, ciphertext) = data.split_at(GCM_NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(book_key)
        .map_err(|e| StorageError::Crypto(format!("bad book key: {e}")))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|_| {
        StorageError::Crypto("book decryption failed (wrong key or corrupt file)".into())
    })?;
    serde_json::from_slice(&plaintext).map_err(|e| StorageError::Corrupt(e.to_string()))
}

// ---------------------------------------------------------------------------
// Atomic replacement + writer lock — Impl Spec §3.1
// ---------------------------------------------------------------------------

fn tmp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .expect("data file path always has a file name")
        .to_string_lossy();
    path.with_file_name(format!("{file_name}{TMP_SUFFIX}"))
}

async fn atomic_write(path: &Path, data: &[u8]) -> Result<(), StorageError> {
    let tmp_path = tmp_path_for(path);
    let mut file = tokio::fs::File::create(&tmp_path).await?;
    file.write_all(data).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
}

/// Held for the duration of a mutation batch's persist; released on drop.
pub struct WriterLockGuard {
    path: PathBuf,
}

impl Drop for WriterLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn acquire_writer_lock(dir: &Path) -> Result<WriterLockGuard, StorageError> {
    let path = dir.join(LOCK_FILE);
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
    {
        Ok(_) => Ok(WriterLockGuard { path }),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(StorageError::LockHeld),
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------
// Git backup policy — Impl Spec §3.3
// ---------------------------------------------------------------------------

async fn run_git(dir: &Path, args: &[&str]) -> Result<std::process::Output, StorageError> {
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|e| StorageError::Git(format!("failed to run git: {e}")))
}

fn require_git_success(
    action: &str,
    output: std::process::Output,
) -> Result<std::process::Output, StorageError> {
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(StorageError::Git(if stderr.is_empty() {
        format!("git {action} failed with {}", output.status)
    } else {
        format!("git {action} failed: {stderr}")
    }))
}

async fn ensure_git_executable(program: &str) -> Result<(), StorageError> {
    let output = tokio::process::Command::new(program)
        .arg("--version")
        .output()
        .await
        .map_err(|e| StorageError::Git(format!("required git executable is unavailable: {e}")))?;
    require_git_success("--version", output)?;
    Ok(())
}

async fn git_init(dir: &Path) -> Result<(), StorageError> {
    if tokio::fs::try_exists(dir.join(".git"))
        .await
        .unwrap_or(false)
    {
        return Ok(());
    }
    require_git_success("init", run_git(dir, &["init", "-q"]).await?)?;
    require_git_success(
        "config user.email",
        run_git(dir, &["config", "user.email", "ledgerzero@local"]).await?,
    )?;
    require_git_success(
        "config user.name",
        run_git(dir, &["config", "user.name", "LedgerZero"]).await?,
    )?;
    Ok(())
}

/// Stages only durable book files and commits them. Transient locks and temp
/// files never enter history. A missing or failing Git executable is reported
/// to the caller instead of silently producing a book without checkpoints.
async fn git_commit(dir: &Path, message: &str) -> Result<(), StorageError> {
    if !tokio::fs::try_exists(dir.join(GITIGNORE_FILE))
        .await
        .unwrap_or(false)
    {
        tokio::fs::write(
            dir.join(GITIGNORE_FILE),
            format!("{LOCK_FILE}\n*{TMP_SUFFIX}\n"),
        )
        .await?;
    }
    let mut durable_files = vec![GITIGNORE_FILE, DATA_FILE, KEYSTORE_FILE];
    if tokio::fs::try_exists(dir.join(META_FILE))
        .await
        .unwrap_or(false)
    {
        durable_files.push(META_FILE);
    }
    let mut add_args = vec!["add", "--"];
    add_args.extend(durable_files);
    require_git_success("add", run_git(dir, &add_args).await?)?;
    require_git_success(
        "rm cached book.lock",
        run_git(
            dir,
            &[
                "rm",
                "--cached",
                "--ignore-unmatch",
                "--quiet",
                "--",
                LOCK_FILE,
            ],
        )
        .await?,
    )?;

    let staged = run_git(dir, &["diff", "--cached", "--quiet"]).await?;
    match staged.status.code() {
        Some(0) => return Ok(()),
        Some(1) => {}
        _ => {
            return Err(StorageError::Git(format!(
                "git diff --cached failed: {}",
                String::from_utf8_lossy(&staged.stderr).trim()
            )))
        }
    }
    require_git_success(
        "commit",
        run_git(dir, &["commit", "-q", "-m", message]).await?,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Storage trait + file driver — Impl Spec §3.2
// ---------------------------------------------------------------------------

/// The storage boundary the runtime backend uses (Impl Spec §3.2, §3.4).
/// Because the on-disk format is the append-ordered event log alone (§3.1),
/// the trait reduces to load-the-log / persist-the-log; every projection and
/// index is rebuilt above this boundary via [`crate::engine::EngineState::replay`].
#[async_trait]
pub trait BookStorage: Send + Sync {
    async fn load(&self) -> Result<Vec<EventRecord>, StorageError>;

    /// Rewrites the whole encrypted log (O(N) per mutation, per §3.1) and
    /// commits the durable book files to its backup git repo.
    /// `new_event_ids` is used only for the commit message.
    async fn persist(
        &self,
        all_events: &[EventRecord],
        new_event_ids: &[Uuid],
    ) -> Result<(), StorageError>;
}

/// The v1 driver: one encrypted file per book folder (Impl Spec §3.1).
pub struct FileBookStore {
    dir: PathBuf,
    book_key: [u8; BOOK_KEY_LEN],
}

impl FileBookStore {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Fails before book creation starts when the required Git runtime is
    /// unavailable or unusable.
    pub async fn ensure_git_available() -> Result<(), StorageError> {
        ensure_git_executable("git").await
    }

    /// Checkpoints durable metadata updates made by the registry after an
    /// earlier encrypted-data/key mutation.
    pub async fn checkpoint(&self, message: &str) -> Result<(), StorageError> {
        git_init(&self.dir).await?;
        git_commit(&self.dir, message).await
    }

    /// Rewraps the already-open in-memory book key for a successor owner.
    /// Book data is not decrypted/re-encrypted and the raw key never leaves
    /// this storage boundary.
    pub async fn rewrap_key(&self, key_provider: &dyn BookKeyProvider) -> Result<(), StorageError> {
        let keystore = key_provider.wrap(&self.book_key)?;
        let bytes = serde_json::to_vec_pretty(&keystore)
            .map_err(|e| StorageError::Corrupt(e.to_string()))?;
        atomic_write(&self.dir.join(KEYSTORE_FILE), &bytes).await?;
        git_init(&self.dir).await?;
        git_commit(&self.dir, "book owner changed; key rewrapped").await?;
        Ok(())
    }

    /// Bootstraps a brand-new book folder: random book key wrapped for the
    /// given key provider, an empty encrypted event log, and a fresh backup
    /// Git repository. The caller's first persist creates the initial complete
    /// checkpoint after `book.json` has also been written.
    pub async fn create(
        dir: &Path,
        key_provider: &dyn BookKeyProvider,
    ) -> Result<FileBookStore, StorageError> {
        tokio::fs::create_dir_all(dir).await?;
        if tokio::fs::try_exists(dir.join(DATA_FILE))
            .await
            .unwrap_or(false)
        {
            return Err(StorageError::Corrupt(
                "a book already exists in this folder".into(),
            ));
        }

        let mut book_key = [0u8; BOOK_KEY_LEN];
        fill_random(&mut book_key)?;
        let keystore = key_provider.wrap(&book_key)?;
        let keystore_json = serde_json::to_vec_pretty(&keystore)
            .map_err(|e| StorageError::Corrupt(e.to_string()))?;
        tokio::fs::write(dir.join(KEYSTORE_FILE), keystore_json).await?;

        let store = FileBookStore {
            dir: dir.to_path_buf(),
            book_key,
        };
        let empty: Vec<EventRecord> = Vec::new();
        let encrypted = encrypt_events(&store.book_key, &empty)?;
        atomic_write(&store.dir.join(DATA_FILE), &encrypted).await?;

        tokio::fs::write(
            store.dir.join(GITIGNORE_FILE),
            format!("{LOCK_FILE}\n*{TMP_SUFFIX}\n"),
        )
        .await?;

        git_init(&store.dir).await?;
        Ok(store)
    }

    /// Opens an existing book folder: unwraps the book key (wrong
    /// passphrase surfaces as `StorageError::Crypto`) and decrypts the event
    /// log. Does not replay — callers build `EngineState` via
    /// `EngineState::replay(book_id, &events)` from the returned events.
    pub async fn open(
        dir: &Path,
        key_provider: &dyn BookKeyProvider,
    ) -> Result<(FileBookStore, Vec<EventRecord>), StorageError> {
        let keystore_bytes = tokio::fs::read(dir.join(KEYSTORE_FILE)).await?;
        let keystore: KeystoreFile = serde_json::from_slice(&keystore_bytes)
            .map_err(|e| StorageError::Corrupt(e.to_string()))?;
        let book_key = key_provider.unwrap(&keystore)?;
        let store = FileBookStore {
            dir: dir.to_path_buf(),
            book_key,
        };
        let events = store.load().await?;
        Ok((store, events))
    }
}

#[async_trait]
impl BookStorage for FileBookStore {
    async fn load(&self) -> Result<Vec<EventRecord>, StorageError> {
        let data = tokio::fs::read(self.dir.join(DATA_FILE)).await?;
        decrypt_events(&self.book_key, &data)
    }

    async fn persist(
        &self,
        all_events: &[EventRecord],
        new_event_ids: &[Uuid],
    ) -> Result<(), StorageError> {
        {
            let _lock = acquire_writer_lock(&self.dir).await?;
            let encrypted = encrypt_events(&self.book_key, all_events)?;
            atomic_write(&self.dir.join(DATA_FILE), &encrypted).await?;
        }
        let message = if new_event_ids.is_empty() {
            format!("mutation batch ({} events total)", all_events.len())
        } else {
            let ids: Vec<String> = new_event_ids.iter().map(Uuid::to_string).collect();
            format!("mutation batch: {}", ids.join(", "))
        };
        // A restored book folder (Impl Plan M9) has no `.git` of its own — it's
        // recreated from just the three portable files. Without this, `git -C`
        // would walk up and commit into whatever parent repo happens to enclose
        // `books/`. Idempotent: no-ops if `.git` already exists.
        git_init(&self.dir).await?;
        git_commit(&self.dir, &message).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_git_runtime_returns_a_clear_error() {
        let result = ensure_git_executable("ledgerzero-test-git-that-does-not-exist").await;
        match result {
            Err(StorageError::Git(message)) => {
                assert!(message.contains("required git executable is unavailable"));
            }
            other => panic!("expected a clear Git dependency error, got {other:?}"),
        }
    }
}
