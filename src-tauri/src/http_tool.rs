use futures::StreamExt;
use reqwest::{
    Method,
    header::{HeaderMap, HeaderName, HeaderValue},
    redirect::Policy,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpToolRequest {
    pub method: String,
    pub url: String,
    pub headers: std::collections::HashMap<String, String>,
    pub body: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpToolResponse {
    pub url: String,
    pub status: u16,
    pub status_text: String,
    pub headers: std::collections::HashMap<String, String>,
    pub body: String,
    pub truncated: bool,
    pub elapsed_ms: u64,
}
fn validate_url(value: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(value).map_err(|_| "请求 URL 无效".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || value.len() > 8192
    {
        Err("仅支持有效的 http/https URL".into())
    } else {
        Ok(parsed)
    }
}
#[tauri::command]
pub async fn http_tool_request(request: HttpToolRequest) -> Result<HttpToolResponse, String> {
    let url = validate_url(&request.url)?;
    let method =
        Method::from_bytes(request.method.as_bytes()).map_err(|_| "请求方法无效".to_string())?;
    if !matches!(
        method,
        Method::GET | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return Err("请求方法不受支持".into());
    }
    let mut headers = HeaderMap::new();
    for (name, value) in request.headers {
        let key = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| "请求 Header 名称无效".to_string())?;
        let item = HeaderValue::from_str(&value).map_err(|_| "请求 Header 值无效".to_string())?;
        headers.append(key, item);
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(Policy::limited(10))
        .build()
        .map_err(|_| "HTTP 客户端初始化失败".to_string())?;
    let started = Instant::now();
    let mut builder = client.request(method, url.clone()).headers(headers);
    if let Some(body) = request.body {
        if body.len() > MAX_RESPONSE {
            return Err("请求 Body 超过4MiB上限".into());
        }
        builder = builder.body(body);
    }
    let response = builder.send().await.map_err(|error| {
        if error.is_timeout() {
            String::from("请求超时（10秒）")
        } else {
            String::from("网络请求失败，请检查地址和网络连接")
        }
    })?;
    let final_url = response.url().to_string();
    let status = response.status();
    let status_text = status.canonical_reason().unwrap_or("").to_string();
    let mut output_headers = std::collections::HashMap::new();
    for (name, value) in response.headers() {
        output_headers.insert(
            name.to_string(),
            value.to_str().unwrap_or("[非文本]").to_string(),
        );
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "响应读取失败".to_string())?;
        let remaining = MAX_RESPONSE.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();
    Ok(HttpToolResponse {
        url: final_url,
        status: status.as_u16(),
        status_text,
        headers: output_headers,
        body,
        truncated,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_validation_rejects_non_http_and_accepts_http() {
        assert!(validate_url("file:///tmp/a").is_err());
        assert!(validate_url("https://example.com/a").is_ok());
    }
    #[test]
    fn response_limit_is_bounded() {
        assert_eq!(MAX_RESPONSE, 4 * 1024 * 1024);
    }
}
