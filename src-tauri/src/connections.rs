//! Profile commands never return credential material. Network lifetimes belong to runtimes.
use crate::{
    credentials::CredentialStore,
    db::{DbConfig, DbKind, DbService, DbSession, DbTls},
    kube_import,
    kubernetes::{KubeRuntimes, KubeSelection, Scoped},
    profiles::*,
    ssh::{ConnectRequest, Endpoint, HostProbe, SshService},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftBinding {
    draft_id: String,
    id: Option<String>,
    expected_revision: Option<u64>,
}
impl DraftBinding {
    fn validate(&self, store: &ProfileStore) -> Result<(), String> {
        if Uuid::parse_str(&self.draft_id).is_err() {
            return Err("导入草稿身份无效".into());
        }
        match (&self.id, self.expected_revision) {
            (Some(id), Some(revision)) => {
                store.snapshot(id, revision)?;
            }
            (None, None) => {}
            _ => return Err("编辑版本无效".into()),
        }
        Ok(())
    }
    fn matches(&self, draft: &ProfileDraft) -> bool {
        self.id == draft.id && self.expected_revision == draft.expected_revision
    }
}
struct PendingImport {
    binding: DraftBinding,
    imported: kube_import::ImportedKube,
    created: Instant,
}
#[derive(Default)]
struct ImportRegistry {
    pending: Mutex<HashMap<String, PendingImport>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    token: String,
    settings: crate::profiles::KubeSettings,
}
impl ImportRegistry {
    fn put(
        &self,
        binding: DraftBinding,
        imported: kube_import::ImportedKube,
    ) -> Result<ImportPreview, String> {
        let mut pending = self.pending.lock().map_err(|_| "导入状态不可用")?;
        pending.retain(|_, v| v.created.elapsed() < Duration::from_secs(120));
        // Re-importing the same draft replaces only its own uncommitted snapshot.
        pending.retain(|_, v| v.binding.draft_id != binding.draft_id);
        if pending.len() >= 4 {
            return Err("最多同时保留4份导入预览，请关闭其他草稿或稍后再试".into());
        }
        let token = Uuid::new_v4().to_string();
        let settings = imported.settings.clone();
        pending.insert(
            token.clone(),
            PendingImport {
                binding,
                imported,
                created: Instant::now(),
            },
        );
        Ok(ImportPreview { token, settings })
    }
    fn take(
        &self,
        token: &str,
        binding: &DraftBinding,
    ) -> Result<kube_import::ImportedKube, String> {
        let mut pending = self.pending.lock().map_err(|_| "导入状态不可用")?;
        if !pending.get(token).is_some_and(|p| {
            p.binding == *binding && p.created.elapsed() < Duration::from_secs(120)
        }) {
            return Err("导入已过期或不属于此编辑草稿，请重新导入".into());
        }
        Ok(pending.remove(token).ok_or("导入已过期")?.imported)
    }
    fn discard(&self, binding: &DraftBinding) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|_, p| p.binding != *binding);
        }
    }
}
struct BoundChallenge {
    id: String,
    revision: u64,
    created: Instant,
}
#[derive(Default)]
struct ProfileChallenges {
    pending: Mutex<HashMap<String, BoundChallenge>>,
}
impl ProfileChallenges {
    fn put(&self, token: String, id: String, revision: u64) -> Result<(), String> {
        let mut p = self.pending.lock().map_err(|_| "指纹确认状态不可用")?;
        p.retain(|_, v| v.created.elapsed() < Duration::from_secs(120));
        if p.len() >= 16 {
            return Err("待确认连接过多".into());
        }
        p.insert(
            token,
            BoundChallenge {
                id,
                revision,
                created: Instant::now(),
            },
        );
        Ok(())
    }
    fn take(&self, token: &str, id: &str, revision: u64) -> Result<(), String> {
        let mut p = self.pending.lock().map_err(|_| "指纹确认状态不可用")?;
        if !p.get(token).is_some_and(|v| {
            v.id == id && v.revision == revision && v.created.elapsed() < Duration::from_secs(120)
        }) {
            return Err("指纹确认不属于当前连接版本，请重新核验".into());
        }
        p.remove(token);
        Ok(())
    }
}
pub struct Connections {
    pub store: Arc<ProfileStore>,
    imports: ImportRegistry,
    challenges: ProfileChallenges,
}
impl Connections {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        Ok(Self {
            store: Arc::new(ProfileStore::open(
                path,
                Arc::new(CredentialStore::system()),
            )?),
            imports: ImportRegistry::default(),
            challenges: ProfileChallenges::default(),
        })
    }
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum CredentialInput {
    Keep,
    ReplacePassword {
        value: String,
    },
    ImportKube {
        token: String,
        binding: DraftBinding,
    },
    Remove,
}
fn apply_save(
    state: &Connections,
    draft: ProfileDraft,
    input: CredentialInput,
) -> Result<MutationReceipt, String> {
    let change = match input {
        CredentialInput::Keep => CredentialChange::Keep,
        CredentialInput::Remove => CredentialChange::Remove,
        CredentialInput::ReplacePassword { value } => {
            if matches!(draft.settings, ProfileSettings::Kubernetes(_)) {
                return Err("集群凭据必须通过导入保存".into());
            }
            CredentialChange::Replace(value.into_bytes())
        }
        CredentialInput::ImportKube { token, binding } => {
            binding.validate(&state.store)?;
            if !binding.matches(&draft) {
                return Err("导入与编辑版本不符".into());
            }
            let imported = state.imports.take(&token, &binding)?;
            let ProfileSettings::Kubernetes(requested) = &draft.settings else {
                return Err("导入只适用于Kubernetes连接".into());
            };
            if requested.snapshot_id != imported.settings.snapshot_id
                || requested.contexts != imported.settings.contexts
            {
                return Err("导入摘要已变化，请重新导入".into());
            }
            CredentialChange::Replace(imported.material)
        }
    };
    state.store.save(draft, change)
}
#[tauri::command]
pub async fn profiles_list(
    state: tauri::State<'_, Arc<Connections>>,
) -> Result<ProfileListing, String> {
    state.store.list()
}
#[tauri::command]
pub async fn profiles_save(
    state: tauri::State<'_, Arc<Connections>>,
    draft: ProfileDraft,
    credential: CredentialInput,
) -> Result<MutationReceipt, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || apply_save(&state, draft, credential))
        .await
        .map_err(|_| "保存任务已停止")?
}
#[tauri::command]
pub async fn profiles_remove(
    state: tauri::State<'_, Arc<Connections>>,
    id: String,
    revision: u64,
) -> Result<MutationReceipt, String> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.remove(&id, revision))
        .await
        .map_err(|_| "删除任务已停止")?
}
#[tauri::command]
pub async fn profiles_retry_cleanup(
    state: tauri::State<'_, Arc<Connections>>,
) -> Result<usize, String> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.retry_cleanup())
        .await
        .map_err(|_| "清理任务已停止")?
}
#[tauri::command]
pub async fn profiles_import_legacy(
    state: tauri::State<'_, Arc<Connections>>,
    profiles: Vec<LegacyProfile>,
) -> Result<usize, String> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.import_legacy(profiles))
        .await
        .map_err(|_| "迁移任务已停止")?
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ImportSource {
    File { path: String },
    Text { text: String },
}
#[tauri::command]
pub async fn kube_test_connection(
    source: ImportSource,
    context: Option<String>,
    namespace: Option<String>,
    use_source_default: bool,
) -> Result<(), String> {
    let config = tokio::task::spawn_blocking(move || {
        let imported = match source {
            ImportSource::File { path } => kube_import::import_file(&path)?,
            ImportSource::Text { text } => kube_import::import_text(&text)?,
        };
        let choice = if use_source_default {
            context.or(imported.settings.context.clone())
        } else {
            context
        };
        let choice = choice.ok_or("请先解析并选择Context")?;
        if !imported.settings.contexts.contains(&choice) {
            return Err("配置中不存在此Context".into());
        }
        Ok::<_, String>((
            kube_import::decode_snapshot(&imported.material, &imported.settings)?,
            choice,
        ))
    })
    .await
    .map_err(|_| "Kubernetes测试解析任务已停止")??;
    crate::kubernetes::test_connection(config.0, Some(&config.1), namespace.as_deref()).await
}
#[tauri::command]
pub async fn profiles_import_kube(
    state: tauri::State<'_, Arc<Connections>>,
    binding: DraftBinding,
    source: ImportSource,
) -> Result<ImportPreview, String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        binding.validate(&state.store)?;
        let imported = match source {
            ImportSource::File { path } => kube_import::import_file(&path)?,
            ImportSource::Text { text } => kube_import::import_text(&text)?,
        };
        state.imports.put(binding, imported)
    })
    .await
    .map_err(|_| "导入任务已停止")?
}
#[tauri::command]
pub fn profiles_discard_import(state: tauri::State<'_, Arc<Connections>>, binding: DraftBinding) {
    state.imports.discard(&binding);
}
async fn resolve(
    state: &Connections,
    id: String,
    revision: u64,
) -> Result<ResolvedProfile, String> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.resolve(&id, revision))
        .await
        .map_err(|_| "凭据读取任务已停止")?
}
fn password(secret: Option<Vec<u8>>) -> Result<String, String> {
    String::from_utf8(secret.ok_or("此连接尚无已保存凭据，请编辑后保存")?)
        .map_err(|_| "已保存凭据格式无效，请重新输入".to_string())
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum DbTestCredential {
    Keep,
    ReplacePassword { value: String },
    Remove,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbProfileTest {
    id: String,
    expected_revision: u64,
    settings: ProfileSettings,
    credential: DbTestCredential,
}
fn prepare_db_test(store: &ProfileStore, request: DbProfileTest) -> Result<DbConfig, String> {
    request.settings.validate()?;
    let kind = match &request.settings {
        ProfileSettings::Mysql(_) => DbKind::Mysql,
        ProfileSettings::Doris(_) => DbKind::Doris,
        _ => return Err("连接类型不是数据库".into()),
    };
    let saved = store.snapshot(&request.id, request.expected_revision)?;
    if !matches!(
        saved.settings,
        ProfileSettings::Mysql(_) | ProfileSettings::Doris(_)
    ) {
        return Err("连接类型不是数据库".into());
    }
    let secret = match request.credential {
        DbTestCredential::Keep => {
            if saved.settings.target() != request.settings.target() {
                return Err("认证目标已变化，请选择替换凭据后测试。".into());
            }
            password(
                store
                    .resolve(&request.id, request.expected_revision)?
                    .secret,
            )?
        }
        DbTestCredential::ReplacePassword { value } => value,
        DbTestCredential::Remove => String::new(),
    };
    store.snapshot(&request.id, request.expected_revision)?;
    let settings = match request.settings {
        ProfileSettings::Mysql(s) | ProfileSettings::Doris(s) => s,
        _ => unreachable!(),
    };
    let tls = match settings.tls.as_str() {
        "auto" => DbTls::Auto,
        "verifyIdentity" => DbTls::VerifyIdentity,
        "required" => DbTls::Required,
        "disabled" => DbTls::Disabled,
        _ => return Err("TLS设置无效".into()),
    };
    Ok(DbConfig {
        name: String::new(),
        host: settings.host,
        port: settings.port,
        username: settings.username,
        password: secret,
        database: settings.database,
        kind,
        tls,
    })
}
#[tauri::command]
pub async fn profiles_test_db(
    state: tauri::State<'_, Arc<Connections>>,
    request: DbProfileTest,
) -> Result<(), String> {
    let store = state.store.clone();
    let id = request.id.clone();
    let revision = request.expected_revision;
    let resolved = store.clone();
    let config = tokio::task::spawn_blocking(move || prepare_db_test(&resolved, request))
        .await
        .map_err(|_| "数据库测试凭据读取任务已停止")??;
    crate::db::test_connection(config).await?;
    store.snapshot(&id, revision)?;
    Ok(())
}
#[tauri::command]
pub async fn profiles_connect_db(
    state: tauri::State<'_, Arc<Connections>>,
    service: tauri::State<'_, DbService>,
    id: String,
    revision: u64,
) -> Result<DbSession, String> {
    let resolved = resolve(&state, id.clone(), revision).await?;
    let kind = match &resolved.profile.settings {
        ProfileSettings::Mysql(_) => DbKind::Mysql,
        ProfileSettings::Doris(_) => DbKind::Doris,
        _ => return Err("连接类型不是数据库".into()),
    };
    let settings = match resolved.profile.settings {
        ProfileSettings::Mysql(s) | ProfileSettings::Doris(s) => s,
        _ => unreachable!(),
    };
    let tls = match settings.tls.as_str() {
        "auto" => DbTls::Auto,
        "verifyIdentity" => DbTls::VerifyIdentity,
        "required" => DbTls::Required,
        "disabled" => DbTls::Disabled,
        _ => return Err("TLS设置无效".into()),
    };
    let result = service
        .connect(DbConfig {
            name: resolved.profile.name,
            host: settings.host,
            port: settings.port,
            username: settings.username,
            password: password(resolved.secret)?,
            database: settings.database,
            kind,
            tls,
        })
        .await?;
    if let Err(error) = state.store.snapshot(&id, revision) {
        let _ = service.disconnect(&result.id);
        return Err(error);
    }
    Ok(result)
}
#[tauri::command]
pub async fn profiles_probe_ssh(
    state: tauri::State<'_, Arc<Connections>>,
    service: tauri::State<'_, Arc<SshService>>,
    id: String,
    revision: u64,
) -> Result<HostProbe, String> {
    let profile = state.store.snapshot(&id, revision)?;
    let ProfileSettings::Ssh(settings) = profile.settings else {
        return Err("连接类型不是SSH".into());
    };
    let endpoint = Endpoint {
        host: settings.host,
        port: settings.port,
        username: settings.username,
    };
    let service = service.inner().clone();
    let probe = tokio::task::spawn_blocking(move || service.probe(endpoint))
        .await
        .map_err(|_| "指纹探测任务已停止")??;
    state.store.snapshot(&id, revision)?;
    state.challenges.put(probe.token.clone(), id, revision)?;
    Ok(probe)
}
#[tauri::command]
pub async fn profiles_connect_ssh(
    state: tauri::State<'_, Arc<Connections>>,
    service: tauri::State<'_, Arc<SshService>>,
    id: String,
    revision: u64,
    token: String,
    trust_host: bool,
) -> Result<String, String> {
    state.challenges.take(&token, &id, revision)?;
    let resolved = resolve(&state, id.clone(), revision).await?;
    let ProfileSettings::Ssh(settings) = resolved.profile.settings else {
        return Err("连接类型不是SSH".into());
    };
    let secret = password(resolved.secret)?;
    let key = settings.authentication == "key";
    let request = ConnectRequest {
        token,
        trust_host,
        password: if key { None } else { Some(secret.clone()) },
        private_key: None,
        private_key_path: if key { settings.private_key_path } else { None },
        passphrase: if key { Some(secret) } else { None },
        cols: 120,
        rows: 36,
    };
    let runtime = service.inner().clone();
    let session = tokio::task::spawn_blocking(move || runtime.connect(request))
        .await
        .map_err(|_| "SSH连接任务已停止")??;
    if let Err(error) = state.store.snapshot(&id, revision) {
        let _ = service.disconnect(&session);
        return Err(error);
    }
    Ok(session)
}
fn saved_kube_context(
    settings: &crate::profiles::KubeSettings,
    requested: Option<&str>,
) -> Result<String, String> {
    let choice = requested
        .or(settings.context.as_deref())
        .ok_or("请选择要打开的Context")?;
    if !settings.contexts.iter().any(|name| name == choice) {
        return Err("配置中不存在此Context".into());
    }
    Ok(choice.to_owned())
}
#[tauri::command]
pub async fn profiles_open_kube(
    state: tauri::State<'_, Arc<Connections>>,
    service: tauri::State<'_, KubeRuntimes>,
    id: String,
    revision: u64,
    context: Option<String>,
) -> Result<Scoped<KubeSelection>, String> {
    let resolved = resolve(&state, id.clone(), revision).await?;
    let ProfileSettings::Kubernetes(settings) = resolved.profile.settings else {
        return Err("连接类型不是Kubernetes".into());
    };
    let choice = saved_kube_context(&settings, context.as_deref())?;
    let config = kube_import::decode_snapshot(
        &resolved.secret.ok_or("此集群尚无已保存配置，请重新导入")?,
        &settings,
    )?;
    let result = service
        .open_config(config, Some(&choice), settings.namespace.as_deref())
        .await?;
    if let Err(error) = state.store.snapshot(&id, revision) {
        let _ = service
            .close(&result.session_id, result.data.generation)
            .await;
        return Err(error);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> DraftBinding {
        DraftBinding {
            draft_id: Uuid::new_v4().to_string(),
            id: None,
            expected_revision: None,
        }
    }
    fn imported() -> kube_import::ImportedKube {
        kube_import::ImportedKube {
            settings: crate::profiles::KubeSettings {
                context: None,
                namespace: None,
                contexts: vec!["ctx".into()],
                snapshot_id: Uuid::new_v4().to_string(),
            },
            material: vec![],
        }
    }
    #[test]
    fn import_tokens_are_bounded_one_time_and_bound_to_exact_draft_and_revision() {
        let registry = ImportRegistry::default();
        let b = binding();
        let preview = registry.put(b.clone(), imported()).unwrap();
        let mut other = b.clone();
        other.expected_revision = Some(2);
        assert!(registry.take(&preview.token, &other).is_err());
        assert!(registry.take(&preview.token, &b).is_ok());
        assert!(registry.take(&preview.token, &b).is_err());
        for _ in 0..4 {
            registry.put(binding(), imported()).unwrap();
        }
        assert!(registry.put(binding(), imported()).is_err());
        registry
            .pending
            .lock()
            .unwrap()
            .values_mut()
            .for_each(|v| v.created = Instant::now() - Duration::from_secs(121));
        assert!(registry.put(binding(), imported()).is_ok());
    }
    #[test]
    fn ssh_profile_challenges_cannot_be_swapped_between_profiles_or_revisions() {
        let registry = ProfileChallenges::default();
        registry.put("token".into(), "A".into(), 2).unwrap();
        assert!(registry.take("token", "B", 2).is_err());
        assert!(registry.take("token", "A", 3).is_err());
        registry.take("token", "A", 2).unwrap();
        assert!(registry.take("token", "A", 2).is_err());
    }
}

#[cfg(test)]
mod context_choice_tests {
    use super::*;
    #[test]
    fn a_cleared_saved_context_requires_an_explicit_declared_choice() {
        let mut settings = crate::profiles::KubeSettings {
            context: None,
            namespace: None,
            contexts: vec!["a".into()],
            snapshot_id: Uuid::new_v4().to_string(),
        };
        assert!(saved_kube_context(&settings, None).is_err());
        assert!(saved_kube_context(&settings, Some("other")).is_err());
        assert_eq!(saved_kube_context(&settings, Some("a")).unwrap(), "a");
        settings.context = Some("a".into());
        assert_eq!(saved_kube_context(&settings, None).unwrap(), "a");
    }
}

#[cfg(test)]
mod db_draft_test_tests {
    use super::*;
    use crate::{credential_tests::MemoryBackend, credentials::CredentialStore};
    use std::sync::atomic::Ordering;
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn setup() -> (Temp, ProfileStore, Arc<MemoryBackend>, ProfileDto) {
        let temp = Temp(std::env::temp_dir().join(format!("ops-db-test-{}", Uuid::new_v4())));
        let backend = Arc::new(MemoryBackend::default());
        let store = ProfileStore::open(
            temp.0.join("profiles.json"),
            Arc::new(CredentialStore::with_backend(backend.clone())),
        )
        .unwrap();
        let profile = store
            .save(
                ProfileDraft {
                    id: None,
                    expected_revision: None,
                    name: "fixture".into(),
                    settings: ProfileSettings::Doris(DatabaseSettings {
                        host: "127.0.0.1".into(),
                        port: 9030,
                        username: "reader".into(),
                        database: "demo".into(),
                        tls: "disabled".into(),
                    }),
                },
                CredentialChange::Replace(b"synthetic-only".to_vec()),
            )
            .unwrap()
            .profile
            .unwrap();
        (temp, store, backend, profile)
    }
    fn request(profile: &ProfileDto, credential: DbTestCredential) -> DbProfileTest {
        DbProfileTest {
            id: profile.id.clone(),
            expected_revision: profile.revision,
            settings: profile.settings.clone(),
            credential,
        }
    }
    #[test]
    fn edited_keep_test_rejects_changed_target_or_revision_before_reading_a_secret() {
        let (_temp, store, backend, profile) = setup();
        for field in ["host", "port", "username", "tls", "driver", "revision"] {
            let mut req = request(&profile, DbTestCredential::Keep);
            if field == "revision" {
                req.expected_revision += 1;
            } else if let ProfileSettings::Doris(ref mut s) = req.settings {
                match field {
                    "host" => s.host = "other.test".into(),
                    "port" => s.port = 3306,
                    "username" => s.username = "other".into(),
                    "tls" => s.tls = "verifyIdentity".into(),
                    _ => {}
                }
                if field == "driver" {
                    req.settings = ProfileSettings::Mysql(s.clone());
                }
            }
            let before = backend.reads.load(Ordering::SeqCst);
            assert!(
                prepare_db_test(&store, req).is_err(),
                "changed {field} must be rejected"
            );
            assert_eq!(
                backend.reads.load(Ordering::SeqCst),
                before,
                "must reject before credential read"
            );
        }
    }
    #[test]
    fn edited_keep_replace_and_remove_probes_do_not_write_metadata_or_credentials() {
        let (temp, store, backend, profile) = setup();
        let before = std::fs::read(temp.0.join("profiles.json")).unwrap();
        let config = prepare_db_test(&store, request(&profile, DbTestCredential::Keep)).unwrap();
        assert!(config.password == "synthetic-only");
        assert!(config.name.is_empty());
        let reads = backend.reads.load(Ordering::SeqCst);
        let mut req = request(
            &profile,
            DbTestCredential::ReplacePassword {
                value: "replacement-only".into(),
            },
        );
        if let ProfileSettings::Doris(s) = &mut req.settings {
            s.host = "other.test".into();
        }
        let config = prepare_db_test(&store, req).unwrap();
        assert!(config.password == "replacement-only");
        assert_eq!(config.host, "other.test");
        let config = prepare_db_test(&store, request(&profile, DbTestCredential::Remove)).unwrap();
        assert!(config.password.is_empty());
        assert_eq!(backend.reads.load(Ordering::SeqCst), reads);
        assert_eq!(std::fs::read(temp.0.join("profiles.json")).unwrap(), before);
        assert!(
            store
                .resolve(&profile.id, profile.revision)
                .unwrap()
                .secret
                .unwrap()
                == b"synthetic-only"
        );
    }
}
