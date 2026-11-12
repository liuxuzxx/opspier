//! Backend-only credential material. Public configuration contains only owned references.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};
use uuid::Uuid;

pub const MAX_SECRET_BYTES: usize = 2 * 1024 * 1024;
const SERVICE: &str = "com.aic.ops.credentials.v1";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretOwner {
    pub profile_id: String,
    pub target: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretRef {
    pub owner: SecretOwner,
    pub id: String,
}
impl SecretRef {
    pub fn new(owner: SecretOwner) -> Result<Self, CredentialError> {
        let value = Self {
            owner,
            id: Uuid::new_v4().to_string(),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), CredentialError> {
        if Uuid::parse_str(&self.id).is_err()
            || Uuid::parse_str(&self.owner.profile_id).is_err()
            || self.owner.target.is_empty()
            || self.owner.target.len() > 4096
        {
            return Err(CredentialError::InvalidReference);
        }
        Ok(())
    }
    fn key(&self) -> String {
        format!("{}.{}", self.owner.profile_id, self.id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialError {
    Unavailable,
    Missing,
    InvalidReference,
    Corrupt,
    TooLarge,
    Unsupported,
}
impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "系统凭据库不可用、已锁定或拒绝访问，请解锁后重试；未降级保存",
            Self::Missing => "已保存凭据不存在，请重新输入",
            Self::InvalidReference => "凭据引用与连接目标不匹配，请重新输入",
            Self::Corrupt => "已保存凭据不可读取，请重新保存",
            Self::TooLarge => "凭据或配置超过2MiB上限",
            Self::Unsupported => "当前平台未配置系统凭据库，不能持久保存",
        })
    }
}
impl std::error::Error for CredentialError {}

pub trait SecretBackend: Send + Sync {
    fn put(&self, key: &str, value: &[u8]) -> Result<(), CredentialError>;
    fn get(&self, key: &str) -> Result<Vec<u8>, CredentialError>;
    fn delete(&self, key: &str) -> Result<(), CredentialError>;
}
struct SecretServiceBackend;
impl SecretServiceBackend {
    #[cfg(target_os = "linux")]
    fn entry(&self, key: &str) -> Result<keyring::Entry, CredentialError> {
        // A concrete provider avoids the crate's unsupported-platform mock default.
        let credential = keyring::secret_service::SsCredential::new_with_target(None, SERVICE, key)
            .map_err(keyring_error)?;
        Ok(keyring::Entry::new_with_credential(Box::new(credential)))
    }
    #[cfg(not(target_os = "linux"))]
    fn entry(&self, _key: &str) -> Result<keyring::Entry, CredentialError> {
        Err(CredentialError::Unsupported)
    }
}
fn keyring_error(error: keyring::Error) -> CredentialError {
    match error {
        keyring::Error::NoEntry => CredentialError::Missing,
        _ => CredentialError::Unavailable,
    }
}
impl SecretBackend for SecretServiceBackend {
    fn put(&self, key: &str, value: &[u8]) -> Result<(), CredentialError> {
        self.entry(key)?.set_secret(value).map_err(keyring_error)
    }
    fn get(&self, key: &str) -> Result<Vec<u8>, CredentialError> {
        self.entry(key)?.get_secret().map_err(keyring_error)
    }
    fn delete(&self, key: &str) -> Result<(), CredentialError> {
        self.entry(key)?.delete_credential().map_err(keyring_error)
    }
}

// This envelope is serialized only into the OS credential service, never into profiles or IPC.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    owner: SecretOwner,
    material: String,
}
pub struct CredentialStore {
    backend: Arc<dyn SecretBackend>,
}
impl CredentialStore {
    pub fn system() -> Self {
        Self {
            backend: Arc::new(SecretServiceBackend),
        }
    }
    #[cfg(test)]
    pub fn with_backend(backend: Arc<dyn SecretBackend>) -> Self {
        Self { backend }
    }
    fn check(owner: &SecretOwner, reference: &SecretRef) -> Result<(), CredentialError> {
        reference.validate()?;
        if &reference.owner != owner {
            return Err(CredentialError::InvalidReference);
        }
        Ok(())
    }
    pub fn put(
        &self,
        owner: &SecretOwner,
        reference: &SecretRef,
        material: &[u8],
    ) -> Result<(), CredentialError> {
        Self::check(owner, reference)?;
        if material.len() > MAX_SECRET_BYTES {
            return Err(CredentialError::TooLarge);
        }
        let bytes = serde_json::to_vec(&Envelope {
            owner: owner.clone(),
            material: STANDARD.encode(material),
        })
        .map_err(|_| CredentialError::Corrupt)?;
        self.backend.put(&reference.key(), &bytes)?;
        if self.get(owner, reference)? != material {
            return Err(CredentialError::Corrupt);
        }
        Ok(())
    }
    pub fn get(
        &self,
        owner: &SecretOwner,
        reference: &SecretRef,
    ) -> Result<Vec<u8>, CredentialError> {
        Self::check(owner, reference)?;
        let bytes = self.backend.get(&reference.key())?;
        if bytes.len() > MAX_SECRET_BYTES * 2 + 8192 {
            return Err(CredentialError::TooLarge);
        }
        let envelope: Envelope =
            serde_json::from_slice(&bytes).map_err(|_| CredentialError::Corrupt)?;
        if &envelope.owner != owner {
            return Err(CredentialError::InvalidReference);
        }
        let material = STANDARD
            .decode(envelope.material)
            .map_err(|_| CredentialError::Corrupt)?;
        if material.len() > MAX_SECRET_BYTES {
            return Err(CredentialError::TooLarge);
        }
        Ok(material)
    }
    pub fn delete(
        &self,
        owner: &SecretOwner,
        reference: &SecretRef,
    ) -> Result<(), CredentialError> {
        // Missing planned entries are idempotent cleanup; never delete an unverified owner.
        match self.get(owner, reference) {
            Ok(_) => {}
            Err(CredentialError::Missing) => return Ok(()),
            Err(error) => return Err(error),
        }
        match self.backend.delete(&reference.key()) {
            Err(CredentialError::Missing) => Ok(()),
            result => result,
        }
    }
}
