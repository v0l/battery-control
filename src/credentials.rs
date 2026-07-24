//! Persistent credential storage for backends that need to remember pairing
//! state across connects (e.g. an Anker bind userId, an EcoFlow user_id).
//!
//! Backends read/write opaque byte blobs keyed by a stable string (typically
//! `"<backend>:<serial>"`). The **host application** chooses where these live by
//! installing a [`CredentialStore`] via [`set_store`]; if none is installed a
//! file-based default is used ([`FileStore`]) at `$BATTERY_CONTROL_STATE_DIR`
//! (or a per-user config dir), so the CLI works out of the box while apps can
//! redirect to their own sandboxed storage.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, RwLock};

/// A pluggable key→bytes credential store.
pub trait CredentialStore: Send + Sync {
    /// Load a previously saved credential.
    fn load(&self, key: &str) -> Option<Vec<u8>>;
    /// Persist a credential (overwrites any previous value).
    fn save(&self, key: &str, value: &[u8]);
    /// Remove a stored credential (e.g. on unbind).
    fn delete(&self, key: &str);
}

static STORE: OnceLock<Box<dyn CredentialStore>> = OnceLock::new();

/// Install the process-wide credential store. Call once at startup, before any
/// connect. A second call is ignored (returns `Err` with the store back).
pub fn set_store(store: Box<dyn CredentialStore>) -> Result<(), Box<dyn CredentialStore>> {
    STORE.set(store)
}

/// The active store — the installed one, or a lazily-created [`FileStore`].
fn store() -> &'static dyn CredentialStore {
    if let Some(s) = STORE.get() {
        return s.as_ref();
    }
    static DEFAULT: OnceLock<FileStore> = OnceLock::new();
    DEFAULT.get_or_init(FileStore::default)
}

/// Load a credential from the active store.
pub fn load(key: &str) -> Option<Vec<u8>> {
    store().load(key)
}

/// Save a credential to the active store.
pub fn save(key: &str, value: &[u8]) {
    store().save(key, value)
}

/// Delete a credential from the active store.
pub fn delete(key: &str) {
    store().delete(key)
}

/// A simple JSON-file-backed store (hex-encoded values), used when the host
/// installs none. Path: `$BATTERY_CONTROL_STATE_DIR/credentials.json`, else a
/// per-user config dir, else the system temp dir.
pub struct FileStore {
    path: PathBuf,
    cache: RwLock<HashMap<String, String>>,
    write_lock: Mutex<()>,
}

impl Default for FileStore {
    fn default() -> Self {
        Self::new(default_path())
    }
}

fn default_path() -> PathBuf {
    if let Ok(dir) = std::env::var("BATTERY_CONTROL_STATE_DIR") {
        return PathBuf::from(dir).join("credentials.json");
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("battery_control").join("credentials.json")
}

impl FileStore {
    /// A file store at an explicit path.
    pub fn new(path: PathBuf) -> Self {
        let cache = load_file(&path).unwrap_or_default();
        Self {
            path,
            cache: RwLock::new(cache),
            write_lock: Mutex::new(()),
        }
    }

    fn flush(&self, map: &HashMap<String, String>) {
        let _g = self.write_lock.lock().unwrap();
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Plain `key\thexvalue` lines (keys never contain a tab or newline).
        let mut out = String::new();
        for (k, v) in map {
            out.push_str(k);
            out.push('\t');
            out.push_str(v);
            out.push('\n');
        }
        let _ = std::fs::write(&self.path, out);
    }
}

fn load_file(path: &PathBuf) -> Option<HashMap<String, String>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut map = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('\t') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    Some(map)
}

impl CredentialStore for FileStore {
    fn load(&self, key: &str) -> Option<Vec<u8>> {
        let hex = self.cache.read().unwrap().get(key).cloned()?;
        decode_hex(&hex)
    }

    fn save(&self, key: &str, value: &[u8]) {
        let mut map = self.cache.write().unwrap();
        map.insert(key.to_string(), encode_hex(value));
        self.flush(&map);
    }

    fn delete(&self, key: &str) {
        let mut map = self.cache.write().unwrap();
        if map.remove(key).is_some() {
            self.flush(&map);
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bc_cred_test_{}", std::process::id()));
        let path = dir.join("credentials.json");
        let _ = std::fs::remove_dir_all(&dir);

        let store = FileStore::new(path.clone());
        assert_eq!(store.load("anker:SN123"), None);
        store.save("anker:SN123", b"ankerrs");
        assert_eq!(store.load("anker:SN123"), Some(b"ankerrs".to_vec()));

        // Reload from disk in a fresh instance.
        let store2 = FileStore::new(path);
        assert_eq!(store2.load("anker:SN123"), Some(b"ankerrs".to_vec()));
        store2.delete("anker:SN123");
        assert_eq!(store2.load("anker:SN123"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
