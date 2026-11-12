//! S3-compatible connections. Credentials and SDK errors never cross the IPC boundary.
use futures::StreamExt;
use minio::s3::{
    MinioClient, MinioClientBuilder,
    creds::StaticProvider,
    error::{Error, NetworkError, S3ServerError},
    http::BaseUrl,
    types::{BucketName, Region, S3Api, ToStream},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;
#[path = "minio_transfer.rs"]
pub mod transfer;
pub use transfer::TransferView;

const INVALID: &str = "MinIO 配置无效，请检查 API 地址、Region 和桶名称";
const MISSING: &str = "桶或对象不存在（HTTP 404）";
const DENIED: &str = "MinIO 拒绝访问（HTTP 403），请检查密钥与桶/对象权限";
const PAGE_TIMEOUT: Duration = Duration::from_secs(30);
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MinioSettings {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
}
impl MinioSettings {
    pub fn validate(&self) -> Result<(), String> {
        let url = url::Url::parse(&self.endpoint).map_err(|_| INVALID)?;
        if self.endpoint.len() > 2048
            || !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
            || self.endpoint.chars().any(char::is_control)
            || self.region.is_empty()
            || self.region.len() > 128
            || !self
                .region
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || (!self.bucket.is_empty() && BucketName::new(&self.bucket).is_err())
        {
            return Err(INVALID.into());
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MinioMaterial {
    pub access_key: String,
    pub secret_key: String,
}
impl MinioMaterial {
    pub fn validate(&self) -> Result<(), String> {
        if self.access_key.is_empty()
            || self.access_key.len() > 256
            || self.secret_key.is_empty()
            || self.secret_key.len() > 1024
            || self
                .access_key
                .chars()
                .chain(self.secret_key.chars())
                .any(char::is_control)
        {
            return Err("Access Key 或 Secret Key 无效".into());
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| "已保存的 MinIO 密钥格式无效，请重新输入")?;
        value.validate()?;
        Ok(value)
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinioSession {
    pub id: String,
    pub endpoint: String,
    pub bucket: String,
}
struct Runtime {
    client: MinioClient,
    settings: MinioSettings,
}
impl Runtime {
    fn region(&self) -> Result<Region, String> {
        Region::new(&self.settings.region).map_err(|_| INVALID.into())
    }
}
#[derive(Default)]
struct ServiceState {
    sessions: HashMap<String, Arc<Runtime>>,
    jobs: HashMap<String, Arc<transfer::TransferJob>>,
}
#[derive(Default)]
pub struct MinioService {
    state: Mutex<ServiceState>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectEntry {
    pub key: String,
    pub size: u64,
    pub directory: bool,
    pub modified: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectPage {
    pub objects: Vec<ObjectEntry>,
    pub next_cursor: Option<String>,
}
fn runtime(settings: MinioSettings, material: MinioMaterial) -> Result<Arc<Runtime>, String> {
    settings.validate()?;
    material.validate()?;
    let base: BaseUrl = settings.endpoint.parse().map_err(|_| INVALID)?;
    let client = MinioClientBuilder::new(base)
        .provider(Some(StaticProvider::new(
            &material.access_key,
            &material.secret_key,
            None,
        )))
        .skip_region_lookup(true)
        .app_info(Some(("OpsPier".into(), env!("CARGO_PKG_VERSION").into())))
        .build()
        .map_err(sdk_error)?;
    Ok(Arc::new(Runtime { client, settings }))
}
async fn bounded<T>(
    future: impl std::future::Future<Output = Result<T, Error>>,
) -> Result<T, String> {
    tokio::time::timeout(PAGE_TIMEOUT, future)
        .await
        .map_err(|_| "MinIO 请求超时（30秒），请检查网络或服务状态".to_string())?
        .map_err(sdk_error)
}
fn sdk_error(error: Error) -> String {
    match &error {
        Error::S3Server(S3ServerError::S3Error(e)) => {
            match e.code().to_string().to_ascii_lowercase().as_str() {
                "accessdenied" | "invalidaccesskeyid" => DENIED.into(),
                "signaturedoesnotmatch" => {
                    "MinIO 签名校验失败，请检查密钥、Region 和本机时间".into()
                }
                "requesttimetoolskewed" | "requesttimetooskewed" => {
                    "MinIO 请求时间偏差过大，请同步本机时间".into()
                }
                "nosuchbucket" | "nosuchkey" => MISSING.into(),
                "preconditionfailed" | "conditionalrequestconflict" => "OBJECT_EXISTS".into(),
                _ => "MinIO 服务返回错误，请检查服务状态或服务端日志".into(),
            }
        }
        Error::S3Server(S3ServerError::HttpError(401 | 403, _)) => DENIED.into(),
        Error::S3Server(S3ServerError::HttpError(404, _)) => MISSING.into(),
        Error::S3Server(S3ServerError::HttpError(409 | 412, _)) => "OBJECT_EXISTS".into(),
        Error::Network(NetworkError::ReqwestError(e)) if e.is_timeout() => {
            "MinIO 网络请求超时".into()
        }
        Error::Network(_) => "MinIO 网络连接失败，请检查 API 地址、端口、TLS 证书及网络".into(),
        Error::DriveIo(_) => "无法读取或写入本地文件，请检查路径和权限".into(),
        Error::Validation(_) => {
            "MinIO 请求或响应无效，请检查桶名、对象名、Region 和 TLS 证书".into()
        }
        _ => "MinIO 服务响应异常，请确认使用 S3 API 端口而非控制台端口".into(),
    }
}
async fn object_page(
    runtime: &Runtime,
    bucket: &str,
    prefix: &str,
    cursor: Option<String>,
) -> Result<ObjectPage, String> {
    if prefix.len() > 1024
        || prefix.chars().any(char::is_control)
        || cursor.as_ref().is_some_and(|c| c.len() > 8192)
    {
        return Err("对象前缀或分页游标无效".into());
    }
    let request = runtime
        .client
        .list_objects(bucket)
        .map_err(|_| INVALID)?
        .region(Some(runtime.region()?))
        .prefix(Some(prefix.to_owned()))
        .delimiter(Some("/".into()))
        .max_keys(Some(200))
        .continuation_token(cursor)
        .build();
    let result = tokio::time::timeout(PAGE_TIMEOUT, async {
        request.to_stream().await.next().await
    })
    .await
    .map_err(|_| "MinIO 对象列表请求超时")?
    .ok_or("MinIO 未返回对象列表")?
    .map_err(sdk_error)?;
    if result.contents.len() > 200 {
        return Err("MinIO 返回的对象数超过单页上限".into());
    }
    if result.is_truncated && result.next_continuation_token.is_none() {
        return Err("MinIO 返回的分页游标缺失，请重新刷新".into());
    }
    Ok(ObjectPage {
        objects: result
            .contents
            .into_iter()
            .map(|entry| ObjectEntry {
                key: entry.name,
                size: entry.size.unwrap_or(0),
                directory: entry.is_prefix,
                modified: entry.last_modified.map(|d| d.to_rfc3339()),
            })
            .collect(),
        next_cursor: if result.is_truncated {
            result.next_continuation_token
        } else {
            None
        },
    })
}
async fn list_buckets(runtime: &Runtime) -> Result<Vec<String>, String> {
    let response = bounded(runtime.client.list_buckets().build().send()).await?;
    let buckets = response.buckets().map_err(|_| "MinIO 桶列表格式无效")?;
    if buckets.len() > 10000 {
        return Err("桶数量超过10000，请填写默认桶后连接".into());
    }
    Ok(buckets.into_iter().map(|b| b.name.to_string()).collect())
}
pub async fn test_connection(
    settings: MinioSettings,
    material: MinioMaterial,
) -> Result<(), String> {
    let runtime = runtime(settings, material)?;
    if runtime.settings.bucket.is_empty() {
        list_buckets(&runtime).await?;
    } else {
        object_page(&runtime, &runtime.settings.bucket, "", None).await?;
    }
    Ok(())
}
impl MinioService {
    fn get(&self, id: &str) -> Result<Arc<Runtime>, String> {
        self.state
            .lock()
            .map_err(|_| "MinIO 状态不可用")?
            .sessions
            .get(id)
            .cloned()
            .ok_or("MinIO 连接已关闭".into())
    }
    pub async fn connect(
        &self,
        settings: MinioSettings,
        material: MinioMaterial,
    ) -> Result<MinioSession, String> {
        let runtime = runtime(settings, material)?;
        if runtime.settings.bucket.is_empty() {
            list_buckets(&runtime).await?;
        } else {
            object_page(&runtime, &runtime.settings.bucket, "", None).await?;
        }
        let mut state = self.state.lock().map_err(|_| "MinIO 状态不可用")?;
        if state.sessions.len() >= 16 {
            return Err("最多同时打开16个 MinIO 连接".into());
        }
        let session = MinioSession {
            id: Uuid::new_v4().to_string(),
            endpoint: runtime.settings.endpoint.clone(),
            bucket: runtime.settings.bucket.clone(),
        };
        state.sessions.insert(session.id.clone(), runtime);
        Ok(session)
    }
    pub async fn buckets(&self, id: &str) -> Result<Vec<String>, String> {
        let runtime = self.get(id)?;
        match list_buckets(&runtime).await {
            Ok(items) => Ok(items),
            Err(e) if e == DENIED && !runtime.settings.bucket.is_empty() => {
                Ok(vec![runtime.settings.bucket.clone()])
            }
            Err(e) => Err(e),
        }
    }
    pub async fn objects(
        &self,
        id: &str,
        bucket: &str,
        prefix: &str,
        cursor: Option<String>,
    ) -> Result<ObjectPage, String> {
        {
            let runtime = self.get(id)?;
            object_page(&runtime, bucket, prefix, cursor).await
        }
    }
    pub async fn delete(&self, id: &str, bucket: &str, key: &str) -> Result<(), String> {
        if key.is_empty() || key.ends_with('/') {
            return Err("仅支持删除单个文件对象".into());
        }
        let runtime = self.get(id)?;
        bounded(
            runtime
                .client
                .delete_object(bucket, key)
                .map_err(|_| INVALID)?
                .region(Some(runtime.region()?))
                .build()
                .send(),
        )
        .await?;
        Ok(())
    }
    pub fn close(&self, id: &str) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "MinIO 状态不可用")?;
        if state
            .jobs
            .values()
            .any(|j| j.session_id == id && j.running())
        {
            return Err("仍有文件正在传输，请完成或取消传输后关闭连接".into());
        }
        state.sessions.remove(id);
        state.jobs.retain(|_, j| j.session_id != id);
        Ok(())
    }
}
#[tauri::command]
pub async fn minio_buckets(
    service: tauri::State<'_, MinioService>,
    session_id: String,
) -> Result<Vec<String>, String> {
    service.buckets(&session_id).await
}
#[tauri::command]
pub async fn minio_objects(
    service: tauri::State<'_, MinioService>,
    session_id: String,
    bucket: String,
    prefix: String,
    cursor: Option<String>,
) -> Result<ObjectPage, String> {
    service.objects(&session_id, &bucket, &prefix, cursor).await
}
#[tauri::command]
pub async fn minio_delete(
    service: tauri::State<'_, MinioService>,
    session_id: String,
    bucket: String,
    key: String,
) -> Result<(), String> {
    service.delete(&session_id, &bucket, &key).await
}
#[tauri::command]
pub fn minio_close(
    service: tauri::State<'_, MinioService>,
    session_id: String,
) -> Result<(), String> {
    service.close(&session_id)
}
#[cfg(test)]
#[path = "minio_tests.rs"]
mod tests;
