//! Who may use the web app. A browser pairs once with a one-time code (shown as a
//! QR code by `valk web` or `valk web pair`) and gets a device token for good. Only
//! SHA-256 hashes of codes and tokens are written to disk.

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// How long a pairing code works.
pub const CODE_TTL_SECS: u64 = 10 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub name: String,
    token_sha256: String,
    pub created_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Code {
    code_sha256: String,
    expires_unix: u64,
}

/// Paired devices and open pairing codes, as files in a private directory, so
/// `valk web pair` can hand the running server a new code.
pub struct Store {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        valkyrie_proto::ensure_private_dir(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            lock: Mutex::new(()),
        })
    }

    /// A fresh single-use pairing code.
    pub fn new_code(&self) -> Result<String> {
        let _guard = self.lock.lock().unwrap();
        let code = random_token()?;
        let now = now_unix();
        let mut codes: Vec<Code> = self.read("codes.json");
        codes.retain(|c| c.expires_unix > now);
        codes.push(Code {
            code_sha256: sha256(&code),
            expires_unix: now + CODE_TTL_SECS,
        });
        self.write("codes.json", &codes)?;
        Ok(code)
    }

    /// Trades a pairing code for a device token, once.
    pub fn pair(&self, code: &str, name: &str) -> Result<String> {
        let _guard = self.lock.lock().unwrap();
        let now = now_unix();
        let mut codes: Vec<Code> = self.read("codes.json");
        codes.retain(|c| c.expires_unix > now);
        let hash = sha256(code);
        let before = codes.len();
        codes.retain(|c| c.code_sha256 != hash);
        let matched = codes.len() != before;
        self.write("codes.json", &codes)?;
        anyhow::ensure!(matched, "that pairing code is wrong or expired");
        let token = random_token()?;
        let mut devices: Vec<Device> = self.read("devices.json");
        let name = name.trim();
        devices.push(Device {
            name: if name.is_empty() {
                "device".into()
            } else {
                name.chars().take(60).collect()
            },
            token_sha256: sha256(&token),
            created_unix: now,
        });
        self.write("devices.json", &devices)?;
        Ok(token)
    }

    /// The device a token belongs to.
    pub fn check(&self, token: &str) -> Option<Device> {
        let hash = sha256(token);
        let devices: Vec<Device> = self.read("devices.json");
        devices.into_iter().find(|d| d.token_sha256 == hash)
    }

    pub fn devices(&self) -> Vec<Device> {
        self.read("devices.json")
    }

    /// Forgets devices by name (or every one, for `all`). Returns how many.
    pub fn revoke(&self, name: &str) -> Result<usize> {
        let _guard = self.lock.lock().unwrap();
        let mut devices: Vec<Device> = self.read("devices.json");
        let before = devices.len();
        devices.retain(|d| name != "all" && d.name != name);
        self.write("devices.json", &devices)?;
        Ok(before - devices.len())
    }

    fn read<T: for<'de> Deserialize<'de>>(&self, file: &str) -> Vec<T> {
        std::fs::read(self.dir.join(file))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn write<T: Serialize>(&self, file: &str, value: &T) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.dir.join(file);
        let tmp = self.dir.join(format!(".{file}.tmp"));
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("write {}", tmp.display()))?;
        out.write_all(&serde_json::to_vec_pretty(value)?)?;
        out.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// 32 random bytes, URL-safe.
fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("random: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn sha256(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_pairs_once_and_its_token_checks() {
        let dir = std::env::temp_dir().join(format!("valkyrie-web-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir).unwrap();
        let code = store.new_code().unwrap();
        assert!(store.pair("wrong", "phone").is_err());
        let token = store.pair(&code, " phone ").unwrap();
        assert!(store.pair(&code, "again").is_err(), "codes are single-use");
        assert_eq!(store.check(&token).unwrap().name, "phone");
        assert!(store.check("not-a-token").is_none());
        // Nothing secret on disk.
        let disk = std::fs::read_to_string(dir.join("devices.json")).unwrap();
        assert!(!disk.contains(&token));
        assert_eq!(store.revoke("phone").unwrap(), 1);
        assert!(store.check(&token).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
