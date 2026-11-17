use super::{CONFLICT, NacosService, UNKNOWN, namespace_id, text};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const CONFIG: &str = "/v1/cs/configs";
const HISTORY: &str = "/v1/cs/history";
const BAD: &str = "Nacos 配置目标、分页或内容格式无效";
const CONTENT_LIMIT: usize = 1024 * 1024;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigKey {
    pub namespace_id: String,
    pub group: String,
    pub data_id: String,
}
impl ConfigKey {
    pub(super) fn checked(mut self) -> Result<Self, String> {
        self.namespace_id = namespace_id(&self.namespace_id);
        if !text(&self.namespace_id, 256, true)
            || !text(&self.group, 256, false)
            || !text(&self.data_id, 256, false)
        {
            return Err(BAD.into());
        }
        Ok(self)
    }
    fn params(&self) -> Vec<(String, String)> {
        vec![
            ("tenant".into(), self.namespace_id.clone()),
            ("group".into(), self.group.clone()),
            ("dataId".into(), self.data_id.clone()),
        ]
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T: Serialize> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub key: ConfigKey,
    pub content: String,
    pub md5: String,
    #[serde(rename = "type")]
    pub config_type: String,
    #[serde(skip)]
    attributes: BTreeMap<String, String>,
}
#[derive(Serialize)]
pub struct ConfigItem {
    pub key: ConfigKey,
    #[serde(rename = "type")]
    pub config_type: String,
    pub md5: String,
}
#[derive(Serialize)]
pub struct ConfigHistory {
    pub id: String,
    pub modified: String,
    pub operation: String,
}
pub(super) fn pagination(page: u32, size: u32) -> Result<(), String> {
    if page == 0 || page > 1_000_000 || size == 0 || size > 200 {
        Err(BAD.into())
    } else {
        Ok(())
    }
}
fn value_text(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}
fn verify_target(row: &Value, key: &ConfigKey) -> Result<(), String> {
    let received = ConfigKey {
        namespace_id: namespace_id(&value_text(row, "tenant")),
        group: value_text(row, "group"),
        data_id: value_text(row, "dataId"),
    };
    if &received != key {
        return Err("Nacos 返回的配置目标不一致，已拒绝使用".into());
    }
    Ok(())
}
fn view(row: &Value, key: &ConfigKey) -> Result<ConfigView, String> {
    verify_target(row, key)?;
    let content = row
        .get("content")
        .and_then(Value::as_str)
        .ok_or("配置内容缺失")?
        .to_owned();
    if content.len() > CONTENT_LIMIT {
        return Err("配置内容超过1MiB上限".into());
    }
    let md5 = value_text(row, "md5");
    if md5.len() != 32 || !md5.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("配置版本标识无效".into());
    }
    let mut attributes = BTreeMap::new();
    for (field, param) in [
        ("appName", "appName"),
        ("desc", "desc"),
        ("use", "use"),
        ("effect", "effect"),
        ("schema", "schema"),
        ("configTags", "config_tags"),
    ] {
        if let Some(value) = row.get(field).and_then(Value::as_str) {
            attributes.insert(param.into(), value.into());
        }
    }
    let kind = row
        .get("type")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .unwrap_or("text");
    Ok(ConfigView {
        key: key.clone(),
        content,
        md5,
        config_type: kind.into(),
        attributes,
    })
}
impl NacosService {
    pub async fn configs(
        &self,
        id: &str,
        namespace: &str,
        group: &str,
        data_id: &str,
        page: u32,
        page_size: u32,
    ) -> Result<Page<ConfigItem>, String> {
        super::deadline(false, async {
            pagination(page, page_size)?;
            if !text(namespace, 256, true) || !text(group, 256, true) || !text(data_id, 256, true) {
                return Err(BAD.into());
            }
            let runtime = self.runtime(id)?;
            let params = vec![
                ("tenant".into(), namespace_id(namespace)),
                ("search".into(), "blur".into()),
                ("group".into(), group.into()),
                ("dataId".into(), data_id.into()),
                ("pageNo".into(), page.to_string()),
                ("pageSize".into(), page_size.to_string()),
            ];
            let reply = runtime.request(Method::GET, CONFIG, &params, None).await?;
            reply.success(false)?;
            let data = reply.json()?;
            let rows = data
                .get("pageItems")
                .and_then(Value::as_array)
                .ok_or("配置列表格式异常")?;
            if rows.len() > page_size as usize {
                return Err("配置列表超过请求分页上限".into());
            }
            let mut items = Vec::new();
            for row in rows {
                let key = ConfigKey {
                    namespace_id: namespace_id(namespace),
                    group: value_text(row, "group"),
                    data_id: value_text(row, "dataId"),
                }
                .checked()?;
                if row
                    .get("tenant")
                    .and_then(Value::as_str)
                    .is_some_and(|v| namespace_id(v) != key.namespace_id)
                {
                    return Err("配置列表返回其他命名空间".into());
                }
                items.push(ConfigItem {
                    key,
                    config_type: row
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("text")
                        .into(),
                    md5: value_text(row, "md5"),
                });
            }
            Ok(Page {
                items,
                total: data
                    .get("totalCount")
                    .and_then(Value::as_u64)
                    .ok_or("配置总数缺失")?,
                page,
                page_size,
            })
        })
        .await
    }
    pub async fn config(&self, id: &str, key: ConfigKey) -> Result<Option<ConfigView>, String> {
        super::deadline(false, async {
            let key = key.checked()?;
            let runtime = self.runtime(id)?;
            let mut params = key.params();
            params.push(("show".into(), "all".into()));
            let reply = runtime.request(Method::GET, CONFIG, &params, None).await?;
            if reply.status == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            reply.success(false)?;
            if reply.bytes.is_empty() {
                return Ok(None);
            }
            let value = reply.json()?;
            if value.is_null() {
                Ok(None)
            } else {
                view(&value, &key).map(Some)
            }
        })
        .await
    }
    pub async fn publish(
        &self,
        id: &str,
        key: ConfigKey,
        content: String,
        config_type: String,
        expected_md5: Option<String>,
    ) -> Result<ConfigView, String> {
        super::deadline(true, async {
            let key = key.checked()?;
            if content.len() > CONTENT_LIMIT
                || !matches!(
                    config_type.as_str(),
                    "text" | "json" | "xml" | "yaml" | "html" | "properties" | "toml"
                )
            {
                return Err(BAD.into());
            }
            let guard = self.begin_write(id)?;
            let _serial = guard.runtime.mutation.lock().await;
            let current = self.config(id, key.clone()).await?;
            if current.as_ref().map(|c| &c.md5) != expected_md5.as_ref() {
                return Err(CONFLICT.into());
            }
            let mut params = key.params();
            params.extend([
                ("content".into(), content.clone()),
                ("type".into(), config_type.clone()),
            ]);
            if let Some(old) = &current {
                params.extend(old.attributes.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            let reply = guard
                .runtime
                .request(Method::POST, CONFIG, &params, expected_md5.as_deref())
                .await?;
            reply.success(true)?;
            if reply.json().map_err(|_| UNKNOWN)?.as_bool() != Some(true) {
                return Err(UNKNOWN.into());
            }
            let result = self
                .config(id, key)
                .await
                .map_err(|_| UNKNOWN)?
                .ok_or(UNKNOWN)?;
            if result.content != content
                || result.config_type != config_type
                || current.as_ref().is_some_and(|old| {
                    old.attributes
                        .iter()
                        .any(|(k, v)| result.attributes.get(k) != Some(v))
                })
            {
                return Err(UNKNOWN.into());
            }
            Ok(result)
        })
        .await
    }
    pub async fn delete_config(
        &self,
        id: &str,
        key: ConfigKey,
        expected_md5: String,
    ) -> Result<(), String> {
        super::deadline(true, async {
            let key = key.checked()?;
            let guard = self.begin_write(id)?;
            let _serial = guard.runtime.mutation.lock().await;
            let old = self.config(id, key.clone()).await?.ok_or(CONFLICT)?;
            if old.md5 != expected_md5 {
                return Err(CONFLICT.into());
            }
            let reply = guard
                .runtime
                .request(Method::DELETE, CONFIG, &key.params(), None)
                .await?;
            reply.success(true)?;
            if reply.json().map_err(|_| UNKNOWN)?.as_bool() != Some(true)
                || self.config(id, key).await.map_err(|_| UNKNOWN)?.is_some()
            {
                return Err(UNKNOWN.into());
            }
            Ok(())
        })
        .await
    }
    pub async fn history(
        &self,
        id: &str,
        key: ConfigKey,
        page: u32,
        page_size: u32,
    ) -> Result<Page<ConfigHistory>, String> {
        super::deadline(false, async {
            let key = key.checked()?;
            pagination(page, page_size)?;
            let mut params = key.params();
            params.extend([
                ("search".into(), "accurate".into()),
                ("pageNo".into(), page.to_string()),
                ("pageSize".into(), page_size.to_string()),
            ]);
            let reply = self
                .runtime(id)?
                .request(Method::GET, HISTORY, &params, None)
                .await?;
            reply.success(false)?;
            let value = reply.json()?;
            let rows = value
                .get("pageItems")
                .and_then(Value::as_array)
                .ok_or("配置历史格式异常")?;
            if rows.len() > page_size as usize {
                return Err("配置历史超过请求分页上限".into());
            }
            let mut items = Vec::new();
            for row in rows {
                verify_target(row, &key)?;
                let id = row
                    .get("id")
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .ok_or("配置历史ID缺失")?;
                if id.is_empty() || id.len() > 32 || !id.bytes().all(|c| c.is_ascii_digit()) {
                    return Err("配置历史ID无效".into());
                }
                let modified = row
                    .get("lastModifiedTime")
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .unwrap_or_default();
                items.push(ConfigHistory {
                    id,
                    modified,
                    operation: value_text(row, "opType"),
                });
            }
            Ok(Page {
                items,
                total: value
                    .get("totalCount")
                    .and_then(Value::as_u64)
                    .ok_or("配置历史总数缺失")?,
                page,
                page_size,
            })
        })
        .await
    }
    pub async fn history_config(
        &self,
        id: &str,
        key: ConfigKey,
        history_id: String,
    ) -> Result<ConfigView, String> {
        super::deadline(false, async {
            let key = key.checked()?;
            if history_id.is_empty()
                || history_id.len() > 32
                || !history_id.bytes().all(|c| c.is_ascii_digit())
            {
                return Err("配置历史ID无效".into());
            }
            let mut params = key.params();
            params.push(("nid".into(), history_id));
            let reply = self
                .runtime(id)?
                .request(Method::GET, HISTORY, &params, None)
                .await?;
            reply.success(false)?;
            view(&reply.json()?, &key)
        })
        .await
    }
    pub async fn rollback(
        &self,
        id: &str,
        key: ConfigKey,
        history_id: String,
        expected_md5: String,
    ) -> Result<ConfigView, String> {
        super::deadline(true, async {
            let old = self.history_config(id, key.clone(), history_id).await?;
            let current = self.config(id, key.clone()).await?.ok_or(CONFLICT)?;
            self.publish(
                id,
                key,
                old.content,
                current.config_type,
                Some(expected_md5),
            )
            .await
        })
        .await
    }
}
#[tauri::command]
pub async fn nacos_configs(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    namespace: String,
    group: String,
    data_id: String,
    page: u32,
    page_size: u32,
) -> Result<Page<ConfigItem>, String> {
    service
        .configs(&session_id, &namespace, &group, &data_id, page, page_size)
        .await
}
#[tauri::command]
pub async fn nacos_config(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    key: ConfigKey,
) -> Result<Option<ConfigView>, String> {
    service.config(&session_id, key).await
}
#[tauri::command]
pub async fn nacos_publish(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    key: ConfigKey,
    content: String,
    config_type: String,
    expected_md5: Option<String>,
) -> Result<ConfigView, String> {
    service
        .publish(&session_id, key, content, config_type, expected_md5)
        .await
}
#[tauri::command]
pub async fn nacos_delete_config(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    key: ConfigKey,
    expected_md5: String,
) -> Result<(), String> {
    service.delete_config(&session_id, key, expected_md5).await
}
#[tauri::command]
pub async fn nacos_history(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    key: ConfigKey,
    page: u32,
    page_size: u32,
) -> Result<Page<ConfigHistory>, String> {
    service.history(&session_id, key, page, page_size).await
}
#[tauri::command]
pub async fn nacos_history_config(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    key: ConfigKey,
    history_id: String,
) -> Result<ConfigView, String> {
    service.history_config(&session_id, key, history_id).await
}
#[tauri::command]
pub async fn nacos_rollback(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    key: ConfigKey,
    history_id: String,
    expected_md5: String,
) -> Result<ConfigView, String> {
    service
        .rollback(&session_id, key, history_id, expected_md5)
        .await
}
