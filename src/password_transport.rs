//! RAGFlow-compatible RSA password transport without a repository-owned key.
//!
//! The fixed RAGFlow `conf/private.pem` / `conf/public.pem` pair is used for
//! RSAES-PKCS1-v1_5 wrapping of a base64-encoded UTF-8 password. RayRAG does
//! not copy that globally known private key. Operators may instead mount a
//! deployment-specific PKCS#1 or PKCS#8 private key and set
//! `RAYRAG_PASSWORD_PRIVATE_KEY_FILE`. The public key is derived from it.

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::{DecodePrivateKey, EncodePublicKey, LineEnding};
use rsa::rand_core::OsRng;
use rsa::traits::PublicKeyParts;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use std::borrow::Cow;
use std::path::Path;

/// Fixed-source evidence. Contents are deliberately not embedded.
pub const RAGFLOW_PRIVATE_PEM_BLOB: &str = "ff333058d0d8087fadb09295c369a59f1d46a166";
pub const RAGFLOW_PRIVATE_PEM_SHA256: &str =
    "41cf8fb7e403f76f884e516392e12c2c8cc9c7b45d3ff1dc70359ee320b16ee5";
pub const RAGFLOW_PRIVATE_PEM_BYTES: usize = 1_743;
pub const RAGFLOW_PUBLIC_PEM_BLOB: &str = "3fbcfe189593c174d9893721839dcffadf7bcce8";
pub const RAGFLOW_PUBLIC_PEM_SHA256: &str =
    "6745cb2fb7215142aca36a2b0b7d4794a7a21894b991263c715fa26367f11e8d";
pub const RAGFLOW_PUBLIC_PEM_BYTES: usize = 451;

/// Optional deployment-specific password transport key.
pub struct PasswordTransport {
    private_key: RsaPrivateKey,
    public_key_pem: String,
}

impl PasswordTransport {
    /// Load the optional private key configured for the process.
    pub fn from_env() -> Result<Option<Self>> {
        let Some(path) = std::env::var_os("RAYRAG_PASSWORD_PRIVATE_KEY_FILE") else {
            return Ok(None);
        };
        if path.is_empty() {
            return Ok(None);
        }
        Self::from_private_key_file(Path::new(&path)).map(Some)
    }

    /// Load an unencrypted PKCS#1 or PKCS#8 PEM private key from a mounted
    /// secret. RayRAG never needs a separately stored public-key file.
    pub fn from_private_key_file(path: &Path) -> Result<Self> {
        let pem = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read password transport key {}", path.display()))?;
        Self::from_private_key_pem(&pem)
            .with_context(|| format!("invalid password transport key {}", path.display()))
    }

    /// Parse a private key. Kept public for offline deployment validation and
    /// focused compatibility tests; callers must not log the input.
    pub fn from_private_key_pem(pem: &str) -> Result<Self> {
        let private_key = RsaPrivateKey::from_pkcs1_pem(pem)
            .or_else(|_| RsaPrivateKey::from_pkcs8_pem(pem))
            .context("expected an unencrypted RSA PKCS#1 or PKCS#8 PEM private key")?;
        private_key
            .validate()
            .context("RSA password transport private key failed validation")?;
        if private_key.n().bits() < 2_048 {
            bail!("RSA password transport key must be at least 2048 bits");
        }
        let public_key_pem = RsaPublicKey::from(&private_key)
            .to_public_key_pem(LineEnding::LF)
            .context("failed to derive RSA password transport public key")?;
        Ok(Self {
            private_key,
            public_key_pem,
        })
    }

    /// SPKI PEM public key for RAGFlow-compatible browser/CLI clients.
    pub fn public_key_pem(&self) -> &str {
        &self.public_key_pem
    }

    /// Decode the fixed RAGFlow wire form:
    /// `base64(RSAES-PKCS1-v1_5(base64(UTF-8 password)))`.
    ///
    /// As in the fixed Go service, non-base64 input, the wrong ciphertext
    /// length, or failed RSA decryption falls back to the original plaintext.
    /// Once valid RSA padding is observed, malformed inner base64/UTF-8 is a
    /// request error instead of being silently treated as a password.
    pub fn decode<'a>(&self, value: &'a str) -> Result<Cow<'a, str>> {
        let Ok(ciphertext) = STANDARD.decode(value) else {
            return Ok(Cow::Borrowed(value));
        };
        if ciphertext.len() != self.private_key.size() {
            return Ok(Cow::Borrowed(value));
        }
        let Ok(inner_base64) =
            self.private_key
                .decrypt_blinded(&mut OsRng, Pkcs1v15Encrypt, &ciphertext)
        else {
            return Ok(Cow::Borrowed(value));
        };
        let plaintext = STANDARD
            .decode(&inner_base64)
            .context("RSA password payload is not base64-encoded UTF-8")?;
        String::from_utf8(plaintext)
            .map(Cow::Owned)
            .context("RSA password payload is not valid UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::{DecodePublicKey, EncodePrivateKey};
    use rsa::rand_core::OsRng;

    fn transport() -> PasswordTransport {
        let private_key = RsaPrivateKey::new(&mut OsRng, 2_048).unwrap();
        let pem = private_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        PasswordTransport::from_private_key_pem(pem.as_str()).unwrap()
    }

    #[test]
    fn decodes_ragflow_browser_and_cli_wire_form_and_preserves_plaintext() {
        let transport = transport();
        let public_key = RsaPublicKey::from_public_key_pem(transport.public_key_pem()).unwrap();
        let inner = STANDARD.encode("correct horse battery staple");
        let ciphertext = public_key
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, inner.as_bytes())
            .unwrap();
        let wire = STANDARD.encode(ciphertext);

        assert_eq!(
            transport.decode(&wire).unwrap(),
            "correct horse battery staple"
        );
        assert_eq!(
            transport.decode("plain password").unwrap(),
            "plain password"
        );
        assert_eq!(transport.decode("cGxhaW4=").unwrap(), "cGxhaW4=");
        assert!(!transport.public_key_pem().contains("PRIVATE"));
    }

    #[test]
    fn rejects_short_keys_and_pins_fixed_source_metadata_without_key_material() {
        let private_key = RsaPrivateKey::new(&mut OsRng, 1_024).unwrap();
        let pem = private_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        assert!(PasswordTransport::from_private_key_pem(pem.as_str()).is_err());

        assert_eq!(RAGFLOW_PRIVATE_PEM_BYTES, 1_743);
        assert_eq!(RAGFLOW_PUBLIC_PEM_BYTES, 451);
        assert_eq!(RAGFLOW_PRIVATE_PEM_BLOB.len(), 40);
        assert_eq!(RAGFLOW_PUBLIC_PEM_BLOB.len(), 40);
        assert_eq!(RAGFLOW_PRIVATE_PEM_SHA256.len(), 64);
        assert_eq!(RAGFLOW_PUBLIC_PEM_SHA256.len(), 64);
    }
}
