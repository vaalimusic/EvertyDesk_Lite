const REMOTE_SERVICE_PREFIX: &str = "EvertyDesk/remote/";
const ACCOUNT_SERVICE_PREFIX: &str = "EvertyDesk/account/";
const LOCAL_PERMANENT_PASSWORD_TARGET: &str = "EvertyDesk/local/permanent-password";
const MAX_PASSWORD_BYTES: usize = 512;
const MAX_ACCOUNT_TOKEN_BYTES: usize = 8 * 1024;

pub fn load_password(remote_id: &str) -> Result<Option<String>, String> {
    platform::load(&remote_target_name(remote_id))
}

pub fn store_password(remote_id: &str, password: &str) -> Result<(), String> {
    if password.len() > MAX_PASSWORD_BYTES {
        return Err("пароль слишком длинный для системного хранилища".to_owned());
    }
    platform::store(&remote_target_name(remote_id), password)
}

pub fn delete_password(remote_id: &str) -> Result<(), String> {
    platform::delete(&remote_target_name(remote_id))
}

pub fn load_permanent_password() -> Result<Option<String>, String> {
    platform::load(LOCAL_PERMANENT_PASSWORD_TARGET)
}

pub fn store_permanent_password(password: &str) -> Result<(), String> {
    if password.len() > MAX_PASSWORD_BYTES {
        return Err("постоянный пароль слишком длинный для системного хранилища".to_owned());
    }
    platform::store(LOCAL_PERMANENT_PASSWORD_TARGET, password)
}

pub fn delete_permanent_password() -> Result<(), String> {
    platform::delete(LOCAL_PERMANENT_PASSWORD_TARGET)
}

pub fn load_account_token(account: &str) -> Result<Option<String>, String> {
    validate_account(account)?;
    platform::load(&account_target_name(account))
}

pub fn store_account_token(account: &str, token: &str) -> Result<(), String> {
    validate_account(account)?;
    if token.len() > MAX_ACCOUNT_TOKEN_BYTES {
        return Err("токен аккаунта слишком длинный для системного хранилища".to_owned());
    }
    platform::store(&account_target_name(account), token)
}

pub fn delete_account_token(account: &str) -> Result<(), String> {
    validate_account(account)?;
    platform::delete(&account_target_name(account))
}

fn validate_account(account: &str) -> Result<(), String> {
    if account.trim().is_empty() {
        Err("не указан аккаунт адресной книги".to_owned())
    } else {
        Ok(())
    }
}

fn remote_target_name(remote_id: &str) -> String {
    format!("{REMOTE_SERVICE_PREFIX}{}", remote_id.trim())
}

fn account_target_name(account: &str) -> String {
    let normalized: String = account
        .trim()
        .to_lowercase()
        .chars()
        .take(200)
        .map(|character| {
            if character.is_ascii_alphanumeric() || "@._-".contains(character) {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!("{ACCOUNT_SERVICE_PREFIX}{normalized}")
}

#[cfg(windows)]
mod platform {
    use std::ptr;
    use std::slice;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };
    use zeroize::Zeroize;

    struct CredentialBuffer(*mut CREDENTIALW);

    impl Drop for CredentialBuffer {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CredFree(self.0.cast()) };
            }
        }
    }

    pub fn load(target: &str) -> Result<Option<String>, String> {
        let target = wide(target);
        let mut raw = ptr::null_mut();
        if let Err(error) =
            unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut raw) }
        {
            return if error.code() == ERROR_NOT_FOUND.to_hresult() {
                Ok(None)
            } else {
                Err(format!("Windows Credential Manager: {error}"))
            };
        }
        let credential = CredentialBuffer(raw);
        let value = unsafe { credential.0.as_ref() }
            .ok_or_else(|| "Windows Credential Manager вернул пустую запись".to_owned())?;
        let blob = unsafe {
            slice::from_raw_parts(value.CredentialBlob, value.CredentialBlobSize as usize)
        };
        String::from_utf8(blob.to_vec())
            .map(Some)
            .map_err(|_| "сохранённый пароль имеет неверную кодировку".to_owned())
    }

    pub fn store(target: &str, password: &str) -> Result<(), String> {
        let mut target = wide(target);
        let mut username = wide("EvertyDesk");
        let mut blob = password.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: u32::try_from(blob.len())
                .map_err(|_| "пароль слишком длинный".to_owned())?,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(username.as_mut_ptr()),
            ..Default::default()
        };
        let result = unsafe { CredWriteW(&credential, 0) }
            .map_err(|error| format!("Windows Credential Manager: {error}"));
        blob.zeroize();
        result
    }

    pub fn delete(target: &str) -> Result<(), String> {
        let target = wide(target);
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(error) if error.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
            Err(error) => Err(format!("Windows Credential Manager: {error}")),
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    const SERVICE: &str = "EvertyDesk Desktop";

    pub fn load(target: &str) -> Result<Option<String>, String> {
        let entry = entry(target)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("системное хранилище учётных данных: {error}")),
        }
    }

    pub fn store(target: &str, password: &str) -> Result<(), String> {
        entry(target)?
            .set_password(password)
            .map_err(|error| format!("системное хранилище учётных данных: {error}"))
    }

    pub fn delete(target: &str) -> Result<(), String> {
        match entry(target)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(format!("системное хранилище учётных данных: {error}")),
        }
    }

    fn entry(target: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, target)
            .map_err(|error| format!("системное хранилище учётных данных: {error}"))
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    pub fn load(_target: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    pub fn store(_target: &str, _password: &str) -> Result<(), String> {
        Err("защищённое хранилище поддерживается на Windows, Linux и macOS".to_owned())
    }

    pub fn delete(_target: &str) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_target_is_namespaced_and_trimmed() {
        assert_eq!(remote_target_name(" 123456 "), "EvertyDesk/remote/123456");
    }

    #[test]
    fn account_target_is_separate_and_normalized() {
        assert_eq!(
            account_target_name(" User Name@Example.COM "),
            "EvertyDesk/account/user_name@example.com"
        );
    }

    #[test]
    fn local_permanent_password_uses_dedicated_target() {
        assert_ne!(
            LOCAL_PERMANENT_PASSWORD_TARGET,
            remote_target_name("permanent-password")
        );
        assert!(LOCAL_PERMANENT_PASSWORD_TARGET.starts_with("EvertyDesk/local/"));
    }

    #[test]
    fn oversized_password_is_rejected_before_platform_access() {
        let password = "x".repeat(MAX_PASSWORD_BYTES + 1);
        assert!(store_password("123", &password).is_err());
        assert!(store_permanent_password(&password).is_err());
    }
}
