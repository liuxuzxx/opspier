//! Browser queries use registered resource identities and bounded raw GET bodies.
use super::*;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
const SEARCH_ROWS: usize = 10_000;
const SEARCH_MATCHES: usize = 1_000;
const SEARCH_PAGES: usize = 64;
const SEARCH_BYTES: usize = 32 * 1024 * 1024;
const TOO_LARGE: &str = "Kubernetes响应超过读取上限";
#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDescriptor {
    pub id: String,
    pub group: String,
    pub version: String,
    pub plural: String,
    pub kind: String,
    pub namespaced: bool,
}
impl ResourceDescriptor {
    fn path(&self, namespace: &str) -> Result<String, String> {
        if !namespace.is_empty() && !namespace_valid(namespace) {
            return Err("命名空间格式无效".into());
        }
        let prefix = if self.group.is_empty() {
            format!("/api/{}", self.version)
        } else {
            format!("/apis/{}/{}", self.group, self.version)
        };
        Ok(if self.namespaced && !namespace.is_empty() {
            format!("{prefix}/namespaces/{namespace}/{}", self.plural)
        } else {
            format!("{prefix}/{}", self.plural)
        })
    }
    fn secret(&self) -> bool {
        self.id == "secrets"
            && self.group.is_empty()
            && self.version == "v1"
            && self.plural == "secrets"
    }
}
fn builtins() -> HashMap<String, ResourceDescriptor> {
    let mut out = HashMap::new();
    for (plural, group, version, kind, ns) in [
        ("pods", "", "v1", "Pod", true),
        ("services", "", "v1", "Service", true),
        ("configmaps", "", "v1", "ConfigMap", true),
        ("secrets", "", "v1", "Secret", true),
        (
            "persistentvolumeclaims",
            "",
            "v1",
            "PersistentVolumeClaim",
            true,
        ),
        ("persistentvolumes", "", "v1", "PersistentVolume", false),
        ("events", "", "v1", "Event", true),
        ("deployments", "apps", "v1", "Deployment", true),
        ("statefulsets", "apps", "v1", "StatefulSet", true),
        ("daemonsets", "apps", "v1", "DaemonSet", true),
        ("jobs", "batch", "v1", "Job", true),
        ("cronjobs", "batch", "v1", "CronJob", true),
        (
            "customresourcedefinitions",
            "apiextensions.k8s.io",
            "v1",
            "CustomResourceDefinition",
            false,
        ),
    ] {
        out.insert(
            plural.into(),
            ResourceDescriptor {
                id: plural.into(),
                group: group.into(),
                version: version.into(),
                plural: plural.into(),
                kind: kind.into(),
                namespaced: ns,
            },
        );
    }
    out
}
#[derive(Clone, PartialEq, Eq)]
struct Scope {
    catalog: u64,
    resource: String,
    namespace: String,
    search: String,
}
struct QuerySlot {
    id: u64,
    cancel: watch::Sender<bool>,
}
pub(super) struct BrowserState {
    catalog: HashMap<String, ResourceDescriptor>,
    catalog_revision: u64,
    scope: Option<Scope>,
    scope_revision: u64,
    latest: u64,
    query: Option<QuerySlot>,
    discovery: Option<watch::Sender<bool>>,
}
impl Default for BrowserState {
    fn default() -> Self {
        Self {
            catalog: builtins(),
            catalog_revision: 0,
            scope: None,
            scope_revision: 0,
            latest: 0,
            query: None,
            discovery: None,
        }
    }
}
impl BrowserState {
    pub(super) fn cancel_all(&self) {
        if let Some(q) = &self.query {
            let _ = q.cancel.send(true);
        }
        if let Some(d) = &self.discovery {
            let _ = d.send(true);
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryInput {
    pub request_id: u64,
    pub catalog_revision: u64,
    pub resource_id: String,
    pub namespace: String,
    pub search: String,
    #[serde(default)]
    pub continue_token: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryPage {
    pub generation: u64,
    pub request_id: u64,
    pub catalog_revision: u64,
    pub scope_revision: u64,
    pub resource_id: String,
    pub namespace: String,
    pub search: String,
    pub items: Vec<KubeRow>,
    pub continue_token: String,
    pub resource_version: String,
    pub scanned: usize,
    pub pages: usize,
    pub bytes: usize,
    pub truncated: bool,
    pub reason: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceCatalog {
    pub generation: u64,
    pub catalog_revision: u64,
    pub resources: Vec<ResourceDescriptor>,
    pub warning: Option<String>,
}
fn http_error(code: u16) -> String {
    match code {
        401 => "Kubernetes认证失败（401）".into(),
        403 => "Kubernetes只读权限不足（403）".into(),
        410 => "资源分页已过期（410），请从首页重新加载".into(),
        _ => format!("Kubernetes API请求失败（HTTP {code}）"),
    }
}
pub(super) async fn bounded_json(
    client: &Client,
    path: String,
    params: ListParams,
    secret: bool,
    name: Option<&str>,
    limit: usize,
    transferred: Option<&AtomicUsize>,
) -> Result<(Value, usize), String> {
    let builder = Request::new(path);
    let request = match (name, secret) {
        (Some(name), true) => builder.get_metadata(name, &GetParams::default()),
        (Some(name), false) => builder.get(name, &GetParams::default()),
        (None, true) => builder.list_metadata(&params),
        (None, false) => builder.list(&params),
    }
    .map_err(|_| "无法构造Kubernetes只读请求")?;
    let response = client
        .send(request.map(kube::client::Body::from))
        .await
        .map_err(api_error)?;
    let status = response.status();
    // A known HTTP failure must never become a body-budget partial success.
    if !status.is_success() {
        return Err(http_error(status.as_u16()));
    }
    let reader = response
        .into_body()
        .into_data_stream()
        .map_err(std::io::Error::other)
        .into_async_read();
    futures::pin_mut!(reader);
    let mut bytes = Vec::new();
    let mut limited = reader.take((limit + 1) as u64);
    let mut chunk = [0u8; 8192];
    loop {
        let count = limited
            .read(&mut chunk)
            .await
            .map_err(|_| "Kubernetes响应读取失败")?;
        if count == 0 {
            break;
        }
        if let Some(total) = transferred {
            total.fetch_add(count, Ordering::Relaxed);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    if bytes.len() > limit {
        return Err(TOO_LARGE.into());
    }
    let size = bytes.len();
    let value = serde_json::from_slice(&bytes).map_err(|_| "Kubernetes响应格式无效")?;
    Ok((value, size))
}
fn list_parts(value: &Value, limit: usize) -> Result<(&Vec<Value>, String), String> {
    let items = value["items"].as_array().ok_or("资源列表格式无效")?;
    if items.len() > limit {
        return Err("API返回数量超过请求分页上限".into());
    }
    let token = value
        .pointer("/metadata/continue")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if token.len() > 4096 {
        return Err("分页标记超过上限".into());
    }
    Ok((items, token))
}
fn descriptors(item: &Value) -> Result<Vec<ResourceDescriptor>, String> {
    let spec = &item["spec"];
    let group = spec["group"].as_str().ok_or("invalid")?;
    let plural = spec["names"]["plural"].as_str().ok_or("invalid")?;
    let kind = spec["names"]["kind"].as_str().ok_or("invalid")?;
    let label = |s: &str| {
        !s.is_empty()
            && s.len() <= 63
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !s.starts_with('-')
            && !s.ends_with('-')
    };
    if group.len() > 253
        || !group.split('.').all(label)
        || !label(plural)
        || kind.is_empty()
        || kind.len() > 128
        || kind.chars().any(char::is_control)
    {
        return Err("invalid".into());
    }
    let namespaced = match spec["scope"].as_str() {
        Some("Namespaced") => true,
        Some("Cluster") => false,
        _ => return Err("invalid".into()),
    };
    let mut out = Vec::new();
    for version in spec["versions"].as_array().ok_or("invalid")? {
        if version["served"] != true {
            continue;
        }
        let version = version["name"].as_str().ok_or("invalid")?;
        if !label(version) {
            return Err("invalid".into());
        }
        out.push(ResourceDescriptor {
            id: format!("custom:{group}/{version}/{plural}"),
            group: group.into(),
            version: version.into(),
            plural: plural.into(),
            kind: kind.into(),
            namespaced,
        });
    }
    Ok(out)
}
impl KubeService {
    pub(super) async fn cancel_query(&self, expected: u64, id: u64) -> Result<(), String> {
        let hub = self.hub.lock().await;
        if hub.generation != expected {
            return Err("连接代次已变化".into());
        }
        if let Some(query) = hub
            .session
            .as_ref()
            .and_then(|s| s.browser.query.as_ref())
            .filter(|q| q.id == id)
        {
            let _ = query.cancel.send(true);
        }
        Ok(())
    }
    pub(super) async fn query(
        &self,
        expected: u64,
        request: QueryInput,
    ) -> Result<QueryPage, String> {
        if request.resource_id.len() > 512
            || request.request_id == 0
            || request.request_id > 9_007_199_254_740_991
            || request.continue_token.len() > 4096
            || request.search.chars().count() > 256
        {
            return Err("查询参数超过上限".into());
        }
        if !request.namespace.is_empty() && !namespace_valid(&request.namespace) {
            return Err("命名空间格式无效".into());
        }
        let (client, descriptor, scope, scope_revision, mut cancelled, mut context_cancel) = {
            let mut hub = self.hub.lock().await;
            if hub.generation != expected {
                return Err("连接代次已变化".into());
            }
            let session = hub.session.as_mut().ok_or("集群连接已关闭")?;
            let browser = &mut session.browser;
            if browser.catalog_revision != request.catalog_revision {
                return Err("资源目录已变化，请刷新后重新选择".into());
            }
            let descriptor = browser
                .catalog
                .get(&request.resource_id)
                .cloned()
                .ok_or("不支持的只读资源类型")?;
            if request.request_id <= browser.latest {
                return Err("查询已被更新的请求替代".into());
            }
            let scope = Scope {
                catalog: request.catalog_revision,
                resource: request.resource_id.clone(),
                namespace: if descriptor.namespaced {
                    request.namespace.clone()
                } else {
                    String::new()
                },
                search: request.search.trim().to_owned(),
            };
            if let Some(old) = &browser.query {
                let _ = old.cancel.send(true);
            }
            if browser.scope.as_ref() != Some(&scope) {
                browser.scope_revision += 1;
                browser.scope = Some(scope.clone());
                let _ = session.cancel.send(true);
                let (cancel, _) = watch::channel(false);
                session.cancel = cancel;
                session.logs = None;
                session.logs_revision += 1;
            }
            let (cancel, receiver) = watch::channel(false);
            browser.latest = request.request_id;
            browser.query = Some(QuerySlot {
                id: request.request_id,
                cancel,
            });
            (
                session.client.clone(),
                descriptor,
                scope,
                browser.scope_revision,
                receiver,
                session.cancel.subscribe(),
            )
        };
        let run = async {
            // The old query drops its HTTP future before releasing this gate.
            let _guard = self.query_gate.lock().await;
            self.verify_query(expected, request.request_id, request.catalog_revision)
                .await?;
            let path = descriptor.path(&scope.namespace)?;
            let searching = !scope.search.is_empty();
            let term = scope.search.to_lowercase();
            let mut result = QueryPage {
                generation: expected,
                request_id: request.request_id,
                catalog_revision: scope.catalog,
                scope_revision,
                resource_id: scope.resource.clone(),
                namespace: scope.namespace.clone(),
                search: scope.search.clone(),
                items: vec![],
                continue_token: String::new(),
                resource_version: String::new(),
                scanned: 0,
                pages: 0,
                bytes: 0,
                truncated: false,
                reason: None,
            };
            let transferred = AtomicUsize::new(0);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            let mut token = if searching {
                String::new()
            } else {
                request.continue_token.clone()
            };
            let mut seen = HashSet::new();
            loop {
                let limit = if searching { 200 } else { 100 };
                let remaining = if searching {
                    SEARCH_BYTES - result.bytes
                } else {
                    RESPONSE_LIMIT
                };
                let mut params = ListParams::default().limit(limit);
                if !token.is_empty() {
                    params = params.continue_token(&token);
                }
                let page = tokio::time::timeout_at(
                    deadline,
                    bounded_json(
                        &client,
                        path.clone(),
                        params,
                        descriptor.secret(),
                        None,
                        remaining.min(RESPONSE_LIMIT),
                        Some(&transferred),
                    ),
                )
                .await;
                result.bytes = transferred.load(Ordering::Relaxed);
                let (value, _size) = match page {
                    Ok(Ok(value)) => value,
                    Err(_) if searching => {
                        result.truncated = true;
                        result.reason = Some("搜索达到30秒总时限，结果不完整".into());
                        break;
                    }
                    Err(_) => return Err("Kubernetes请求超时".into()),
                    Ok(Err(error))
                        if searching && remaining < RESPONSE_LIMIT && error == TOO_LARGE =>
                    {
                        result.truncated = true;
                        result.reason = Some("搜索达到32MiB累计传输上限，结果不完整".into());
                        break;
                    }
                    Ok(Err(error)) => return Err(error),
                };
                result.pages += 1;
                let (items, next) = list_parts(&value, limit as usize)?;
                result.resource_version = text_at(&value, "/metadata/resourceVersion");
                for (index, item) in items.iter().enumerate() {
                    if searching
                        && (result.scanned >= SEARCH_ROWS || result.items.len() >= SEARCH_MATCHES)
                    {
                        result.truncated = true;
                        result.reason = Some("搜索达到扫描或匹配条目上限，结果不完整".into());
                        break;
                    }
                    let row = summary(item, &descriptor.plural);
                    if descriptor.namespaced
                        && (!namespace_valid(&row.namespace)
                            || (!scope.namespace.is_empty() && row.namespace != scope.namespace))
                    {
                        return Err("API返回的资源命名空间与查询范围不符".into());
                    }
                    result.scanned += 1;
                    if !searching
                        || format!("{} {} {}", row.name, row.namespace, row.status)
                            .to_lowercase()
                            .contains(&term)
                    {
                        result.items.push(row);
                    }
                    if searching
                        && (result.scanned >= SEARCH_ROWS || result.items.len() >= SEARCH_MATCHES)
                        && (index + 1 < items.len() || !next.is_empty())
                    {
                        result.truncated = true;
                        result.reason = Some("搜索达到扫描或匹配条目上限，结果不完整".into());
                        break;
                    }
                }
                if result.truncated {
                    break;
                }
                if !searching {
                    result.continue_token = next;
                    break;
                }
                if next.is_empty() {
                    break;
                }
                if !seen.insert(next.clone()) {
                    result.truncated = true;
                    result.reason = Some("分页标记循环，搜索结果不完整".into());
                    break;
                }
                if result.pages >= SEARCH_PAGES || result.bytes >= SEARCH_BYTES {
                    result.truncated = true;
                    result.reason = Some("搜索达到页数或累计传输上限，结果不完整".into());
                    break;
                }
                token = next;
            }
            self.verify_query(expected, request.request_id, scope.catalog)
                .await?;
            Ok(result)
        };
        tokio::select! {biased; _=cancelled.changed()=>Err("查询已取消".into()),_=context_cancel.changed()=>Err("查询范围已变化".into()),result=run=>result}
    }
    async fn verify_query(&self, expected: u64, id: u64, catalog: u64) -> Result<(), String> {
        let hub = self.hub.lock().await;
        let session = hub.session.as_ref().ok_or("集群连接已关闭")?;
        if hub.generation != expected
            || session.browser.latest != id
            || session.browser.catalog_revision != catalog
        {
            return Err("查询已被更新的范围替代".into());
        }
        Ok(())
    }
    pub(super) async fn resources(&self, expected: u64) -> Result<ResourceCatalog, String> {
        let (client, mut cancelled) = {
            let mut hub = self.hub.lock().await;
            if hub.generation != expected {
                return Err("连接代次已变化".into());
            }
            let session = hub.session.as_mut().ok_or("集群连接已关闭")?;
            if let Some(old) = &session.browser.discovery {
                let _ = old.send(true);
            }
            let (cancel, receiver) = watch::channel(false);
            session.browser.discovery = Some(cancel);
            (session.client.clone(), receiver)
        };
        let discover = async {
            let _guard = self.catalog_gate.lock().await;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            let mut catalog = builtins();
            let mut token = String::new();
            let mut seen = HashSet::new();
            let mut count = 0;
            let mut versions = 0;
            let mut bytes = 0;
            let mut warning = None;
            for page in 0..32 {
                let remaining = 16 * 1024 * 1024 - bytes;
                if remaining == 0 {
                    warning = Some("CRD目录达到累计传输上限，目录不完整".into());
                    break;
                }
                let mut params = ListParams::default().limit(200);
                if !token.is_empty() {
                    params = params.continue_token(&token);
                }
                let response = tokio::time::timeout_at(
                    deadline,
                    bounded_json(
                        &client,
                        "/apis/apiextensions.k8s.io/v1/customresourcedefinitions".into(),
                        params,
                        false,
                        None,
                        remaining.min(RESPONSE_LIMIT),
                        None,
                    ),
                )
                .await;
                let (value, size) = match response {
                    Err(_) => {
                        warning = Some("CRD发现达到30秒总时限，目录不完整".into());
                        break;
                    }
                    Ok(Err(error)) if error == TOO_LARGE && remaining < RESPONSE_LIMIT => {
                        warning = Some("CRD发现达到16MiB累计传输上限，目录不完整".into());
                        break;
                    }
                    Ok(Err(error)) => return Err(error),
                    Ok(Ok(value)) => value,
                };
                bytes += size;
                let (items, next) = list_parts(&value, 200)?;
                for item in items {
                    if count >= 200 {
                        warning = Some("CRD数量达到200上限，目录不完整".into());
                        break;
                    }
                    count += 1;
                    match descriptors(item) {
                        Ok(entries) => {
                            for entry in entries {
                                if versions >= 400 {
                                    warning = Some("CRD版本达到400上限，目录不完整".into());
                                    break;
                                }
                                if catalog.contains_key(&entry.id) {
                                    warning = Some("CRD目录包含重复身份，已跳过".into());
                                    continue;
                                }
                                catalog.insert(entry.id.clone(), entry);
                                versions += 1;
                            }
                        }
                        Err(_) => warning = Some("部分CRD描述无效，已拒绝登记".into()),
                    }
                }
                if next.is_empty() {
                    break;
                }
                if count >= 200 || versions >= 400 || page == 31 {
                    warning = Some("CRD目录达到发现上限，目录不完整".into());
                    break;
                }
                if !seen.insert(next.clone()) {
                    warning = Some("CRD分页标记循环，目录不完整".into());
                    break;
                }
                token = next;
            }
            Ok::<_, String>((catalog, warning))
        };
        let discovered = tokio::select! {biased; _=cancelled.changed()=>return Err("CRD发现已取消".into()),result=discover=>result};
        let mut hub = self.hub.lock().await;
        if hub.generation != expected {
            return Err("CRD发现范围已变化".into());
        }
        let session = hub.session.as_mut().ok_or("集群连接已关闭")?;
        let warning = match discovered {
            Ok((catalog, warning)) => {
                if session.browser.catalog != catalog {
                    session.browser.catalog = catalog;
                    session.browser.catalog_revision += 1;
                    session.browser.scope = None;
                    session.browser.scope_revision += 1;
                    if let Some(q) = &session.browser.query {
                        let _ = q.cancel.send(true);
                    }
                    let _ = session.cancel.send(true);
                    let (cancel, _) = watch::channel(false);
                    session.cancel = cancel;
                    session.logs = None;
                    session.logs_revision += 1;
                }
                warning
            }
            Err(error) => Some(format!("CRD发现失败，保留已有资源：{error}")),
        };
        let mut resources: Vec<_> = session.browser.catalog.values().cloned().collect();
        resources.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(ResourceCatalog {
            generation: expected,
            catalog_revision: session.browser.catalog_revision,
            resources,
            warning,
        })
    }
    async fn row_descriptor(
        &self,
        expected: u64,
        catalog: u64,
        scope_revision: u64,
        resource: &str,
        namespace: &str,
    ) -> Result<ResourceDescriptor, String> {
        let hub = self.hub.lock().await;
        let session = hub.session.as_ref().ok_or("集群连接已关闭")?;
        let browser = &session.browser;
        if hub.generation != expected
            || browser.catalog_revision != catalog
            || browser.scope_revision != scope_revision
        {
            return Err("资源显示范围已变化，请刷新后重新选择".into());
        }
        let scope = browser.scope.as_ref().ok_or("请先加载资源列表")?;
        if scope.resource != resource {
            return Err("资源类型与显示范围不符".into());
        }
        let descriptor = browser
            .catalog
            .get(resource)
            .cloned()
            .ok_or("资源目录已变化")?;
        if descriptor.namespaced {
            if !namespace_valid(namespace)
                || (!scope.namespace.is_empty() && scope.namespace != namespace)
            {
                return Err("资源命名空间与显示范围不符".into());
            }
        } else if !namespace.is_empty() {
            return Err("集群级资源不能指定命名空间".into());
        }
        Ok(descriptor)
    }
    pub(super) async fn resource_detail(
        &self,
        expected: u64,
        catalog: u64,
        scope_revision: u64,
        resource: &str,
        namespace: &str,
        name: &str,
    ) -> Result<KubeDetail, String> {
        let descriptor = self
            .row_descriptor(expected, catalog, scope_revision, resource, namespace)
            .await?;
        let snap = self.snapshot(expected).await?;
        let value = self
            .json(
                snap,
                descriptor.path(namespace)?,
                descriptor.secret(),
                Some(name),
                ListParams::default(),
            )
            .await?;
        self.row_descriptor(expected, catalog, scope_revision, resource, namespace)
            .await?;
        let containers = if resource == "pods" {
            containers(&value)
        } else {
            vec![]
        };
        let yaml = serde_yaml::to_string(&safe_resource(value, descriptor.secret()))
            .map_err(|_| "YAML转换失败")?;
        if yaml.len() > DETAIL_LIMIT {
            return Err("详情超过2MiB上限".into());
        }
        Ok(KubeDetail {
            generation: expected,
            yaml,
            containers,
        })
    }
    pub(super) async fn resource_logs_start(
        &self,
        expected: u64,
        catalog: u64,
        scope_revision: u64,
        resource: &str,
        namespace: &str,
        pod: &str,
        container: &str,
        follow: bool,
        tail_lines: u32,
    ) -> Result<KubeLogStarted, String> {
        self.row_descriptor(expected, catalog, scope_revision, resource, namespace)
            .await?;
        if resource != "pods" {
            return Err("仅核心Pods支持容器日志".into());
        }
        let result = self
            .logs_start_in(
                expected,
                pod,
                container,
                follow,
                tail_lines,
                Some(namespace),
            )
            .await?;
        if let Err(error) = self
            .row_descriptor(expected, catalog, scope_revision, resource, namespace)
            .await
        {
            let _ = self.logs_stop(expected, Some(&result.log_id)).await;
            return Err(error);
        }
        Ok(result)
    }
}
#[tauri::command]
pub async fn kube_query(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    request: QueryInput,
) -> Result<Scoped<QueryPage>, String> {
    let target = service.get(&session_id).await?;
    let result = target.query(generation, request).await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_cancel_query(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    request_id: u64,
) -> Result<Scoped<KubeAck>, String> {
    service
        .get(&session_id)
        .await?
        .cancel_query(generation, request_id)
        .await?;
    Ok(Scoped::new(
        &session_id,
        KubeAck {
            generation,
            log_id: None,
        },
    ))
}
#[tauri::command]
pub async fn kube_resources(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
) -> Result<Scoped<ResourceCatalog>, String> {
    let result = service
        .get(&session_id)
        .await?
        .resources(generation)
        .await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_resource_detail(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    catalog_revision: u64,
    scope_revision: u64,
    resource_id: String,
    namespace: String,
    name: String,
) -> Result<Scoped<KubeDetail>, String> {
    let result = service
        .get(&session_id)
        .await?
        .resource_detail(
            generation,
            catalog_revision,
            scope_revision,
            &resource_id,
            &namespace,
            &name,
        )
        .await?;
    Ok(Scoped::new(&session_id, result))
}
#[tauri::command]
pub async fn kube_resource_logs_start(
    service: tauri::State<'_, KubeRuntimes>,
    session_id: String,
    generation: u64,
    catalog_revision: u64,
    scope_revision: u64,
    resource_id: String,
    namespace: String,
    pod: String,
    container: String,
    follow: bool,
    tail_lines: u32,
) -> Result<Scoped<KubeLogStarted>, String> {
    let result = service
        .get(&session_id)
        .await?
        .resource_logs_start(
            generation,
            catalog_revision,
            scope_revision,
            &resource_id,
            &namespace,
            &pod,
            &container,
            follow,
            tail_lines,
        )
        .await?;
    Ok(Scoped::new(&session_id, result))
}
