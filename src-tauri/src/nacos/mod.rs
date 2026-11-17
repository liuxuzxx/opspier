//! Native Nacos administration. Authentication material never enters public profile metadata.
pub mod config;
mod http;
pub mod naming;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

pub(super) async fn deadline<T>(
    write: bool,
    operation: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(std::time::Duration::from_secs(15), operation)
        .await
        .map_err(|_| {
            if write {
                UNKNOWN.to_owned()
            } else {
                "Nacos 操作超时（15秒总期限）".into()
            }
        })?
}
pub(super) const INVALID: &str = "Nacos 连接配置无效，请检查 HTTP API 地址、认证方式和命名空间";
pub(super) const CONFLICT: &str = "目标已被其他操作修改，请刷新后重新确认；未发布的草稿已保留";
pub(super) const UNKNOWN: &str = "写请求结果尚未确认，请刷新检查服务端状态后再操作，避免重复提交";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NacosSettings {
    pub endpoint: String,
    pub authentication: String,
    pub namespace: String,
}
impl NacosSettings {
    pub fn validate(&self) -> Result<(), String> {
        let url = url::Url::parse(&self.endpoint).map_err(|_| INVALID)?;
        if self.endpoint.len() > 2048
            || !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || self.endpoint.chars().any(char::is_control)
            || !matches!(self.authentication.as_str(), "password" | "none")
            || self.namespace.len() > 256
            || self.namespace.chars().any(char::is_control)
        {
            return Err(INVALID.into());
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NacosMaterial {
    pub username: String,
    pub password: String,
}
impl NacosMaterial {
    pub fn validate(&self) -> Result<(), String> {
        if self.username.trim().is_empty()
            || self.username.len() > 256
            || self.password.is_empty()
            || self.password.len() > 4096
            || self
                .username
                .chars()
                .chain(self.password.chars())
                .any(char::is_control)
        {
            return Err("Nacos 账号或密码格式无效".into());
        }
        Ok(())
    }
    pub fn decode(value: &[u8]) -> Result<Self, String> {
        let result: Self =
            serde_json::from_slice(value).map_err(|_| "保存的 Nacos 凭据格式无效，请重新输入")?;
        result.validate()?;
        Ok(result)
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NacosSession {
    pub id: String,
    pub endpoint: String,
    pub version: String,
    pub namespace: String,
}
#[derive(Clone, Serialize)]
pub struct Namespace {
    pub id: String,
    pub name: String,
}
struct AuthToken {
    value: String,
    expires: Instant,
}
pub(super) struct Runtime {
    settings: NacosSettings,
    client: reqwest::Client,
    material: Option<NacosMaterial>,
    token: AsyncMutex<Option<AuthToken>>,
    closed: AtomicBool,
    writes: AtomicUsize,
    mutation: AsyncMutex<()>,
}
#[derive(Default)]
pub struct NacosService {
    runtimes: Mutex<HashMap<String, Arc<Runtime>>>,
}
pub(super) struct WriteGuard {
    runtime: Arc<Runtime>,
}
impl Drop for WriteGuard {
    fn drop(&mut self) {
        self.runtime.writes.fetch_sub(1, Ordering::SeqCst);
    }
}
impl NacosService {
    pub(super) fn runtime(&self, id: &str) -> Result<Arc<Runtime>, String> {
        self.runtimes
            .lock()
            .map_err(|_| "Nacos 连接状态不可用")?
            .get(id)
            .cloned()
            .ok_or("Nacos 连接已关闭".into())
    }
    pub(super) fn begin_write(&self, id: &str) -> Result<WriteGuard, String> {
        let all = self.runtimes.lock().map_err(|_| "Nacos 连接状态不可用")?;
        let runtime = all.get(id).cloned().ok_or("Nacos 连接已关闭")?;
        if runtime
            .writes
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("已有 Nacos 写操作进行中，请等待核验结果".into());
        }
        Ok(WriteGuard { runtime })
    }
    pub async fn connect(
        &self,
        settings: NacosSettings,
        material: Option<NacosMaterial>,
    ) -> Result<NacosSession, String> {
        let runtime = Runtime::new(settings, material)?;
        let version = deadline(false, runtime.probe()).await?;
        let mut all = self.runtimes.lock().map_err(|_| "Nacos 连接状态不可用")?;
        if all.len() >= 16 {
            return Err("最多同时打开16个 Nacos 连接".into());
        }
        let session = NacosSession {
            id: Uuid::new_v4().to_string(),
            endpoint: runtime.settings.endpoint.clone(),
            version,
            namespace: namespace_id(&runtime.settings.namespace),
        };
        all.insert(session.id.clone(), Arc::new(runtime));
        Ok(session)
    }
    pub async fn namespaces(&self, id: &str) -> Result<Vec<Namespace>, String> {
        deadline(false, self.runtime(id)?.namespaces()).await
    }
    pub fn close(&self, id: &str) -> Result<(), String> {
        let mut all = self.runtimes.lock().map_err(|_| "Nacos 连接状态不可用")?;
        if let Some(runtime) = all.get(id) {
            if runtime.writes.load(Ordering::SeqCst) > 0 {
                return Err("仍有 Nacos 写操作进行中，请等待结果后关闭".into());
            }
            runtime.closed.store(true, Ordering::SeqCst);
        }
        all.remove(id);
        Ok(())
    }
}
pub async fn test_connection(
    settings: NacosSettings,
    material: Option<NacosMaterial>,
) -> Result<(), String> {
    deadline(false, Runtime::new(settings, material)?.probe())
        .await
        .map(|_| ())
}
pub(super) fn namespace_id(value: &str) -> String {
    if value == "public" {
        String::new()
    } else {
        value.into()
    }
}
pub(super) fn naming_namespace(value: &str) -> String {
    if value.is_empty() {
        "public".into()
    } else {
        value.into()
    }
}
pub(super) fn text(value: &str, max: usize, empty: bool) -> bool {
    (empty || !value.trim().is_empty())
        && value.len() <= max
        && !value.chars().any(char::is_control)
}
#[tauri::command]
pub async fn nacos_namespaces(
    service: tauri::State<'_, NacosService>,
    session_id: String,
) -> Result<Vec<Namespace>, String> {
    service.namespaces(&session_id).await
}
#[tauri::command]
pub fn nacos_close(
    service: tauri::State<'_, NacosService>,
    session_id: String,
) -> Result<(), String> {
    service.close(&session_id)
}
#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod tests;
