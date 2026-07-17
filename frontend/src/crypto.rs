use aes_gcm_siv::{
    Aes256GcmSiv, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use getrandom::getrandom;
use mew_image_shared::EncryptedSecret;
use pbkdf2::pbkdf2_hmac;
use sha2::{Digest, Sha256};

const TRUSTED_SYNC_KEY_ROUNDS: u32 = 120_000;

pub fn derive_trusted_sync_secret(user_id: &str, password: &str) -> String {
    let salt = format!("mew-image-trusted-sync-v1:{user_id}");
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(
        password.as_bytes(),
        salt.as_bytes(),
        TRUSTED_SYNC_KEY_ROUNDS,
        &mut key,
    );
    BASE64.encode(key)
}

pub fn encrypt_secret(secret: &str, plaintext: &str) -> Result<EncryptedSecret, String> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    getrandom(&mut salt).map_err(|error| error.to_string())?;
    getrandom(&mut nonce).map_err(|error| error.to_string())?;
    let key = derive_key(secret, &salt);
    let cipher = Aes256GcmSiv::new_from_slice(&key).map_err(|error| error.to_string())?;
    let encrypted = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(EncryptedSecret {
        salt_b64: BASE64.encode(salt),
        nonce_b64: BASE64.encode(nonce),
        ciphertext_b64: BASE64.encode(encrypted),
    })
}

pub fn decrypt_secret(secret: &str, encrypted: &EncryptedSecret) -> Result<String, String> {
    let salt = BASE64
        .decode(&encrypted.salt_b64)
        .map_err(|error| error.to_string())?;
    let nonce = BASE64
        .decode(&encrypted.nonce_b64)
        .map_err(|error| error.to_string())?;
    let ciphertext = BASE64
        .decode(&encrypted.ciphertext_b64)
        .map_err(|error| error.to_string())?;
    let key = derive_key(secret, &salt);
    let cipher = Aes256GcmSiv::new_from_slice(&key).map_err(|error| error.to_string())?;
    let decrypted = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|error| error.to_string())?;
    String::from_utf8(decrypted).map_err(|error| error.to_string())
}

fn derive_key(secret: &str, salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(secret.as_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest[..32]);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_sync_secret_is_stable_and_user_scoped() {
        let first = derive_trusted_sync_secret("user-1", "Strong!Password1");
        let repeated = derive_trusted_sync_secret("user-1", "Strong!Password1");
        let other_user = derive_trusted_sync_secret("user-2", "Strong!Password1");

        assert_eq!(first, repeated);
        assert_ne!(first, other_user);
    }

    #[test]
    fn trusted_sync_secret_can_encrypt_and_decrypt_api_key() {
        let secret = derive_trusted_sync_secret("user-1", "Strong!Password1");
        let encrypted = encrypt_secret(&secret, "sk-example").unwrap();
        assert_eq!(decrypt_secret(&secret, &encrypted).unwrap(), "sk-example");
    }
}
