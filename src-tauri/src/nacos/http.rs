use super::*;
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::time::Duration;
pub(super) const MAX_BODY: usize = 4 * 1024 * 1024;
const AUTH: &str = "Nacos 认证失败或登录已失效，请核对账号密码后重新连接";
const FORBIDDEN: &str = "Nacos 权限不足（HTTP 403），请检查当前命名空间的读写权限";
#[derive(Clone)]
pub(super) struct Reply {
    pub status: StatusCode,
    pub bytes: Vec<u8>,
}
impl Reply {
    pub fn json(&self) -> Result<Value, String> {
        serde_json::from_slice(&self.bytes)
            .map_err(|_| "Nacos 响应格式异常，请确认 API 地址和版本".into())
    }
    pub fn success(&self, write: bool) -> Result<(), String> {
        if self.status.is_success() {
            return Ok(());
        }
        if write
            && serde_json::from_slice::<Value>(&self.bytes)
                .ok()
                .and_then(|v| v.get("errorCode").and_then(Value::as_u64))
                == Some(30003)
        {
            return Err(CONFLICT.into());
        }
        Err(match self.status.as_u16() {
            401 => AUTH.into(),
            403 => FORBIDDEN.into(),
            404 => "目标不存在（HTTP 404）".into(),
            409 | 412 => CONFLICT.into(),
            400 => "Nacos 拒绝请求参数（HTTP 400），请检查配置或实例字段".into(),
            code if code >= 500 && write => UNKNOWN.into(),
            code => format!("Nacos 请求失败（HTTP {code}）"),
        })
    }
}
impl Runtime {
    pub fn new(settings: NacosSettings, material: Option<NacosMaterial>) -> Result<Self, String> {
        settings.validate()?;
        if settings.authentication == "password" {
            material
                .as_ref()
                .ok_or("此 Nacos 连接尚未保存账号密码")?
                .validate()?;
        }
        if settings.authentication == "none" && material.is_some() {
            return Err("未启用认证的连接不应携带账号密码".into());
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "无法初始化 Nacos HTTP 客户端")?;
        Ok(Self {
            settings,
            client,
            material,
            token: AsyncMutex::new(None),
            closed: AtomicBool::new(false),
            writes: AtomicUsize::new(0),
            mutation: AsyncMutex::new(()),
        })
    }
    async fn send(
        &self,
        method: Method,
        path: &str,
        params: &[(String, String)],
        token: Option<&str>,
        cas: Option<&str>,
    ) -> Result<Reply, String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("Nacos 连接已关闭".into());
        }
        let write = matches!(method, Method::POST | Method::PUT | Method::DELETE)
            && !path.ends_with("/login");
        let url = format!("{}{}", self.settings.endpoint.trim_end_matches('/'), path);
        let mut request = self.client.request(method.clone(), url);
        if method == Method::GET || method == Method::DELETE {
            request = request.query(params);
        } else {
            request = request.form(params);
        }
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(md5) = cas {
            request = request.header("casMd5", md5);
        }
        let mut response = request.send().await.map_err(|error| {
            if write {
                UNKNOWN.into()
            } else if error.is_timeout() {
                "Nacos 请求超时（15秒）".into()
            } else {
                "Nacos 网络连接失败，请检查地址、端口及 TLS 证书".to_string()
            }
        })?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
        {
            return Err("Nacos 响应超过4MiB上限".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| if write { UNKNOWN } else { "Nacos 响应中断" })?
        {
            if bytes.len() + chunk.len() > MAX_BODY {
                return Err("Nacos 响应超过4MiB上限".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err("Nacos 连接已关闭".into());
        }
        Ok(Reply { status, bytes })
    }
    async fn access_token(&self) -> Result<Option<String>, String> {
        if self.settings.authentication == "none" {
            return Ok(None);
        }
        let mut saved = self.token.lock().await;
        if let Some(value) = saved.as_ref() {
            if value.expires > Instant::now() {
                return Ok(Some(value.value.clone()));
            }
        }
        let material = self.material.as_ref().ok_or(AUTH)?;
        let reply = self
            .send(
                Method::POST,
                "/v1/auth/users/login",
                &[
                    ("username".into(), material.username.clone()),
                    ("password".into(), material.password.clone()),
                ],
                None,
                None,
            )
            .await?;
        if !reply.status.is_success() {
            return Err(AUTH.into());
        }
        let value = reply.json()?;
        let token = value
            .get("accessToken")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty() && t.len() <= 16384)
            .ok_or(AUTH)?;
        let ttl = value
            .get("tokenTtl")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .ok_or(AUTH)?
            .min(86400);
        let margin = (ttl / 10).min(30);
        *saved = Some(AuthToken {
            value: token.into(),
            expires: Instant::now() + Duration::from_secs(ttl - margin),
        });
        Ok(Some(token.into()))
    }
    pub(super) async fn request(
        &self,
        method: Method,
        path: &str,
        params: &[(String, String)],
        cas: Option<&str>,
    ) -> Result<Reply, String> {
        let token = self.access_token().await?;
        let reply = self
            .send(method.clone(), path, params, token.as_deref(), cas)
            .await?;
        let expired = reply.status == StatusCode::UNAUTHORIZED
            || reply.status == StatusCode::FORBIDDEN
                && String::from_utf8_lossy(&reply.bytes)
                    .to_ascii_lowercase()
                    .contains("token expired");
        if expired && token.is_some() {
            {
                let mut current = self.token.lock().await;
                if current.as_ref().map(|v| &v.value) == token.as_ref() {
                    *current = None;
                }
            }
            if method == Method::GET {
                let refreshed = self.access_token().await?;
                let retried = self
                    .send(method, path, params, refreshed.as_deref(), cas)
                    .await?;
                retried.success(false)?;
                return Ok(retried);
            }
            return Err(AUTH.into());
        }
        // Missing objects are interpreted by scope-specific APIs, never by an error body.
        if reply.status != StatusCode::NOT_FOUND {
            reply.success(method != Method::GET)?;
        }
        Ok(reply)
    }
    pub async fn probe(&self) -> Result<String, String> {
        let state = self
            .send(Method::GET, "/v1/console/server/state", &[], None, None)
            .await?;
        state.success(false)?;
        let json = state.json()?;
        let version = json
            .get("version")
            .and_then(Value::as_str)
            .ok_or("无法识别 Nacos 版本")?;
        if !version.starts_with("2.") {
            return Err("本版以 Nacos 2.x 为接口基线，请检查服务器版本".into());
        }
        self.namespaces().await?;
        Ok(version.into())
    }
    pub async fn namespaces(&self) -> Result<Vec<Namespace>, String> {
        let reply = self
            .request(Method::GET, "/v1/console/namespaces", &[], None)
            .await?;
        reply.success(false)?;
        let value = reply.json()?;
        let rows = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or("Nacos 命名空间列表格式异常")?;
        if rows.len() > 1000 {
            return Err("命名空间数量超过1000上限".into());
        }
        rows.iter()
            .map(|row| {
                let id = row
                    .get("namespace")
                    .and_then(Value::as_str)
                    .ok_or("命名空间ID缺失")?;
                let name = row
                    .get("namespaceShowName")
                    .and_then(Value::as_str)
                    .unwrap_or(id);
                Ok(Namespace {
                    id: namespace_id(id),
                    name: if id.is_empty() {
                        "public".into()
                    } else {
                        name.into()
                    },
                })
            })
            .collect()
    }
}
