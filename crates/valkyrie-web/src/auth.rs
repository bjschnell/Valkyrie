//! Who may use the web app. A browser pairs once with a one-time code and gets a
//! device token that holds until it is revoked (ADR-0009). The first device pairs
//! with a link from `valk web pair` (a QR code); any paired device, or the TUI, can
//! then add another with a short code typed into its pairing screen. Only SHA-256
//! hashes of codes and tokens are written to disk; a short code's hash hides it from
//! no one who can read the file, since its six characters are quick to try, but
//! such a process could write `devices.json` anyway. A device may also leave a push
//! subscription, which goes when the device is revoked.

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// How long a pairing link's code works.
pub const CODE_TTL_SECS: u64 = 10 * 60;
/// How long a short code works.
pub const SHORT_CODE_TTL_SECS: u64 = 5 * 60;
/// Wrong codes, in all, that end the open short codes: six characters are few
/// enough to guess at without a limit.
pub const SHORT_CODE_TRIES: u32 = 5;
/// Short codes' characters: Crockford's base32, without the look-alikes I, L, O, U.
const SHORT_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const SHORT_LEN: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub name: String,
    token_sha256: String,
    pub created_unix: u64,
    /// The paired device that added this one; `None` for this machine.
    #[serde(default)]
    pub added_by: Option<String>,
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
    /// A short code, typed in rather than opened as a link.
    #[serde(default)]
    short: bool,
    /// Names a short code to whoever shows it. Random, not from the code: a hash
    /// of six characters gives them away.
    #[serde(default)]
    id: String,
    /// Wrong codes tried while this one was open.
    #[serde(default)]
    failures: u32,
    /// The device that made it; `None` for this machine.
    #[serde(default)]
    by: Option<String>,
    /// The device a short code paired. It stays until it expires, so whoever
    /// shows it can see it was used.
    #[serde(default)]
    paired: Option<String>,
}

/// A short code just made, for a person to type on the new device.
#[derive(Debug, Clone, Serialize)]
pub struct ShortCode {
    /// As shown: `KQ7-4M2`.
    pub code: String,
    /// Names the code when asking whether it was used, without the code itself.
    pub id: String,
    pub expires_unix: u64,
}

/// What became of a short code.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CodeState {
    Open,
    Paired {
        device: String,
    },
    /// Expired, or ended by wrong tries.
    Gone,
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

    /// A fresh single-use code for a pairing link.
    pub fn new_code(&self) -> Result<String> {
        let code = random_token()?;
        self.open_code(&code, CODE_TTL_SECS, None, None)?;
        Ok(code)
    }

    /// A fresh single-use short code, made by `by` (`None`: this machine).
    pub fn new_short_code(&self, by: Option<&Device>) -> Result<ShortCode> {
        let mut bytes = [0u8; SHORT_LEN];
        getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("random: {e}"))?;
        let code: String = bytes
            .iter()
            .map(|b| SHORT_ALPHABET[usize::from(b % 32)] as char)
            .collect();
        let id = random_token()?;
        let expires_unix = self.open_code(
            &code,
            SHORT_CODE_TTL_SECS,
            Some(id.clone()),
            by.map(|d| d.name.clone()),
        )?;
        Ok(ShortCode {
            code: format!("{}-{}", &code[..3], &code[3..]),
            id,
            expires_unix,
        })
    }

    /// Opens `code`; a short one has an `id`.
    fn open_code(
        &self,
        code: &str,
        ttl: u64,
        id: Option<String>,
        by: Option<String>,
    ) -> Result<u64> {
        let _guard = self.lock()?;
        let now = now_unix();
        let mut codes: Vec<Code> = self.read("codes.json");
        codes.retain(|c| c.expires_unix > now);
        codes.push(Code {
            code_sha256: sha256(code),
            expires_unix: now + ttl,
            short: id.is_some(),
            id: id.unwrap_or_default(),
            failures: 0,
            by,
            paired: None,
        });
        self.write("codes.json", &codes)?;
        Ok(now + ttl)
    }

    /// What became of the short code `id` names.
    pub fn code_state(&self, id: &str) -> CodeState {
        let now = now_unix();
        let codes: Vec<Code> = self.read("codes.json");
        match codes
            .iter()
            .find(|c| c.short && !id.is_empty() && c.id == id && c.expires_unix > now)
        {
            Some(Code {
                paired: Some(device),
                ..
            }) => CodeState::Paired {
                device: device.clone(),
            },
            Some(_) => CodeState::Open,
            None => CodeState::Gone,
        }
    }

    /// Trades a code, a link's or a short one, for a device token, once.
    pub fn pair(&self, code: &str, name: &str) -> Result<String> {
        let _guard = self.lock()?;
        let now = now_unix();
        let mut codes: Vec<Code> = self.read("codes.json");
        codes.retain(|c| c.expires_unix > now);
        let hash = sha256(&short_form(code).unwrap_or_else(|| code.trim().to_owned()));
        let Some(i) = codes
            .iter()
            .position(|c| c.code_sha256 == hash && c.paired.is_none())
        else {
            let mut counted = false;
            for c in codes.iter_mut().filter(|c| c.short && c.paired.is_none()) {
                c.failures += 1;
                counted = true;
            }
            if counted {
                codes.retain(|c| c.failures < SHORT_CODE_TRIES);
                self.write("codes.json", &codes)?;
            }
            anyhow::bail!("that pairing code is wrong or expired");
        };
        let name = name.trim();
        let name: String = if name.is_empty() {
            "device".into()
        } else {
            name.chars().take(60).collect()
        };
        let added_by = codes[i].by.clone();
        if codes[i].short {
            codes[i].paired = Some(name.clone());
        } else {
            codes.remove(i);
        }
        self.write("codes.json", &codes)?;
        let token = random_token()?;
        let mut devices: Vec<Device> = self.read("devices.json");
        devices.push(Device {
            name,
            token_sha256: sha256(&token),
            created_unix: now,
            added_by,
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
        let _guard = self.lock()?;
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
        let _guard = self.lock()?;
        sub.device = device.id().to_owned();
        let mut subs: Vec<Subscription> = self.read("subscriptions.json");
        subs.retain(|s| s.endpoint != sub.endpoint);
        subs.push(sub);
        self.write("subscriptions.json", &subs)
    }

    /// Drops a push subscription, by its endpoint.
    pub fn unsubscribe(&self, endpoint: &str) -> Result<()> {
        let _guard = self.lock()?;
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
        let _guard = self.lock()?;
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

    /// Holds off other changes to the store until dropped: this process's, and
    /// another's (`valk web pair` or the TUI make codes while the server pairs, and a
    /// lost write could reopen a used code or forget wrong tries).
    fn lock(&self) -> Result<(std::sync::MutexGuard<'_, ()>, std::fs::File)> {
        let guard = self.lock.lock().unwrap();
        let file = valkyrie_proto::private_file()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join(".lock"))
            .context("open the store's lock")?;
        file.lock().context("lock the store")?;
        Ok((guard, file))
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

/// A short code as typed (`kq7 4m2`, `KQ7-4M2`, an O for a zero), the way it was
/// made; `None` if it isn't one.
fn short_form(typed: &str) -> Option<String> {
    let code: String = typed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    (code.len() == SHORT_LEN && code.bytes().all(|b| SHORT_ALPHABET.contains(&b))).then_some(code)
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

    /// Saved devices are found by this hash, so it must not change with a dependency.
    #[test]
    fn hashes_like_the_fips_example() {
        assert_eq!(
            sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

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

    fn store(name: &str) -> (Store, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("valkyrie-web-auth-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Store::open(&dir).unwrap(), dir)
    }

    #[test]
    fn reads_short_codes_as_people_type_them() {
        assert_eq!(short_form("KQ7-4M2").as_deref(), Some("KQ74M2"));
        assert_eq!(short_form(" kq7 4m2\n").as_deref(), Some("KQ74M2"));
        assert_eq!(short_form("1O0-IL1").as_deref(), Some("100111"));
        assert_eq!(short_form("KQ7-4M"), None);
        assert_eq!(short_form("KQ7-4MU"), None, "U isn't in the alphabet");
        assert_eq!(
            short_form("q3Vb0_x-Zc7tY2kLmN8pR4sT6uW1yA9dF5gH3jK0eB2"),
            None
        );
    }

    #[test]
    fn a_device_adds_another_with_a_short_code() {
        let (store, dir) = store("short");
        let link = store.new_code().unwrap();
        let phone = store.check(&store.pair(&link, "phone").unwrap()).unwrap();
        assert_eq!(phone.added_by, None, "paired from this machine");

        let short = store.new_short_code(Some(&phone)).unwrap();
        assert!(short.code.len() == 7 && short.code.as_bytes()[3] == b'-');
        assert_eq!(store.code_state(&short.id), CodeState::Open);
        assert_eq!(store.code_state("someone-elses-id"), CodeState::Gone);
        let typed = short.code.to_lowercase().replace('-', " ");
        let laptop = store.check(&store.pair(&typed, "laptop").unwrap()).unwrap();
        assert_eq!(laptop.added_by.as_deref(), Some("phone"));
        assert_eq!(
            store.code_state(&short.id),
            CodeState::Paired {
                device: "laptop".into()
            }
        );
        assert!(store.pair(&short.code, "again").is_err(), "single-use");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wrong_tries_end_short_codes_but_not_links() {
        let (store, dir) = store("tries");
        let link = store.new_code().unwrap();
        let short = store.new_short_code(None).unwrap();
        for _ in 0..SHORT_CODE_TRIES {
            assert!(store.pair("AAA-AAA", "guess").is_err());
        }
        assert_eq!(store.code_state(&short.id), CodeState::Gone);
        assert!(store.pair(&short.code, "late").is_err());
        assert!(
            store.pair(&link, "phone").is_ok(),
            "a link's code can't be guessed"
        );
        // A code made after the guesses starts afresh.
        let next = store.new_short_code(None).unwrap();
        assert!(store.pair(&next.code, "tablet").is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
