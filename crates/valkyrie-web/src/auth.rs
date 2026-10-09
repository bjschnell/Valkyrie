//! Who may use the web app. A browser pairs once with a one-time code (shown as a
//! QR code by `valk web` or `valk web pair`) and gets a device token for good. Only
//! SHA-256 hashes of codes and tokens are written to disk. A device may also leave a
//! push subscription, which goes when the device is revoked.

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

impl Device {
    /// Names the device on its push subscriptions, without the token.
    pub fn id(&self) -> &str {
        &self.token_sha256
    }
}

/// Where and how to reach one browser by Web Push (its `PushSubscription`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    /// The paired device it belongs to (`Device::id`).
    #[serde(default)]
    pub device: String,
    pub endpoint: String,
    /// The browser's P-256 public key and auth secret, base64url.
    pub p256dh: String,
    pub auth: String,
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

    /// Forgets devices by name (or every one, for `all`), and their push
    /// subscriptions. Returns how many devices.
    pub fn revoke(&self, name: &str) -> Result<usize> {
        let _guard = self.lock.lock().unwrap();
        let mut devices: Vec<Device> = self.read("devices.json");
        let before = devices.len();
        devices.retain(|d| name != "all" && d.name != name);
        self.write("devices.json", &devices)?;
        let mut subs: Vec<Subscription> = self.read("subscriptions.json");
        subs.retain(|s| devices.iter().any(|d| d.id() == s.device));
        self.write("subscriptions.json", &subs)?;
        Ok(before - devices.len())
    }

    /// Adds (or replaces) a device's push subscription.
    pub fn subscribe(&self, device: &Device, mut sub: Subscription) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        sub.device = device.id().to_owned();
        let mut subs: Vec<Subscription> = self.read("subscriptions.json");
        subs.retain(|s| s.endpoint != sub.endpoint);
        subs.push(sub);
        self.write("subscriptions.json", &subs)
    }

    /// Drops a push subscription, by its endpoint.
    pub fn unsubscribe(&self, endpoint: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let mut subs: Vec<Subscription> = self.read("subscriptions.json");
        subs.retain(|s| s.endpoint != endpoint);
        self.write("subscriptions.json", &subs)
    }

    /// Push subscriptions of devices still paired.
    pub fn subscriptions(&self) -> Vec<Subscription> {
        let devices = self.devices();
        let mut subs: Vec<Subscription> = self.read("subscriptions.json");
        subs.retain(|s| devices.iter().any(|d| d.id() == s.device));
        subs
    }

    /// This server's VAPID signing key (RFC 8292), made on first use. Browsers bind
    /// their subscriptions to its public half, so it must stay the same.
    pub fn vapid_secret(&self) -> Result<[u8; 32]> {
        #[derive(Serialize, Deserialize)]
        struct Vapid {
            secret: String,
        }
        let _guard = self.lock.lock().unwrap();
        let stored: Vec<Vapid> = self.read("vapid.json");
        if let Some(bytes) = stored
            .first()
            .and_then(|v| URL_SAFE_NO_PAD.decode(&v.secret).ok())
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
        {
            return Ok(bytes);
        }
        let secret = crate::push::new_secret()?;
        self.write(
            "vapid.json",
            &[Vapid {
                secret: URL_SAFE_NO_PAD.encode(secret),
            }],
        )?;
        Ok(secret)
    }

    fn read<T: for<'de> Deserialize<'de>>(&self, file: &str) -> Vec<T> {
        std::fs::read(self.dir.join(file))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn write<T: Serialize>(&self, file: &str, value: &T) -> Result<()> {
        use std::io::Write;
        let path = self.dir.join(file);
        let tmp = self.dir.join(format!(".{file}.tmp"));
        let mut out = valkyrie_proto::private_file()
            .write(true)
            .create(true)
            .truncate(true)
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
        let device = store.check(&token).unwrap();
        let sub = Subscription {
            device: String::new(),
            endpoint: "https://push.example/1".into(),
            p256dh: "k".into(),
            auth: "a".into(),
        };
        store.subscribe(&device, sub.clone()).unwrap();
        store.subscribe(&device, sub).unwrap();
        assert_eq!(store.subscriptions().len(), 1, "one per endpoint");
        assert_eq!(store.subscriptions()[0].device, device.id());
        let key = store.vapid_secret().unwrap();
        assert_eq!(store.vapid_secret().unwrap(), key, "the VAPID key stays");
        assert_eq!(store.revoke("phone").unwrap(), 1);
        assert!(store.check(&token).is_none());
        assert!(store.subscriptions().is_empty(), "revoking drops its push");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
