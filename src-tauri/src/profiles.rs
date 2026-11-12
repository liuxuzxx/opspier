//! Public profile metadata and exact, durable OS-credential cleanup intents.
use crate::credentials::{CredentialStore, SecretOwner, SecretRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

const MAX_FILE: usize = 1024 * 1024;
const MAX_PROFILES: usize = 100;
const MAX_PENDING: usize = 500;
const IO_ERROR: &str = "无法原子保存连接配置，原有效记录未替换";
const DURABILITY_UNKNOWN: &str =
    "连接配置已提交，但磁盘持久性尚未确认；请刷新并重试清理，勿重复新建";
const STALE: &str = "连接配置已修改或删除，请刷新后重试";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub database: String,
    pub tls: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub authentication: String,
    pub private_key_path: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KubeSettings {
    pub context: Option<String>,
    pub namespace: Option<String>,
    pub contexts: Vec<String>,
    pub snapshot_id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct KubeMaterial {
    pub snapshot_id: String,
    pub kubeconfig: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "driver", rename_all = "lowercase")]
pub enum ProfileSettings {
    Ssh(SshSettings),
    Mysql(DatabaseSettings),
    Doris(DatabaseSettings),
    Kubernetes(KubeSettings),
    Minio(crate::minio_service::MinioSettings),
}
impl ProfileSettings {
    pub fn driver(&self) -> &'static str {
        match self {
            Self::Ssh(_) => "ssh",
            Self::Mysql(_) => "mysql",
            Self::Doris(_) => "doris",
            Self::Kubernetes(_) => "kubernetes",
            Self::Minio(_) => "minio",
        }
    }
    pub fn target(&self) -> String {
        // Ordered tuples make this stable even if serde_json's map-order feature changes.
        let public_target = match self {
            Self::Ssh(s) => serde_json::to_string(&(
                "ssh",
                &s.host,
                s.port,
                &s.username,
                &s.authentication,
                &s.private_key_path,
            ))
            .unwrap_or_default(),
            Self::Mysql(s) | Self::Doris(s) => {
                serde_json::to_string(&(self.driver(), &s.host, s.port, &s.username, &s.tls))
                    .unwrap_or_default()
            }
            Self::Kubernetes(s) => format!("kubernetes:{}", s.snapshot_id),
            Self::Minio(s) => {
                serde_json::to_string(&("minio", &s.endpoint, &s.region)).unwrap_or_default()
            }
        };
        digest(public_target.as_bytes())
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        let valid = match self {
            Self::Ssh(s) => {
                host_valid(&s.host)
                    && user_valid(&s.username)
                    && s.port > 0
                    && match s.authentication.as_str() {
                        "password" => s.private_key_path.is_none(),
                        "key" => s.private_key_path.as_deref().is_some_and(|p| {
                            p.len() <= 4096
                                && !p.chars().any(char::is_control)
                                && Path::new(p).is_absolute()
                        }),
                        _ => false,
                    }
            }
            Self::Mysql(s) | Self::Doris(s) => {
                host_valid(&s.host)
                    && user_valid(&s.username)
                    && s.port > 0
                    && s.database.len() <= 128
                    && !s.database.chars().any(char::is_control)
                    && ["auto", "verifyIdentity", "required", "disabled"].contains(&s.tls.as_str())
            }
            Self::Minio(s) => s.validate().is_ok(),
            Self::Kubernetes(s) => {
                !s.contexts.is_empty()
                    && s.contexts.len() <= 200
                    && s.contexts.iter().all(|s| text_valid(s, 256))
                    && s.context.as_ref().is_none_or(|c| s.contexts.contains(c))
                    && s.namespace.as_ref().is_none_or(|n| namespace_valid(n))
                    && Uuid::parse_str(&s.snapshot_id).is_ok()
            }
        };
        if valid {
            Ok(())
        } else {
            Err("连接配置字段无效或超出上限".into())
        }
    }
}
fn text_valid(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn host_valid(value: &str) -> bool {
    let raw = value
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(value);
    text_valid(value, 253)
        && (raw.parse::<std::net::IpAddr>().is_ok()
            || raw
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)))
}
fn user_valid(value: &str) -> bool {
    text_valid(value, 128)
}
fn namespace_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileDraft {
    pub id: Option<String>,
    pub expected_revision: Option<u64>,
    pub name: String,
    pub settings: ProfileSettings,
}
// Deliberately not Deserialize/Serialize/Debug: IPC adapters construct this explicitly.
pub enum CredentialChange {
    Keep,
    Replace(Vec<u8>),
    Remove,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredProfile {
    id: String,
    name: String,
    settings: ProfileSettings,
    revision: u64,
    secret_ref: Option<SecretRef>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub id: String,
    pub name: String,
    pub settings: ProfileSettings,
    pub revision: u64,
    pub credential_saved: bool,
}
impl From<&StoredProfile> for ProfileDto {
    fn from(p: &StoredProfile) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            settings: p.settings.clone(),
            revision: p.revision,
            credential_saved: p.secret_ref.is_some(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileListing {
    pub profiles: Vec<ProfileDto>,
    pub cleanup_pending: usize,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationReceipt {
    pub durability_confirmed: bool,
    pub profile: Option<ProfileDto>,
    pub cleanup_pending: usize,
}
pub struct ResolvedProfile {
    pub profile: ProfileDto,
    pub secret: Option<Vec<u8>>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyProfile {
    pub source_id: String,
    pub name: String,
    pub settings: ProfileSettings,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoreData {
    version: u32,
    profiles: Vec<StoredProfile>,
    pending_cleanup: Vec<SecretRef>,
    legacy_sources: HashSet<String>,
}
impl Default for StoreData {
    fn default() -> Self {
        Self {
            version: 1,
            profiles: vec![],
            pending_cleanup: vec![],
            legacy_sources: HashSet::new(),
        }
    }
}
impl StoreData {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.profiles.len() > MAX_PROFILES
            || self.pending_cleanup.len() > MAX_PENDING
            || self.legacy_sources.len() > 500
        {
            return Err("不支持的连接配置版本或数量超限".into());
        }
        let mut ids = HashSet::new();
        for p in &self.profiles {
            if Uuid::parse_str(&p.id).is_err()
                || !ids.insert(&p.id)
                || !text_valid(&p.name, 80)
                || p.revision == 0
            {
                return Err("保存连接的身份或字段无效".into());
            }
            p.settings.validate()?;
            if let Some(reference) = &p.secret_ref {
                reference.validate().map_err(|e| e.to_string())?;
                if reference.owner
                    != (SecretOwner {
                        profile_id: p.id.clone(),
                        target: p.settings.target(),
                    })
                {
                    return Err("凭据归属与连接目标不符".into());
                }
            }
        }
        for reference in &self.pending_cleanup {
            reference.validate().map_err(|e| e.to_string())?;
            if self
                .profiles
                .iter()
                .filter_map(|p| p.secret_ref.as_ref())
                .any(|r| same_key(r, reference))
            {
                return Err("待清理引用指向正在使用的凭据".into());
            }
        }
        Ok(())
    }
}
fn same_key(a: &SecretRef, b: &SecretRef) -> bool {
    a.owner.profile_id == b.owner.profile_id && a.id == b.id
}
fn add_pending(data: &mut StoreData, reference: SecretRef) {
    if !data.pending_cleanup.iter().any(|r| same_key(r, &reference)) {
        data.pending_cleanup.push(reference);
    }
}

pub struct ProfileStore {
    path: PathBuf,
    data: Mutex<StoreData>,
    credentials: Arc<CredentialStore>,
}
impl ProfileStore {
    pub fn open(path: PathBuf, credentials: Arc<CredentialStore>) -> Result<Self, String> {
        let data = match fs::File::open(&path) {
            Ok(file) => {
                if !file.metadata().map_err(|_| "无法读取连接配置")?.is_file() {
                    return Err("连接配置不是普通文件".into());
                }
                let mut bytes = Vec::new();
                file.take((MAX_FILE + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "无法读取连接配置")?;
                if bytes.len() > MAX_FILE {
                    return Err("连接配置超过1MiB上限".into());
                }
                serde_json::from_slice::<StoreData>(&bytes)
                    .map_err(|_| "连接配置格式无效，未覆盖原文件")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoreData::default(),
            Err(_) => return Err("无法读取连接配置".into()),
        };
        data.validate()?;
        Ok(Self {
            path,
            data: Mutex::new(data),
            credentials,
        })
    }
    fn commit(&self, current: &mut StoreData, next: StoreData) -> Result<bool, String> {
        self.commit_with_sync(current, next, sync_parent)
    }
    fn commit_with_sync(
        &self,
        current: &mut StoreData,
        next: StoreData,
        sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<bool, String> {
        next.validate()?;
        let bytes = serde_json::to_vec(&next).map_err(|_| IO_ERROR)?;
        if bytes.len() > MAX_FILE {
            return Err("连接配置超过1MiB上限".into());
        }
        let parent = self.path.parent().ok_or(IO_ERROR)?;
        fs::create_dir_all(parent).map_err(|_| IO_ERROR)?;
        let temporary = parent.join(format!(".ops-profiles-{}.tmp", Uuid::new_v4()));
        let written = (|| -> std::io::Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        })();
        if written.is_err() {
            let _ = fs::remove_file(temporary);
            return Err(IO_ERROR.into());
        }
        *current = next;
        // Rename is the commit point. A later fsync error cannot mean the old file survived.
        Ok(sync(parent).is_ok())
    }
    pub fn list(&self) -> Result<ProfileListing, String> {
        let data = self.data.lock().map_err(|_| "连接状态不可用")?;
        Ok(ProfileListing {
            profiles: data.profiles.iter().map(ProfileDto::from).collect(),
            cleanup_pending: data.pending_cleanup.len(),
        })
    }
    pub fn snapshot(&self, id: &str, revision: u64) -> Result<ProfileDto, String> {
        let data = self.data.lock().map_err(|_| "连接状态不可用")?;
        data.profiles
            .iter()
            .find(|p| p.id == id && p.revision == revision)
            .map(ProfileDto::from)
            .ok_or_else(|| STALE.into())
    }
    pub fn resolve(&self, id: &str, revision: u64) -> Result<ResolvedProfile, String> {
        let (profile, reference) = {
            let data = self.data.lock().map_err(|_| "连接状态不可用")?;
            let p = data
                .profiles
                .iter()
                .find(|p| p.id == id && p.revision == revision)
                .ok_or(STALE)?;
            (ProfileDto::from(p), p.secret_ref.clone())
        };
        let secret = reference
            .as_ref()
            .map(|r| {
                self.credentials
                    .get(
                        &SecretOwner {
                            profile_id: profile.id.clone(),
                            target: profile.settings.target(),
                        },
                        r,
                    )
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        Ok(ResolvedProfile { profile, secret })
    }
    pub fn save(
        &self,
        draft: ProfileDraft,
        change: CredentialChange,
    ) -> Result<MutationReceipt, String> {
        if !text_valid(&draft.name, 80) {
            return Err("连接显示名不能为空且最多80字符".into());
        }
        draft.settings.validate()?;
        let mut data = self.data.lock().map_err(|_| "连接状态不可用")?;
        let old = if let Some(id) = &draft.id {
            Some(
                data.profiles
                    .iter()
                    .find(|p| &p.id == id && Some(p.revision) == draft.expected_revision)
                    .cloned()
                    .ok_or(STALE)?,
            )
        } else {
            if draft.expected_revision.is_some() || data.profiles.len() >= MAX_PROFILES {
                return Err("最多保存100个连接，或新建请求带有无效版本".into());
            }
            None
        };
        let id = old
            .as_ref()
            .map(|p| p.id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let owner = SecretOwner {
            profile_id: id.clone(),
            target: draft.settings.target(),
        };
        let old_ref = old.as_ref().and_then(|p| p.secret_ref.clone());
        let reference = match change {
            CredentialChange::Keep => {
                if old_ref.as_ref().is_some_and(|r| r.owner != owner) {
                    return Err("认证目标已变化，请重新输入凭据".into());
                }
                if let (
                    Some(StoredProfile {
                        settings: ProfileSettings::Kubernetes(before),
                        ..
                    }),
                    ProfileSettings::Kubernetes(after),
                ) = (&old, &draft.settings)
                {
                    if before.snapshot_id != after.snapshot_id || before.contexts != after.contexts
                    {
                        return Err("替换集群配置必须重新导入".into());
                    }
                }
                old_ref.clone()
            }
            CredentialChange::Remove => None,
            CredentialChange::Replace(material) => {
                if matches!(&draft.settings, ProfileSettings::Minio(_)) {
                    crate::minio_service::MinioMaterial::decode(&material)?;
                }
                if let ProfileSettings::Kubernetes(s) = &draft.settings {
                    if serde_json::from_slice::<KubeMaterial>(&material)
                        .map(|value| value.snapshot_id != s.snapshot_id)
                        .unwrap_or(true)
                    {
                        return Err("集群配置快照不匹配，请重新导入".into());
                    }
                }
                if !matches!(&draft.settings, ProfileSettings::Kubernetes(_))
                    && material.len() > 64 * 1024
                {
                    return Err("密码或口令超过64KiB上限".into());
                }
                let reference = SecretRef::new(owner.clone()).map_err(|e| e.to_string())?;
                // Durable intent exists before any new secret can outlive a failed metadata commit.
                let mut intent = data.clone();
                add_pending(&mut intent, reference.clone());
                if !self.commit(&mut data, intent)? {
                    return Err("尚未写入凭据：清理登记的磁盘持久性未确认，请刷新后重试".into());
                }
                if let Err(error) = self.credentials.put(&owner, &reference, &material) {
                    self.cleanup_locked(&mut data);
                    return Err(error.to_string());
                }
                Some(reference)
            }
        };
        let revision = old
            .as_ref()
            .map(|p| p.revision)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(STALE)?;
        let profile = StoredProfile {
            id: id.clone(),
            name: draft.name,
            settings: draft.settings,
            revision,
            secret_ref: reference.clone(),
        };
        let mut next = data.clone();
        next.profiles.retain(|p| p.id != id);
        next.profiles.push(profile.clone());
        if let Some(r) = &reference {
            next.pending_cleanup.retain(|pending| !same_key(pending, r));
        }
        if old_ref != reference {
            if let Some(r) = old_ref {
                add_pending(&mut next, r);
            }
        }
        let durability_confirmed = self.commit(&mut data, next)?;
        if durability_confirmed {
            self.cleanup_locked(&mut data);
        }
        Ok(MutationReceipt {
            durability_confirmed,
            profile: Some(ProfileDto::from(&profile)),
            cleanup_pending: data.pending_cleanup.len(),
        })
    }
    pub fn remove(&self, id: &str, revision: u64) -> Result<MutationReceipt, String> {
        let mut data = self.data.lock().map_err(|_| "连接状态不可用")?;
        let old = data
            .profiles
            .iter()
            .find(|p| p.id == id && p.revision == revision)
            .cloned()
            .ok_or(STALE)?;
        let mut next = data.clone();
        next.profiles.retain(|p| p.id != id);
        if let Some(r) = old.secret_ref {
            add_pending(&mut next, r);
        }
        let durability_confirmed = self.commit(&mut data, next)?;
        if durability_confirmed {
            self.cleanup_locked(&mut data);
        }
        Ok(MutationReceipt {
            durability_confirmed,
            profile: None,
            cleanup_pending: data.pending_cleanup.len(),
        })
    }
    fn cleanup_locked(&self, data: &mut StoreData) {
        self.cleanup_with_sync(data, sync_parent);
    }
    fn cleanup_with_sync(
        &self,
        data: &mut StoreData,
        sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) {
        // A failed sync must retain every old secret, even after a committed metadata change.
        if data.pending_cleanup.is_empty()
            || fs::File::open(&self.path)
                .and_then(|f| f.sync_all())
                .and_then(|_| {
                    sync(
                        self.path
                            .parent()
                            .ok_or_else(|| std::io::Error::other("parent"))?,
                    )
                })
                .is_err()
        {
            return;
        }
        let mut next = data.clone();
        next.pending_cleanup.retain(|reference| {
            if data
                .profiles
                .iter()
                .filter_map(|p| p.secret_ref.as_ref())
                .any(|r| same_key(r, reference))
            {
                return true;
            }
            self.credentials
                .delete(&reference.owner, reference)
                .is_err()
        });
        if next.pending_cleanup.len() != data.pending_cleanup.len() {
            let _ = self.commit(data, next);
        }
    }
    pub fn retry_cleanup(&self) -> Result<usize, String> {
        let mut data = self.data.lock().map_err(|_| "连接状态不可用")?;
        self.cleanup_locked(&mut data);
        Ok(data.pending_cleanup.len())
    }
    pub fn import_legacy(&self, profiles: Vec<LegacyProfile>) -> Result<usize, String> {
        if profiles.len() > MAX_PROFILES {
            return Err("迁移数量超过上限".into());
        }
        let mut data = self.data.lock().map_err(|_| "连接状态不可用")?;
        let mut next = data.clone();
        let mut added = 0;
        for profile in profiles {
            if !text_valid(&profile.name, 80)
                || !text_valid(&profile.source_id, 256)
                || matches!(
                    &profile.settings,
                    ProfileSettings::Kubernetes(_) | ProfileSettings::Minio(_)
                )
            {
                return Err("旧连接字段无效，原记录保留".into());
            }
            profile.settings.validate()?;
            let marker = digest(&serde_json::to_vec(&profile).map_err(|_| "旧连接无法迁移")?);
            if next.legacy_sources.insert(marker) {
                next.profiles.push(StoredProfile {
                    id: Uuid::new_v4().to_string(),
                    name: profile.name,
                    settings: profile.settings,
                    revision: 1,
                    secret_ref: None,
                });
                added += 1;
            }
        }
        if added == 0 {
            return Ok(0);
        }
        if !self.commit(&mut data, next)? {
            return Err(DURABILITY_UNKNOWN.into());
        }
        Ok(added)
    }
}

fn sync_parent(parent: &Path) -> std::io::Result<()> {
    fs::File::open(parent)?.sync_all()
}

#[cfg(test)]
mod durability_tests {
    use super::*;
    #[test]
    fn directory_sync_failure_is_already_committed_and_never_reported_as_old_file_intact() {
        let path = std::env::temp_dir().join(format!("ops-durability-{}", Uuid::new_v4()));
        let store = ProfileStore::open(
            path.join("profiles.json"),
            Arc::new(CredentialStore::with_backend(Arc::new(
                crate::credential_tests::MemoryBackend::default(),
            ))),
        )
        .unwrap();
        let mut current = StoreData::default();
        let mut next = current.clone();
        next.legacy_sources.insert("public-marker".into());
        let durable = store
            .commit_with_sync(&mut current, next, |_| {
                Err(std::io::Error::other("simulated directory sync failure"))
            })
            .unwrap();
        assert!(!durable);
        assert!(current.legacy_sources.contains("public-marker"));
        let disk: StoreData = serde_json::from_slice(&fs::read(&store.path).unwrap()).unwrap();
        assert!(disk.legacy_sources.contains("public-marker"));
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn unconfirmed_metadata_never_cleans_the_previous_secret_until_sync_retry_succeeds() {
        let root = std::env::temp_dir().join(format!("ops-cleanup-sync-{}", Uuid::new_v4()));
        let credentials = Arc::new(CredentialStore::with_backend(Arc::new(
            crate::credential_tests::MemoryBackend::default(),
        )));
        let store = ProfileStore::open(root.join("profiles.json"), credentials.clone()).unwrap();
        store
            .save(
                ProfileDraft {
                    id: None,
                    expected_revision: None,
                    name: "public".into(),
                    settings: ProfileSettings::Mysql(DatabaseSettings {
                        host: "localhost".into(),
                        port: 3306,
                        username: "reader".into(),
                        database: "".into(),
                        tls: "verifyIdentity".into(),
                    }),
                },
                CredentialChange::Replace(b"synthetic-old".to_vec()),
            )
            .unwrap();
        let mut data = store.data.lock().unwrap();
        let old = data.profiles[0].secret_ref.clone().unwrap();
        let mut next = data.clone();
        next.profiles.clear();
        add_pending(&mut next, old.clone());
        assert!(
            !store
                .commit_with_sync(&mut data, next, |_| Err(std::io::Error::other("sync")))
                .unwrap()
        );
        store.cleanup_with_sync(&mut data, |_| Err(std::io::Error::other("sync")));
        assert_eq!(data.pending_cleanup.len(), 1);
        assert!(credentials.get(&old.owner, &old).is_ok());
        store.cleanup_with_sync(&mut data, sync_parent);
        assert!(data.pending_cleanup.is_empty());
        assert!(credentials.get(&old.owner, &old).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
