//! Where a platform sign-in is kept: encrypted with Windows' data protection for this Windows
//! user (DPAPI), in the app's data folder. Only this user on this PC can read it back; nothing of
//! it is ever logged.

use anyhow::{anyhow, Result};
use std::path::PathBuf;

fn dir() -> PathBuf {
    let d = crate::paths::data_dir().join("accounts");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn file(name: &str) -> PathBuf {
    dir().join(format!("{name}.bin"))
}

/// Encrypt for this Windows user.
pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(|e| anyhow!("Could not protect the sign-in: {e}"))?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
        Ok(v)
    }
}

/// Decrypt what `protect` made for this Windows user.
pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: sealed.len() as u32, pbData: sealed.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(|e| anyhow!("Could not read the sign-in: {e}"))?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
        Ok(v)
    }
}

/// Keep `value` (JSON) under `name`, encrypted.
pub fn save<T: serde::Serialize>(name: &str, value: &T) -> Result<()> {
    let sealed = protect(&serde_json::to_vec(value)?)?;
    let f = file(name);
    let tmp = f.with_extension("bin.part");
    std::fs::write(&tmp, sealed)?;
    std::fs::rename(&tmp, &f)?;
    Ok(())
}

/// What `save` kept under `name`, if anything (and if it can still be read).
pub fn load<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    let sealed = std::fs::read(file(name)).ok()?;
    serde_json::from_slice(&unprotect(&sealed).ok()?).ok()
}

pub fn has(name: &str) -> bool {
    file(name).is_file()
}

/// Forget what is kept under `name`.
pub fn remove(name: &str) {
    let _ = std::fs::remove_file(file(name));
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_sign_in_is_kept_encrypted_and_read_back() {
        let secret = b"refresh-token-made-up-for-the-test";
        let sealed = super::protect(secret).unwrap();
        // Not stored in the clear.
        assert!(!sealed.windows(secret.len()).any(|w| w == secret));
        assert_eq!(super::unprotect(&sealed).unwrap(), secret);
        assert!(super::unprotect(b"not sealed").is_err());
    }
}
