//! Strictly read-only Kubernetes requests. No default kubeconfig lookup or process execution.
#[cfg(test)]
#[path = "kubernetes_protocol_tests.rs"]
mod protocol_tests;
#[path = "kubernetes_query.rs"]
pub mod query;
#[cfg(test)]
#[path = "kubernetes_query_tests.rs"]
mod query_tests;
use futures::{TryStreamExt, io::AsyncReadExt};
use http_body_util::BodyExt;
use kube::{
    Client, Config,
    api::{GetParams, ListParams, LogParams, WatchParams},
    config::{KubeConfigOptions, Kubeconfig},
    core::Request,
};
use serde::Serialize;
use serde_json::{Value, json};
#[cfg(test)]
use std::path::Path;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
const DETAIL_LIMIT: usize = 2 * 1024 * 1024;
const LOG_LIMIT: usize = 256 * 1024;
const POLL_LIMIT: usize = 64 * 1024;

#[derive(Default)]
pub struct KubeService {
    hub: Mutex<Hub>,
    query_gate: Mutex<()>,
    catalog_gate: Mutex<()>,
}
#[derive(Default)]
struct Hub {
    generation: u64,
    session: Option<Session>,
}
struct Session {
    config: Kubeconfig,
    client: Client,
    context: String,
    namespace: String,
    cancel: watch::Sender<bool>,
    logs: Option<LogSession>,
    logs_revision: u64,
    browser: query::BrowserState,
}
struct LogSession {
    id: String,
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
    buffer: Arc<Mutex<LogBuffer>>,
}
impl Drop for LogSession {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        self.task.abort();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        self.browser.cancel_all();
    }
}
struct Snapshot {
    client: Client,
    namespace: String,
    generation: u64,
    cancel: watch::Receiver<bool>,
}
#[derive(Default)]
struct LogBuffer {
    text: String,
    dropped: usize,
    done: bool,
    error: Option<String>,
}
impl LogBuffer {
    fn push(&mut self, text: &str) {
        self.text.push_str(text);
        if self.text.len() > LOG_LIMIT {
            let mut start = self.text.len() - LOG_LIMIT;
            while !self.text.is_char_boundary(start) {
                start += 1;
            }
            self.dropped += start;
            self.text.drain(..start);
        }
    }
    fn take(&mut self) -> String {
        let mut end = self.text.len().min(POLL_LIMIT);
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }
        let tail = self.text.split_off(end);
        std::mem::replace(&mut self.text, tail)
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeSelection {
    pub(crate) generation: u64,
    context: String,
    namespace: String,
    contexts: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeRow {
    name: String,
    namespace: String,
    kind: String,
    status: String,
    created: String,
    containers: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubePage {
    generation: u64,
    items: Vec<KubeRow>,
    continue_token: String,
    resource_version: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeDetail {
    generation: u64,
    yaml: String,
    containers: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeNamespaces {
    generation: u64,
    namespaces: Vec<String>,
    truncated: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeLogStarted {
    generation: u64,
    log_id: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeLogPoll {
    generation: u64,
    log_id: String,
    text: String,
    dropped_bytes: usize,
    done: bool,
    error: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeChange {
    event: String,
    row: KubeRow,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeWatch {
    generation: u64,
    resource_version: String,
    changes: Vec<KubeChange>,
    expired: bool,
}

fn page_limit(limit: u32) -> u32 {
    limit.clamp(1, 200)
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.')
        && name != "."
        && name != ".."
}
fn namespace_valid(name: &str) -> bool {
    safe_name(name) && name.len() <= 63 && !name.contains('.')
}
fn resource_path(kind: &str, namespace: &str) -> Result<String, String> {
    if !namespace_valid(namespace) {
        return Err("命名空间格式无效".into());
    }
    let (prefix, global) = match kind {
        "pods" | "services" | "configmaps" | "secrets" | "persistentvolumeclaims" | "events" => {
            ("/api/v1", false)
        }
        "persistentvolumes" => ("/api/v1", true),
        "deployments" | "statefulsets" | "daemonsets" => ("/apis/apps/v1", false),
        "jobs" | "cronjobs" => ("/apis/batch/v1", false),
        _ => return Err("不支持的只读资源类型".into()),
    };
    Ok(if global {
        format!("{prefix}/{kind}")
    } else {
        format!("{prefix}/namespaces/{namespace}/{kind}")
    })
}
pub(crate) fn validate_config(config: &Kubeconfig) -> Result<(), String> {
    if config.contexts.len() > 200
        || config
            .contexts
            .iter()
            .any(|c| c.name.is_empty() || c.name.len() > 256)
    {
        return Err("配置上下文数量或名称超出上限".into());
    }
    if config
        .auth_infos
        .iter()
        .filter_map(|user| user.auth_info.as_ref())
        .any(|user| user.exec.is_some() || user.auth_provider.is_some())
    {
        return Err("已拒绝外部认证插件（exec/auth-provider）；请提供使用静态令牌或客户端证书的专用只读配置".into());
    }
    Ok(())
}
fn context_selection(
    config: &Kubeconfig,
    context: Option<&str>,
    namespace: Option<&str>,
) -> Result<(String, String), String> {
    let selected = context
        .or(config.current_context.as_deref())
        .ok_or("请选择配置中存在的 context")?;
    let item = config
        .contexts
        .iter()
        .find(|item| item.name == selected)
        .and_then(|item| item.context.as_ref())
        .ok_or("配置中不存在此 context")?;
    let ns = namespace.or(item.namespace.as_deref()).unwrap_or("default");
    if !namespace_valid(ns) {
        return Err("命名空间格式无效".into());
    }
    Ok((selected.to_string(), ns.to_string()))
}
fn safe_resource(mut value: Value, secret: bool) -> Value {
    if secret {
        let original = &value["metadata"];
        let mut meta = serde_json::Map::new();
        for key in [
            "name",
            "namespace",
            "uid",
            "resourceVersion",
            "generation",
            "creationTimestamp",
            "deletionTimestamp",
            "labels",
        ] {
            if let Some(data) = original.get(key) {
                meta.insert(key.into(), data.clone());
            }
        }
        return json!({"apiVersion":"v1", "kind":"Secret", "metadata":meta});
    }
    if let Some(meta) = value.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
        meta.remove("annotations");
    }
    value
}
fn text_at(value: &Value, path: &str) -> String {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(512)
        .collect()
}
fn containers(value: &Value) -> Vec<String> {
    [
        "/spec/containers",
        "/spec/initContainers",
        "/spec/ephemeralContainers",
    ]
    .iter()
    .filter_map(|path| value.pointer(path).and_then(Value::as_array))
    .flatten()
    .filter_map(|item| item["name"].as_str())
    .take(100)
    .map(|name| name.chars().take(253).collect())
    .collect()
}
fn summary(value: &Value, kind: &str) -> KubeRow {
    let status = if let Some(phase) = value.pointer("/status/phase").and_then(Value::as_str) {
        phase.chars().take(100).collect()
    } else if let Some(ready) = value
        .pointer("/status/readyReplicas")
        .or(value.pointer("/status/numberReady"))
    {
        format!(
            "就绪 {} / {}",
            ready,
            value
                .pointer("/spec/replicas")
                .or(value.pointer("/status/desiredNumberScheduled"))
                .unwrap_or(&Value::Null)
        )
    } else if kind == "events" {
        text_at(value, "/reason")
    } else if kind == "services" {
        text_at(value, "/spec/type")
    } else {
        String::new()
    };
    KubeRow {
        name: text_at(value, "/metadata/name"),
        namespace: text_at(value, "/metadata/namespace"),
        kind: kind.into(),
        status,
        created: text_at(value, "/metadata/creationTimestamp"),
        containers: containers(value),
    }
}
fn api_error(error: kube::Error) -> String {
    match error {
        kube::Error::Api(response) => match response.code {
            401 => "Kubernetes 认证失败（401）".into(),
            403 => "Kubernetes 只读权限不足（403）".into(),
            404 => "Kubernetes 资源不存在（404）".into(),
            410 => "资源版本已过期，请刷新（410）".into(),
            code => format!("Kubernetes API 请求失败（HTTP {code}）"),
        },
        _ => "Kubernetes 网络、TLS 或认证连接失败".into(),
    }
}
async fn client_for(config: Kubeconfig, context: &str) -> Result<Client, String> {
    validate_config(&config)?;
    let mut config = Config::from_custom_kubeconfig(
        config,
        &KubeConfigOptions {
            context: Some(context.into()),
            ..Default::default()
        },
    )
    .await
    .map_err(|_| "无法加载指定 context 的认证或 TLS 配置".to_string())?;
    // Explicit timeouts; no fallback to in-cluster or another user's home config.
    config.connect_timeout = Some(Duration::from_secs(10));
    config.read_timeout = Some(Duration::from_secs(70));
    Client::try_from(config).map_err(|_| "无法建立 Kubernetes 客户端".into())
}
pub async fn test_connection(
    config: Kubeconfig,
    context: Option<&str>,
    namespace: Option<&str>,
) -> Result<(), String> {
    validate_config(&config)?;
    let (context, namespace) = context_selection(&config, context, namespace)?;
    let probe = async {
        let client = client_for(config, &context).await?;
        let request = Request::new(resource_path("pods", &namespace)?)
            .list(&ListParams::default().limit(1))
            .map_err(|_| "无法构造Kubernetes只读测试请求")?;
        // Read all statuses through the same limit before parsing any server body.
        let response = client
            .send(request.map(kube::client::Body::from))
            .await
            .map_err(api_error)?;
        let status = response.status();
        let reader = response
            .into_body()
            .into_data_stream()
            .map_err(std::io::Error::other)
            .into_async_read();
        futures::pin_mut!(reader);
        let mut bytes = Vec::new();
        reader
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Kubernetes测试响应读取失败")?;
        if bytes.len() > 64 * 1024 {
            return Err("Kubernetes测试响应超过64KiB上限".into());
        }
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 => "Kubernetes 认证失败（401）".into(),
                403 => "Kubernetes 只读权限不足（403）".into(),
                code => format!("Kubernetes API 请求失败（HTTP {code}）"),
            });
        }
        let _: Value = serde_json::from_slice(&bytes).map_err(|_| "Kubernetes测试响应格式无效")?;
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(15), probe)
        .await
        .map_err(|_| "Kubernetes测试连接超时")?
}
impl KubeService {
    async fn snapshot(&self, expected: u64) -> Result<Snapshot, String> {
        let hub = self.hub.lock().await;
        if hub.generation != expected {
            return Err("请求已取消：连接代次发生变化".into());
        }
        let session = hub.session.as_ref().ok_or("请先打开 kubeconfig")?;
        Ok(Snapshot {
            client: session.client.clone(),
            namespace: session.namespace.clone(),
            generation: expected,
            cancel: session.cancel.subscribe(),
        })
    }
    async fn verify(&self, expected: u64) -> Result<(), String> {
        let hub = self.hub.lock().await;
        if hub.generation != expected || hub.session.is_none() {
            return Err("请求已取消：连接代次发生变化".into());
        }
        Ok(())
    }
    fn selection(hub: &Hub) -> Result<KubeSelection, String> {
        let session = hub.session.as_ref().ok_or("请先打开 kubeconfig")?;
        Ok(KubeSelection {
            generation: hub.generation,
            context: session.context.clone(),
            namespace: session.namespace.clone(),
            contexts: session
                .config
                .contexts
                .iter()
                .map(|item| item.name.clone())
                .collect(),
        })
    }
    #[cfg(test)]
    async fn open(&self, path: &str, context: Option<&str>) -> Result<KubeSelection, String> {
        // Replacing a connection invalidates its streams even if the new config fails.
        let reserved = {
            let mut hub = self.hub.lock().await;
            hub.session = None;
            hub.generation += 1;
            hub.generation
        };
        if !Path::new(path).is_absolute() || path.len() > 4096 {
            return Err("请提供 kubeconfig 文件的绝对路径".into());
        }
        let path = path.to_string();
        let config = tokio::task::spawn_blocking(move || -> Result<Kubeconfig, String> {
            let meta =
                std::fs::metadata(&path).map_err(|_| "无法读取指定 kubeconfig 文件".to_string())?;
            if !meta.is_file() || meta.len() > 1024 * 1024 {
                return Err("kubeconfig 必须是小于 1MiB 的文件".into());
            }
            Kubeconfig::read_from(path).map_err(|_| "kubeconfig 格式或文件读取失败".into())
        })
        .await
        .map_err(|_| "配置读取任务已停止".to_string())??;
        self.install_config(config, context, None, reserved).await
    }
    async fn open_config(
        &self,
        config: Kubeconfig,
        context: Option<&str>,
        namespace: Option<&str>,
    ) -> Result<KubeSelection, String> {
        let reserved = {
            let mut hub = self.hub.lock().await;
            hub.session = None;
            hub.generation += 1;
            hub.generation
        };
        self.install_config(config, context, namespace, reserved)
            .await
    }
    async fn install_config(
        &self,
        config: Kubeconfig,
        context: Option<&str>,
        namespace: Option<&str>,
        reserved: u64,
    ) -> Result<KubeSelection, String> {
        validate_config(&config)?;
        let (context, namespace) = context_selection(&config, context, namespace)?;
        let client = client_for(config.clone(), &context).await?;
        let mut hub = self.hub.lock().await;
        if hub.generation != reserved {
            return Err("配置加载已取消".into());
        }
        let (cancel, _) = watch::channel(false);
        hub.session = Some(Session {
            config,
            client,
            context,
            namespace,
            cancel,
            logs: None,
            logs_revision: 0,
            browser: query::BrowserState::default(),
        });
        Self::selection(&hub)
    }
    async fn select(
        &self,
        expected: u64,
        context: &str,
        namespace: Option<&str>,
    ) -> Result<KubeSelection, String> {
        let (config, context, namespace) = {
            let hub = self.hub.lock().await;
            if hub.generation != expected {
                return Err("连接代次已变化".into());
            }
            let session = hub.session.as_ref().ok_or("请先打开 kubeconfig")?;
            let (context, namespace) =
                context_selection(&session.config, Some(context), namespace)?;
            (session.config.clone(), context, namespace)
        };
        let client = client_for(config, &context).await?;
        let mut hub = self.hub.lock().await;
        if hub.generation != expected {
            return Err("切换已取消".into());
        }
        let session = hub.session.as_mut().ok_or("连接已关闭")?;
        let _ = session.cancel.send(true);
        session.logs = None;
        let (cancel, _) = watch::channel(false);
        session.cancel = cancel;
        session.browser.cancel_all();
        session.browser = query::BrowserState::default();
        session.client = client;
        session.context = context;
        session.namespace = namespace;
        hub.generation += 1;
        Self::selection(&hub)
    }
    async fn stop(&self, expected: u64) -> Result<KubeSelection, String> {
        let mut hub = self.hub.lock().await;
        if hub.generation != expected {
            return Err("连接代次已变化".into());
        }
        let session = hub.session.as_mut().ok_or("请先打开 kubeconfig")?;
        let _ = session.cancel.send(true);
        session.logs = None;
        let (cancel, _) = watch::channel(false);
        session.cancel = cancel;
        hub.generation += 1;
        Self::selection(&hub)
    }
    async fn close(&self) {
        let mut hub = self.hub.lock().await;
        hub.session = None;
        hub.generation += 1;
    }
    async fn json(
        &self,
        mut snap: Snapshot,
        path: String,
        secret: bool,
        name: Option<&str>,
        params: ListParams,
    ) -> Result<Value, String> {
        if name.is_some_and(|name| !safe_name(name)) {
            return Err("资源名称格式无效".into());
        }
        let request_future = async {
            query::bounded_json(
                &snap.client,
                path,
                params,
                secret,
                name,
                RESPONSE_LIMIT,
                None,
            )
            .await
            .map(|(value, _)| value)
        };
        let value = tokio::select! {
            biased;
            _ = snap.cancel.changed() => Err("请求已取消".into()),
            result = tokio::time::timeout(Duration::from_secs(30), request_future) => result.map_err(|_| "Kubernetes 请求超时".to_string())?,
        }?;
        self.verify(snap.generation).await?;
        Ok(value)
    }
    async fn list(
        &self,
        expected: u64,
        kind: &str,
        limit: u32,
        token: &str,
    ) -> Result<KubePage, String> {
        if token.len() > 4096 {
            return Err("分页标记超过上限".into());
        }
        let snap = self.snapshot(expected).await?;
        let path = resource_path(kind, &snap.namespace)?;
        let limit = page_limit(limit);
        let mut params = ListParams::default().limit(limit);
        if !token.is_empty() {
            params = params.continue_token(token);
        }
        let value = self
            .json(snap, path, kind == "secrets", None, params)
            .await?;
        let items = value["items"].as_array().ok_or("资源列表格式无效")?;
        if items.len() > limit as usize {
            return Err("API 返回数量超过请求分页上限".into());
        }
        let token = text_at(&value, "/metadata/continue");
        // Continue tokens are opaque and may exceed the summary-field length.
        let token = value
            .pointer("/metadata/continue")
            .and_then(Value::as_str)
            .unwrap_or(&token)
            .to_string();
        if token.len() > 4096 {
            return Err("API 返回分页标记超过上限".into());
        }
        Ok(KubePage {
            generation: expected,
            items: items.iter().map(|item| summary(item, kind)).collect(),
            continue_token: token,
            resource_version: text_at(&value, "/metadata/resourceVersion"),
        })
    }
    async fn namespaces(&self, expected: u64) -> Result<KubeNamespaces, String> {
        let snap = self.snapshot(expected).await?;
        let value = self
            .json(
                snap,
                "/api/v1/namespaces".into(),
                true,
                None,
                ListParams::default().limit(200),
            )
            .await?;
        let items = value["items"].as_array().ok_or("命名空间列表格式无效")?;
        Ok(KubeNamespaces {
            generation: expected,
            namespaces: items
                .iter()
                .take(200)
                .map(|item| text_at(item, "/metadata/name"))
                .collect(),
            truncated: items.len() > 200 || !text_at(&value, "/metadata/continue").is_empty(),
        })
    }
    async fn detail(&self, expected: u64, kind: &str, name: &str) -> Result<KubeDetail, String> {
        let snap = self.snapshot(expected).await?;
        let path = resource_path(kind, &snap.namespace)?;
        let value = self
            .json(
                snap,
                path,
                kind == "secrets",
                Some(name),
                ListParams::default(),
            )
            .await?;
        let container_names = containers(&value);
        let yaml = serde_yaml::to_string(&safe_resource(value, kind == "secrets"))
            .map_err(|_| "YAML 转换失败".to_string())?;
        if yaml.len() > DETAIL_LIMIT {
            return Err("详情超过 2MiB 上限".into());
        }
        Ok(KubeDetail {
            generation: expected,
            yaml,
            containers: container_names,
        })
    }
    async fn logs_start(
        &self,
        expected: u64,
        pod: &str,
        container: &str,
        follow: bool,
        tail_lines: u32,
    ) -> Result<KubeLogStarted, String> {
        self.logs_start_in(expected, pod, container, follow, tail_lines, None)
            .await
    }
    async fn logs_start_in(
        &self,
        expected: u64,
        pod: &str,
        container: &str,
        follow: bool,
        tail_lines: u32,
        namespace: Option<&str>,
    ) -> Result<KubeLogStarted, String> {
        if namespace.is_some_and(|ns| !namespace_valid(ns)) {
            return Err("日志命名空间无效".into());
        }
        if !safe_name(pod) || !safe_name(container) {
            return Err("Pod 或容器名称格式无效".into());
        }
        let (snap, log_revision) = {
            let mut hub = self.hub.lock().await;
            if hub.generation != expected {
                return Err("连接代次已变化".into());
            }
            let session = hub.session.as_mut().ok_or("请先打开 kubeconfig")?;
            session.logs_revision += 1;
            session.logs = None;
            (
                Snapshot {
                    client: session.client.clone(),
                    namespace: namespace.unwrap_or(&session.namespace).to_owned(),
                    generation: expected,
                    cancel: session.cancel.subscribe(),
                },
                session.logs_revision,
            )
        };
        let request = Request::new(resource_path("pods", &snap.namespace)?)
            .logs(
                pod,
                &LogParams {
                    container: Some(container.into()),
                    follow,
                    tail_lines: Some(tail_lines.clamp(1, 1000) as i64),
                    limit_bytes: if follow { None } else { Some(LOG_LIMIT as i64) },
                    timestamps: true,
                    ..Default::default()
                },
            )
            .map_err(|_| "无法构造日志请求".to_string())?;
        let id = Uuid::new_v4().to_string();
        let buffer = Arc::new(Mutex::new(LogBuffer::default()));
        let output = buffer.clone();
        let (cancel, mut stopped) = watch::channel(false);
        let mut selection_cancel = snap.cancel;
        let task = tokio::spawn(async move {
            let run = async {
                let reader = snap
                    .client
                    .request_stream(request)
                    .await
                    .map_err(api_error)?;
                futures::pin_mut!(reader);
                let mut chunk = [0u8; 8192];
                let mut pending = Vec::with_capacity(8196);
                loop {
                    let size = reader
                        .read(&mut chunk)
                        .await
                        .map_err(|_| "日志连接读取失败".to_string())?;
                    if size == 0 {
                        if !pending.is_empty() {
                            output.lock().await.push(&String::from_utf8_lossy(&pending));
                        }
                        break;
                    }
                    pending.extend_from_slice(&chunk[..size]);
                    loop {
                        match std::str::from_utf8(&pending) {
                            Ok(text) => {
                                output.lock().await.push(text);
                                pending.clear();
                                break;
                            }
                            Err(error) => {
                                let valid = error.valid_up_to();
                                if valid > 0 {
                                    output
                                        .lock()
                                        .await
                                        .push(std::str::from_utf8(&pending[..valid]).unwrap_or(""));
                                }
                                match error.error_len() {
                                    Some(invalid) => {
                                        output.lock().await.push("\u{fffd}");
                                        pending.drain(..valid + invalid);
                                    }
                                    None => {
                                        pending.drain(..valid);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                Ok::<(), String>(())
            };
            let result = tokio::select! {biased; _=stopped.changed()=>Ok(()), _=selection_cancel.changed()=>Ok(()), result=run=>result};
            let mut buffer = output.lock().await;
            buffer.done = true;
            buffer.error = result.err();
        });
        let mut hub = self.hub.lock().await;
        if hub.generation != expected
            || hub
                .session
                .as_ref()
                .is_none_or(|session| session.logs_revision != log_revision)
        {
            task.abort();
            return Err("日志请求已取消".into());
        }
        hub.session.as_mut().unwrap().logs = Some(LogSession {
            id: id.clone(),
            cancel,
            task,
            buffer,
        });
        Ok(KubeLogStarted {
            generation: expected,
            log_id: id,
        })
    }
    async fn logs_poll(&self, expected: u64, log_id: &str) -> Result<KubeLogPoll, String> {
        let buffer = {
            let hub = self.hub.lock().await;
            if hub.generation != expected {
                return Err("日志请求代次已变化".into());
            }
            let log = hub
                .session
                .as_ref()
                .and_then(|session| session.logs.as_ref())
                .ok_or("日志流已停止")?;
            if log.id != log_id {
                return Err("日志流已被替换".into());
            }
            log.buffer.clone()
        };
        let mut buffer = buffer.lock().await;
        let text = buffer.take();
        Ok(KubeLogPoll {
            generation: expected,
            log_id: log_id.into(),
            text,
            dropped_bytes: std::mem::take(&mut buffer.dropped),
            done: buffer.done && buffer.text.is_empty(),
            error: buffer.error.clone(),
        })
    }
    async fn logs_stop(&self, expected: u64, log_id: Option<&str>) -> Result<(), String> {
        let mut hub = self.hub.lock().await;
        if hub.generation != expected {
            return Err("连接代次已变化".into());
        }
        let session = hub.session.as_mut().ok_or("连接已关闭")?;
        if log_id.is_some_and(|id| session.logs.as_ref().is_none_or(|log| log.id != id)) {
            return Ok(());
        }
        session.logs_revision += 1;
        session.logs = None;
        Ok(())
    }
    async fn watch(&self, expected: u64, kind: &str, version: &str) -> Result<KubeWatch, String> {
        if version.is_empty() || version.len() > 512 {
            return Err("请先刷新列表取得资源版本".into());
        }
        let mut snap = self.snapshot(expected).await?;
        let builder = Request::new(resource_path(kind, &snap.namespace)?);
        let params = WatchParams {
            timeout: Some(5),
            bookmarks: true,
            ..Default::default()
        };
        let request = if kind == "secrets" {
            builder.watch_metadata(&params, version)
        } else {
            builder.watch(&params, version)
        }
        .map_err(|_| "无法构造只读 watch 请求".to_string())?;
        let run = async {
            let reader = match snap.client.request_stream(request).await {
                Ok(reader) => reader,
                Err(kube::Error::Api(response)) if response.code == 410 => {
                    return Ok(KubeWatch {
                        generation: expected,
                        resource_version: version.into(),
                        changes: vec![],
                        expired: true,
                    });
                }
                Err(error) => return Err(api_error(error)),
            };
            futures::pin_mut!(reader);
            let mut result = KubeWatch {
                generation: expected,
                resource_version: version.into(),
                changes: vec![],
                expired: false,
            };
            let mut pending = Vec::new();
            let mut chunk = [0u8; 8192];
            let mut total = 0;
            loop {
                let size = reader
                    .read(&mut chunk)
                    .await
                    .map_err(|_| "watch 响应读取失败".to_string())?;
                if size == 0 {
                    break;
                }
                total += size;
                if total > RESPONSE_LIMIT {
                    return Err("watch 响应超过上限，请刷新".into());
                }
                pending.extend_from_slice(&chunk[..size]);
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let bytes: Vec<u8> = pending.drain(..=end).collect();
                    if bytes.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let event: Value = serde_json::from_slice(&bytes)
                        .map_err(|_| "watch 响应格式无效".to_string())?;
                    if event["type"] == "ERROR" {
                        result.expired = true;
                        return Ok(result);
                    }
                    let object = &event["object"];
                    let rv = text_at(object, "/metadata/resourceVersion");
                    if !rv.is_empty() {
                        result.resource_version = rv;
                    }
                    match event["type"].as_str() {
                        Some(event @ ("ADDED" | "MODIFIED" | "DELETED")) => {
                            result.changes.push(KubeChange {
                                event: event.into(),
                                row: summary(object, kind),
                            })
                        }
                        _ => {}
                    }
                    if result.changes.len() >= 100 {
                        return Ok(result);
                    }
                }
                if pending.len() > DETAIL_LIMIT {
                    return Err("单项 watch 事件超过上限，请刷新".into());
                }
                // Deliver complete batches promptly; resume from the last received resourceVersion.
                if !result.changes.is_empty() {
                    return Ok(result);
                }
            }
            Ok(result)
        };
        let result = tokio::select! {biased; _=snap.cancel.changed()=>Err("watch 已取消".into()), result=tokio::time::timeout(Duration::from_secs(10),run)=>result.map_err(|_|"watch 超时，请刷新".to_string())?}?;
        self.verify(expected).await?;
        Ok(result)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scoped<T: Serialize> {
    pub session_id: String,
    #[serde(flatten)]
    pub data: T,
}
impl<T: Serialize> Scoped<T> {
    fn new(id: &str, data: T) -> Self {
        Self {
            session_id: id.to_owned(),
            data,
        }
    }
}
#[derive(Serialize)]
pub struct KubeAck {
    generation: u64,
    #[serde(rename = "logId", skip_serializing_if = "Option::is_none")]
    log_id: Option<String>,
}
#[derive(Default)]
pub struct KubeRuntimes {
    sessions: Mutex<HashMap<String, Arc<KubeService>>>,
}
impl KubeRuntimes {
    async fn get(&self, id: &str) -> Result<Arc<KubeService>, String> {
        self.sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| "集群工作区已关闭".into())
    }
    pub async fn open_config(
        &self,
        config: Kubeconfig,
        context: Option<&str>,
        namespace: Option<&str>,
    ) -> Result<Scoped<KubeSelection>, String> {
        let id = Uuid::new_v4().to_string();
        let service = Arc::new(KubeService::default());
        {
            let mut sessions = self.sessions.lock().await;
            if sessions.len() >= 4 {
                return Err("最多同时打开4个集群工作区".into());
            }
            sessions.insert(id.clone(), service.clone());
        }
        match service.open_config(config, context, namespace).await {
            Ok(selection) => Ok(Scoped::new(&id, selection)),
            Err(error) => {
                self.sessions.lock().await.remove(&id);
                service.close().await;
                Err(error)
            }
        }
    }
    pub async fn close(&self, id: &str, generation: u64) -> Result<Scoped<KubeAck>, String> {
        let service = self.sessions.lock().await.get(id).cloned();
        let Some(service) = service else {
            return Ok(Scoped::new(
                id,
                KubeAck {
                    generation,
                    log_id: None,
                },
            ));
        };
        let next = {
            let mut hub = service.hub.lock().await;
            if generation > hub.generation {
                return Err("连接代次无效".into());
            }
            hub.session = None;
            hub.generation += 1;
            hub.generation
        };
        self.sessions.lock().await.remove(id);
        Ok(Scoped::new(
            id,
            KubeAck {
                generation: next,
                log_id: None,
            },
        ))
    }
}
#[tauri::command]
pub async fn kube_open(
    service: tauri::State<'_, KubeRuntimes>,
    path: String,
    context: Option<String>,
) -> Result<Scoped<KubeSelection>, String> {
    let imported = tokio::task::spawn_blocking(move || crate::kube_import::import_file(&path))
        .await
        .map_err(|_| "导入任务已停止")??;
    let config = crate::kube_import::decode_snapshot(&imported.material, &imported.settings)?;
    service.open_config(config, context.as_deref(), None).await
}
#[tauri::command]
pub async fn kube_select(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    context: String,
    namespace: Option<String>,
) -> Result<Scoped<KubeSelection>, String> {
    let target = service.get(&session_id).await?;
    let result = target
        .select(generation, &context, namespace.as_deref())
        .await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_namespaces(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
) -> Result<Scoped<KubeNamespaces>, String> {
    let target = service.get(&session_id).await?;
    let result = target.namespaces(generation).await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_list(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    kind: String,
    limit: u32,
    continue_token: Option<String>,
) -> Result<Scoped<KubePage>, String> {
    let target = service.get(&session_id).await?;
    let result = target
        .list(
            generation,
            &kind,
            limit,
            continue_token.as_deref().unwrap_or(""),
        )
        .await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_detail(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    kind: String,
    name: String,
) -> Result<Scoped<KubeDetail>, String> {
    let target = service.get(&session_id).await?;
    let result = target.detail(generation, &kind, &name).await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_watch(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    kind: String,
    resource_version: String,
) -> Result<Scoped<KubeWatch>, String> {
    let target = service.get(&session_id).await?;
    let result = target.watch(generation, &kind, &resource_version).await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_logs_start(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    pod: String,
    container: String,
    follow: bool,
    tail_lines: u32,
) -> Result<Scoped<KubeLogStarted>, String> {
    let target = service.get(&session_id).await?;
    let result = target
        .logs_start(generation, &pod, &container, follow, tail_lines)
        .await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_logs_poll(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    log_id: String,
) -> Result<Scoped<KubeLogPoll>, String> {
    let target = service.get(&session_id).await?;
    let result = target.logs_poll(generation, &log_id).await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_stop(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
) -> Result<Scoped<KubeSelection>, String> {
    let target = service.get(&session_id).await?;
    let result = target.stop(generation).await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_logs_stop(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    log_id: Option<String>,
) -> Result<Scoped<KubeAck>, String> {
    let target = service.get(&session_id).await?;
    target.logs_stop(generation, log_id.as_deref()).await?;
    Ok(Scoped::new(&session_id, KubeAck { generation, log_id }))
}
#[tauri::command]
pub async fn kube_close(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
) -> Result<Scoped<KubeAck>, String> {
    service.close(&session_id, generation).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn paths_are_read_only_resource_allowlist_and_namespace_safe() {
        assert_eq!(
            resource_path("pods", "ops").unwrap(),
            "/api/v1/namespaces/ops/pods"
        );
        assert_eq!(
            resource_path("deployments", "ops").unwrap(),
            "/apis/apps/v1/namespaces/ops/deployments"
        );
        assert_eq!(
            resource_path("persistentvolumes", "ops").unwrap(),
            "/api/v1/persistentvolumes"
        );
        for kind in ["exec", "pods/exec", "nodes", "../secrets", "portforward"] {
            assert!(resource_path(kind, "ops").is_err());
        }
        assert!(resource_path("pods", "../default").is_err());
    }
    #[test]
    fn bounded_page_limit() {
        assert_eq!(page_limit(0), 1);
        assert_eq!(page_limit(5000), 200);
    }
    #[test]
    fn secret_details_never_return_data_annotations_or_managed_fields() {
        let input = json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"fixture","namespace":"ops","annotations":{"last-applied":"fixture-hidden"},"managedFields":[{"data":"fixture-hidden"}]},"data":{"password":"fixture-hidden"},"stringData":{"password":"fixture-hidden"}});
        let safe = safe_resource(input, true);
        assert_eq!(safe["metadata"]["name"], "fixture");
        assert!(safe.get("data").is_none());
        assert!(safe.get("stringData").is_none());
        assert!(!safe.to_string().contains("fixture-hidden"));
    }
    #[test]
    fn external_authentication_plugins_are_refused_before_client_construction() {
        let config:Kubeconfig=serde_yaml::from_str("users:\n- name: untrusted\n  user:\n    exec:\n      command: fixture-never-execute\n      apiVersion: client.authentication.k8s.io/v1\ncontexts: []\nclusters: []\n").unwrap();
        assert!(validate_config(&config).is_err());
    }
    #[test]
    fn context_and_namespace_require_explicit_known_selection() {
        let config:Kubeconfig=serde_yaml::from_str("current-context: first\ncontexts:\n- name: first\n  context: {cluster: c, user: u, namespace: ops}\n- name: second\n  context: {cluster: c, user: u}\nusers: []\nclusters: []\n").unwrap();
        assert_eq!(
            context_selection(&config, None, None).unwrap(),
            ("first".into(), "ops".into())
        );
        assert_eq!(
            context_selection(&config, Some("second"), Some("review")).unwrap(),
            ("second".into(), "review".into())
        );
        assert!(context_selection(&config, Some("missing"), None).is_err());
    }
}
