//! Opaque secrets (cookies, OAuth codes and tokens) are random, URL-safe
//! strings; only their SHA-256 digest is ever stored.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// A fresh 256-bit secret, base64url-encoded.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Digest used as the database key for a secret.
pub fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// AES-256-GCM for small secrets kept in the database: `nonce || ciphertext`,
/// with `aad` (e.g. the row's name) bound so a value cannot be moved.
pub mod sealed {
    use aes_gcm::{
        Aes256Gcm, Key, Nonce,
        aead::{Aead, KeyInit, Payload},
    };

    const NONCE_LEN: usize = 12;

    pub fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let nonce: [u8; NONCE_LEN] = rand::random();
        let ciphertext = cipher(key)
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("encryption failed"))?;
        Ok([nonce.as_slice(), &ciphertext].concat())
    }

    pub fn open(key: &[u8; 32], aad: &[u8], sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
        let (nonce, ciphertext) = sealed
            .split_at_checked(NONCE_LEN)
            .ok_or_else(|| anyhow::anyhow!("sealed value is too short"))?;
        cipher(key)
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| {
                anyhow::anyhow!("it was encrypted with a different ENCRYPTION_KEY or is corrupted")
            })
    }

    fn cipher(key: &[u8; 32]) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
    }
}
