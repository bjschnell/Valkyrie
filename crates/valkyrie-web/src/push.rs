//! Web Push (RFC 8030): a message encrypted to one browser (RFC 8291, `aes128gcm`)
//! and signed as this server (VAPID, RFC 8292), posted to the push service the
//! browser chose (Apple's, Google's, Mozilla's). The push service sees only the
//! ciphertext.

use crate::auth::Subscription;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use sha2::Sha256;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The record size in the header; a message is one record, so it must fit.
const RECORD_SIZE: u32 = 4096;
/// The most plaintext one record holds: the record, less the tag and delimiter.
pub const MAX_PAYLOAD: usize = RECORD_SIZE as usize - 16 - 1;
/// Push services want a contact; this one names the project.
const SUBJECT: &str = "https://github.com/bjschnell/Valkyrie";

/// A new P-256 private key.
pub fn new_secret() -> Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    loop {
        getrandom::fill(&mut bytes).map_err(|e| anyhow!("random: {e}"))?;
        // Almost every 32 bytes is a valid scalar; retry the rest.
        if SecretKey::from_slice(&bytes).is_ok() {
            return Ok(bytes);
        }
    }
}

/// How soon the push service should deliver (RFC 8030 §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Normal,
    High,
}

/// What happened to one push.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    /// The browser unsubscribed or the subscription expired: forget it.
    Gone,
}

pub struct Sender {
    http: reqwest::Client,
    key: SigningKey,
    /// The public key browsers subscribe with (`applicationServerKey`).
    pub public: String,
}

impl Sender {
    pub fn new(secret: &[u8; 32]) -> Result<Self> {
        // reqwest is built without a default TLS provider; this picks ring.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let secret = SecretKey::from_slice(secret).context("VAPID key")?;
        let public = URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false));
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()?,
            key: SigningKey::from(secret),
            public,
        })
    }

    /// Delivers `payload` to one browser. `topic` replaces an undelivered push with
    /// the same topic (a phone that was offline gets the latest, not every one).
    pub async fn send(
        &self,
        sub: &Subscription,
        payload: &[u8],
        topic: &str,
        urgency: Urgency,
        ttl: Duration,
    ) -> Result<Outcome> {
        anyhow::ensure!(
            sub.endpoint.starts_with("https://"),
            "push endpoint isn't https"
        );
        let ua_public = URL_SAFE_NO_PAD.decode(sub.p256dh.trim_end_matches('='))?;
        let auth = URL_SAFE_NO_PAD.decode(sub.auth.trim_end_matches('='))?;
        let body = encrypt(&ua_public, &auth, payload)?;
        let jwt = vapid_jwt(&self.key, &sub.endpoint, now_unix())?;
        let response = self
            .http
            .post(&sub.endpoint)
            .header("TTL", ttl.as_secs().to_string())
            .header(
                "Urgency",
                match urgency {
                    Urgency::Normal => "normal",
                    Urgency::High => "high",
                },
            )
            .header("Topic", topic)
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("Authorization", format!("vapid t={jwt}, k={}", self.public))
            .body(body)
            .send()
            .await
            // The endpoint URL is the subscription's secret; keep it out of logs.
            .map_err(|e| anyhow!("push service: {}", e.without_url()))?;
        let status = response.status();
        match status.as_u16() {
            200..=299 => Ok(Outcome::Sent),
            404 | 410 => Ok(Outcome::Gone),
            _ => {
                let text = response.text().await.unwrap_or_default();
                bail!("push service said {status}: {}", text.trim())
            }
        }
    }
}

/// Encrypts one message to a browser's key and auth secret (RFC 8291).
pub fn encrypt(ua_public: &[u8], auth: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| anyhow!("random: {e}"))?;
    let ephemeral = SecretKey::from_slice(&new_secret()?)?;
    encrypt_with(&ephemeral, &salt, ua_public, auth, plaintext)
}

fn encrypt_with(
    as_secret: &SecretKey,
    salt: &[u8; 16],
    ua_public: &[u8],
    auth: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    anyhow::ensure!(plaintext.len() <= MAX_PAYLOAD, "push message too long");
    let ua_key = PublicKey::from_sec1_bytes(ua_public).context("browser push key")?;
    let as_public = as_secret.public_key().to_encoded_point(false);
    let shared = p256::ecdh::diffie_hellman(as_secret.to_nonzero_scalar(), ua_key.as_affine());

    // IKM = HKDF(auth, ecdh, "WebPush: info\0" || ua_public || as_public)
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public.as_bytes());
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .map_err(|e| anyhow!("hkdf: {e}"))?;

    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .and_then(|()| prk.expand(b"Content-Encoding: nonce\0", &mut nonce))
        .map_err(|e| anyhow!("hkdf: {e}"))?;

    // One record: the message, then 0x02 for "last record", no padding.
    let mut record = plaintext.to_vec();
    record.push(2);
    let sealed = Aes128Gcm::new(&cek.into())
        .encrypt(Nonce::from_slice(&nonce), record.as_slice())
        .map_err(|e| anyhow!("encrypt: {e}"))?;

    let mut out = Vec::with_capacity(86 + sealed.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(as_public.as_bytes());
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// A VAPID token for the push service at `endpoint`, good for 12 hours.
fn vapid_jwt(key: &SigningKey, endpoint: &str, now: u64) -> Result<String> {
    let url = reqwest::Url::parse(endpoint)?;
    let audience = url.origin().ascii_serialization();
    let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = URL_SAFE_NO_PAD.encode(
        serde_json::json!({ "aud": audience, "exp": now + 12 * 3600, "sub": SUBJECT }).to_string(),
    );
    let signed = format!("{header}.{claims}");
    let signature: Signature = key.sign(signed.as_bytes());
    Ok(format!(
        "{signed}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
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

    fn b64(text: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(text).unwrap()
    }

    /// RFC 8291 §5 and Appendix A.
    #[test]
    fn encrypts_like_the_rfc_example() {
        let as_secret =
            SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let ua_public = b64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let auth = b64("BTBZMqHH6r4Tts7J_aSIgg");
        let out = encrypt_with(
            &as_secret,
            &salt,
            &ua_public,
            &auth,
            b"When I grow up, I want to be a watermelon",
        )
        .unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(out),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml\
             mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT\
             pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn a_vapid_token_verifies_with_the_public_key() {
        use p256::ecdsa::VerifyingKey;
        use p256::ecdsa::signature::Verifier;
        let sender = Sender::new(&new_secret().unwrap()).unwrap();
        let jwt = vapid_jwt(&sender.key, "https://web.push.apple.com/abc/def", 1000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value = serde_json::from_slice(&b64(parts[1])).unwrap();
        assert_eq!(claims["aud"], "https://web.push.apple.com");
        assert_eq!(claims["exp"], 1000 + 12 * 3600);
        let public = VerifyingKey::from_sec1_bytes(&b64(&sender.public)).unwrap();
        let signature = Signature::from_slice(&b64(parts[2])).unwrap();
        public
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .unwrap();
    }

    #[test]
    fn too_long_a_message_is_refused() {
        let ua = SecretKey::from_slice(&new_secret().unwrap()).unwrap();
        let ua_public = ua.public_key().to_encoded_point(false);
        assert!(encrypt(ua_public.as_bytes(), &[0; 16], &vec![b'x'; MAX_PAYLOAD]).is_ok());
        assert!(encrypt(ua_public.as_bytes(), &[0; 16], &vec![b'x'; MAX_PAYLOAD + 1]).is_err());
    }
}
