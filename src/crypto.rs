//! Sealing secrets at rest with AES-256-GCM (`PEEK_ENCRYPTION_KEY`).
//!
//! Used for org BYO Deepgram keys and for testing-environment root keys. The
//! sealed form is `0x01 || nonce(12) || ciphertext+tag`. The associated data
//! names what the secret is for (for example `peek/byo/v1/<ctx>/<org>/deepgram`),
//! so a sealed value copied into another row fails to open.

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};

use crate::error::{ApiError, ApiResult};

const FORMAT_V1: u8 = 1;
const NONCE_LEN: usize = 12;

/// Seals and opens secrets with the server's key.
#[derive(Clone)]
pub(crate) struct Sealer {
    cipher: Aes256Gcm,
}

impl Sealer {
    /// A sealer for a 32-byte key.
    pub(crate) fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)),
        }
    }

    /// Encrypts `plaintext`, binding it to `aad`.
    pub(crate) fn seal(&self, aad: &str, plaintext: &[u8]) -> ApiResult<Vec<u8>> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| ApiError::internal("sealing a secret failed"))?;
        let mut out = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
        out.push(FORMAT_V1);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Decrypts a value sealed for `aad`.
    pub(crate) fn open(&self, aad: &str, sealed: &[u8]) -> ApiResult<Vec<u8>> {
        if sealed.len() <= 1 + NONCE_LEN || sealed[0] != FORMAT_V1 {
            return Err(ApiError::internal(
                "a sealed secret in the database has an unknown format",
            ));
        }
        let nonce = Nonce::from_slice(&sealed[1..=NONCE_LEN]);
        self.cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &sealed[1 + NONCE_LEN..],
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| {
                ApiError::internal(
                    "a sealed secret could not be opened; PEEK_ENCRYPTION_KEY may have changed since it was stored",
                )
            })
    }

    /// Opens a sealed UTF-8 secret.
    pub(crate) fn open_string(&self, aad: &str, sealed: &[u8]) -> ApiResult<String> {
        String::from_utf8(self.open(aad, sealed)?)
            .map_err(|_| ApiError::internal("a sealed secret is not UTF-8"))
    }
}

/// AAD for an org's BYO key.
pub(crate) fn byo_aad(ctx: &str, org: &str) -> String {
    format!("peek/byo/v1/{ctx}/{org}/deepgram")
}

/// AAD for a testing environment's root key.
pub(crate) fn root_key_aad(environment_id: &str) -> String {
    format!("peek/env-root/v1/{environment_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_binding() -> ApiResult<()> {
        let cipher = Sealer::new(&[7; 32]);
        let boxed = cipher.seal("a", b"dg-key")?;
        assert_ne!(&boxed[13..], b"dg-key");
        assert_eq!(cipher.open("a", &boxed)?, b"dg-key");
        assert!(cipher.open("b", &boxed).is_err(), "aad binds the purpose");
        let other = Sealer::new(&[8; 32]);
        assert!(other.open("a", &boxed).is_err(), "wrong key fails");
        assert!(cipher.open("a", &boxed[..5]).is_err());
        let again = cipher.seal("a", b"dg-key")?;
        assert_ne!(boxed, again, "fresh nonce per seal");
        Ok(())
    }
}
