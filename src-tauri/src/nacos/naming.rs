use super::config::{Page, pagination};
use super::{CONFLICT, NacosService, UNKNOWN, namespace_id, naming_namespace, text};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
const BAD: &str = "Nacos 服务或实例目标/属性无效";
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceKey {
    pub namespace_id: String,
    pub group: String,
    pub name: String,
}
impl ServiceKey {
    fn checked(mut self) -> Result<Self, String> {
        self.namespace_id = namespace_id(&self.namespace_id);
        if !text(&self.namespace_id, 256, true)
            || !text(&self.group, 256, false)
            || !text(&self.name, 256, false)
            || self.group.contains("@@")
            || self.name.contains("@@")
        {
            return Err(BAD.into());
        }
        Ok(self)
    }
    fn params(&self) -> Vec<(String, String)> {
        vec![
            ("namespaceId".into(), naming_namespace(&self.namespace_id)),
            (
                "serviceName".into(),
                format!("{}@@{}", self.group, self.name),
            ),
        ]
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstanceKey {
    pub ip: String,
    pub port: u16,
    pub cluster: String,
    pub ephemeral: bool,
}
impl InstanceKey {
    fn check(&self) -> Result<(), String> {
        if !text(&self.ip, 256, false) || self.port == 0 || !text(&self.cluster, 256, false) {
            Err(BAD.into())
        } else {
            Ok(())
        }
    }
    fn params(&self) -> Vec<(String, String)> {
        vec![
            ("ip".into(), self.ip.clone()),
            ("port".into(), self.port.to_string()),
            ("clusterName".into(), self.cluster.clone()),
            ("ephemeral".into(), self.ephemeral.to_string()),
        ]
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceItem {
    pub key: ServiceKey,
    pub instance_count: u64,
    pub healthy_count: u64,
    pub cluster_count: u64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NacosInstance {
    pub key: InstanceKey,
    pub healthy: bool,
    pub enabled: bool,
    pub weight: f64,
    pub metadata: BTreeMap<String, String>,
    pub snapshot: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    pub key: ServiceKey,
    pub protect_threshold: f64,
    pub metadata: BTreeMap<String, String>,
    pub selector: Value,
    pub snapshot: String,
    pub instances: Vec<NacosInstance>,
}
fn canonical(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            serde_json::to_value(sorted).unwrap_or(Value::Null)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(canonical).collect()),
        other => other,
    }
}
fn fingerprint(v: Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(v)).unwrap_or_default())
    )
}
fn metadata(v: Option<&Value>) -> Result<BTreeMap<String, String>, String> {
    let result: BTreeMap<String, String> = match v {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(v) => {
            serde_json::from_value(v.clone()).map_err(|_| "Nacos 元数据必须为字符串键值对象")?
        }
    };
    check_metadata(&result)?;
    Ok(result)
}
fn check_metadata(v: &BTreeMap<String, String>) -> Result<(), String> {
    if v.len() > 256
        || serde_json::to_vec(v).map_err(|_| BAD)?.len() > 65536
        || v.iter()
            .any(|(k, val)| !text(k, 1024, false) || val.len() > 65536)
    {
        Err("元数据超过256项或64KiB上限，或键无效".into())
    } else {
        Ok(())
    }
}
fn instance(row: &Value) -> Result<NacosInstance, String> {
    let key = InstanceKey {
        ip: row.get("ip").and_then(Value::as_str).ok_or(BAD)?.into(),
        port: row
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|n| u16::try_from(n).ok())
            .ok_or(BAD)?,
        cluster: row
            .get("clusterName")
            .and_then(Value::as_str)
            .unwrap_or("DEFAULT")
            .into(),
        ephemeral: row.get("ephemeral").and_then(Value::as_bool).ok_or(BAD)?,
    };
    key.check()?;
    let weight = row
        .get("weight")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 10000.0)
        .ok_or(BAD)?;
    let enabled = row.get("enabled").and_then(Value::as_bool).ok_or(BAD)?;
    let healthy = row.get("healthy").and_then(Value::as_bool).unwrap_or(false);
    let metadata = metadata(row.get("metadata"))?;
    let snapshot = fingerprint(
        serde_json::json!({"key":key,"enabled":enabled,"weight":weight,"metadata":metadata}),
    );
    Ok(NacosInstance {
        key,
        healthy,
        enabled,
        weight,
        metadata,
        snapshot,
    })
}
impl NacosService {
    pub async fn services(
        &self,
        id: &str,
        namespace: &str,
        group: &str,
        name: &str,
        page: u32,
        page_size: u32,
    ) -> Result<Page<ServiceItem>, String> {
        super::deadline(false, async {
            pagination(page, page_size)?;
            if !text(namespace, 256, true) || !text(group, 256, true) || !text(name, 256, true) {
                return Err(BAD.into());
            }
            let params = vec![
                (
                    "namespaceId".into(),
                    naming_namespace(&namespace_id(namespace)),
                ),
                ("groupNameParam".into(), group.into()),
                ("serviceNameParam".into(), name.into()),
                ("pageNo".into(), page.to_string()),
                ("pageSize".into(), page_size.to_string()),
                ("withInstances".into(), "false".into()),
            ];
            let reply = self
                .runtime(id)?
                .request(Method::GET, "/v1/ns/catalog/services", &params, None)
                .await?;
            reply.success(false)?;
            let value = reply.json()?;
            let rows = value
                .get("serviceList")
                .and_then(Value::as_array)
                .ok_or("服务列表格式异常")?;
            if rows.len() > page_size as usize {
                return Err("服务列表超过请求分页上限".into());
            }
            let mut items = Vec::new();
            for row in rows {
                let key = ServiceKey {
                    namespace_id: namespace_id(namespace),
                    group: row
                        .get("groupName")
                        .and_then(Value::as_str)
                        .unwrap_or("DEFAULT_GROUP")
                        .into(),
                    name: row.get("name").and_then(Value::as_str).ok_or(BAD)?.into(),
                }
                .checked()?;
                items.push(ServiceItem {
                    key,
                    instance_count: row.get("ipCount").and_then(Value::as_u64).unwrap_or(0),
                    healthy_count: row
                        .get("healthyInstanceCount")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    cluster_count: row.get("clusterCount").and_then(Value::as_u64).unwrap_or(0),
                });
            }
            Ok(Page {
                items,
                total: value
                    .get("count")
                    .and_then(Value::as_u64)
                    .ok_or("服务总数缺失")?,
                page,
                page_size,
            })
        })
        .await
    }
    pub async fn service(&self, id: &str, key: ServiceKey) -> Result<ServiceView, String> {
        super::deadline(false,async{
  let key=key.checked()?;let runtime=self.runtime(id)?;let params=key.params();let reply=runtime.request(Method::GET,"/v1/ns/service",&params,None).await?;reply.success(false)?;let row=reply.json()?;
  let received_namespace=row.get("namespaceId").or_else(||row.get("namespace")).and_then(Value::as_str).ok_or("服务命名空间缺失")?;
  let received_name=row.get("name").or_else(||row.get("serviceName")).and_then(Value::as_str).ok_or("服务名称缺失")?;
  let full=format!("{}@@{}",key.group,key.name);
  if namespace_id(received_namespace)!=key.namespace_id||(received_name!=key.name&&received_name!=full)||row.get("groupName").and_then(Value::as_str)!=Some(key.group.as_str()){return Err("服务返回的目标不一致".into());}
  let threshold=row.get("protectThreshold").and_then(Value::as_f64).filter(|v|v.is_finite()&&(0.0..=1.0).contains(v)).ok_or(BAD)?;
  let metadata=metadata(row.get("metadata"))?;let selector=row.get("selector").cloned().unwrap_or_else(||serde_json::json!({"type":"none"}));
  let snapshot=fingerprint(serde_json::json!({"key":key,"threshold":threshold,"metadata":metadata,"selector":selector}));
  // Catalog is the management view: subscriptions omit disabled instances and may rewrite health.
  let clusters=row.get("clusters").and_then(Value::as_array).ok_or("服务集群列表缺失")?;
  if clusters.len()>128{return Err("服务集群超过128个上限".into());}
  let mut instances=Vec::new();let mut names=std::collections::HashSet::new();
  for cluster in clusters {
   let cluster=cluster.get("name").and_then(Value::as_str).filter(|v|text(v,256,false)).ok_or(BAD)?;
   if !names.insert(cluster){return Err("服务集群重复".into());}
   let mut expected_total=None;
   for page in 1..=50 {
    let mut query=params.clone();query.extend([("clusterName".into(),cluster.into()),("pageNo".into(),page.to_string()),("pageSize".into(),"200".into())]);
    let reply=runtime.request(Method::GET,"/v1/ns/catalog/instances",&query,None).await?;reply.success(false)?;let data=reply.json()?;
    let total=data.get("count").and_then(Value::as_u64).filter(|n|*n<=10000).ok_or("实例数量缺失或超过10000上限")?;
    if expected_total.is_some_and(|before|before!=total){return Err("实例列表发生变化，请刷新".into());}expected_total=Some(total);
    let rows=data.get("list").and_then(Value::as_array).ok_or("实例列表格式异常")?;if rows.len()>200{return Err("实例列表超过分页上限".into());}
    for row in rows {let item=instance(row)?;if item.key.cluster!=cluster||instances.iter().any(|before:&NacosInstance|before.key==item.key){return Err("实例列表目标重复或不一致，请刷新".into());}instances.push(item);}
    if instances.len()>10000{return Err("实例数量超过10000上限".into());}
    let collected=instances.iter().filter(|v|v.key.cluster==cluster).count() as u64;
    if collected==total{break;}if rows.is_empty()||collected>total{return Err("实例列表发生变化，请刷新".into());}
   }
  }
  Ok(ServiceView{key,protect_threshold:threshold,metadata,selector,snapshot,instances})
 }).await
    }
    async fn instance(
        &self,
        id: &str,
        key: &ServiceKey,
        target: &InstanceKey,
    ) -> Result<NacosInstance, String> {
        target.check()?;
        // Nacos 2.x detail omits enabled/ephemeral; Catalog retains complete managed instances.
        let mut matches = self
            .service(id, key.clone())
            .await?
            .instances
            .into_iter()
            .filter(|item| &item.key == target);
        let result = matches.next().ok_or("实例已不存在，请刷新")?;
        if matches.next().is_some() {
            return Err("实例目标不唯一".into());
        }
        Ok(result)
    }
    pub async fn update_service(
        &self,
        id: &str,
        key: ServiceKey,
        expected_snapshot: String,
        protect_threshold: f64,
        metadata: BTreeMap<String, String>,
    ) -> Result<ServiceView, String> {
        super::deadline(true, async {
            let key = key.checked()?;
            if !protect_threshold.is_finite() || !(0.0..=1.0).contains(&protect_threshold) {
                return Err(BAD.into());
            }
            check_metadata(&metadata)?;
            let guard = self.begin_write(id)?;
            let _serial = guard.runtime.mutation.lock().await;
            let current = self.service(id, key.clone()).await?;
            if current.snapshot != expected_snapshot {
                return Err(CONFLICT.into());
            }
            let mut params = key.params();
            params.extend([
                ("protectThreshold".into(), protect_threshold.to_string()),
                (
                    "metadata".into(),
                    serde_json::to_string(&metadata).map_err(|_| BAD)?,
                ),
                (
                    "selector".into(),
                    serde_json::to_string(&current.selector).map_err(|_| BAD)?,
                ),
            ]);
            let reply = guard
                .runtime
                .request(Method::PUT, "/v1/ns/service", &params, None)
                .await?;
            reply.success(true)?;
            if reply.bytes != b"ok" {
                return Err(UNKNOWN.into());
            }
            let result = self.service(id, key).await.map_err(|_| UNKNOWN)?;
            if (result.protect_threshold - protect_threshold).abs() > 1e-6
                || result.metadata != metadata
                || result.selector != current.selector
            {
                return Err(UNKNOWN.into());
            }
            Ok(result)
        })
        .await
    }
    pub async fn update_instance(
        &self,
        id: &str,
        key: ServiceKey,
        target: InstanceKey,
        expected_snapshot: String,
        enabled: bool,
        weight: f64,
        metadata: BTreeMap<String, String>,
    ) -> Result<NacosInstance, String> {
        super::deadline(true, async {
            let key = key.checked()?;
            target.check()?;
            if !weight.is_finite() || !(0.0..=10000.0).contains(&weight) {
                return Err(BAD.into());
            }
            check_metadata(&metadata)?;
            let guard = self.begin_write(id)?;
            let _serial = guard.runtime.mutation.lock().await;
            let current = self.instance(id, &key, &target).await?;
            if current.snapshot != expected_snapshot {
                return Err(CONFLICT.into());
            }
            if current
                .metadata
                .keys()
                .any(|key| !metadata.contains_key(key))
            {
                return Err(
                    "Nacos 2.x 实例元数据按覆盖合并，仅支持新增或修改；请保留现有键".into(),
                );
            }
            let mut params = key.params();
            params.extend(target.params());
            params.extend([
                ("enabled".into(), enabled.to_string()),
                ("weight".into(), weight.to_string()),
                (
                    "metadata".into(),
                    serde_json::to_string(&metadata).map_err(|_| BAD)?,
                ),
            ]);
            let reply = guard
                .runtime
                .request(Method::PUT, "/v1/ns/instance", &params, None)
                .await?;
            reply.success(true)?;
            if reply.bytes != b"ok" {
                return Err(UNKNOWN.into());
            }
            for attempt in 0..25 {
                let result = self
                    .instance(id, &key, &target)
                    .await
                    .map_err(|_| UNKNOWN)?;
                if result.enabled == enabled
                    && (result.weight - weight).abs() <= 1e-6
                    && result.metadata == metadata
                {
                    return Ok(result);
                }
                if attempt < 24 {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
            Err(UNKNOWN.into())
        })
        .await
    }
}
#[tauri::command]
pub async fn nacos_services(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    namespace: String,
    group: String,
    name: String,
    page: u32,
    page_size: u32,
) -> Result<Page<ServiceItem>, String> {
    service
        .services(&session_id, &namespace, &group, &name, page, page_size)
        .await
}
#[tauri::command]
pub async fn nacos_service(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    key: ServiceKey,
) -> Result<ServiceView, String> {
    service.service(&session_id, key).await
}
#[tauri::command]
pub async fn nacos_update_service(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    key: ServiceKey,
    expected_snapshot: String,
    protect_threshold: f64,
    metadata: BTreeMap<String, String>,
) -> Result<ServiceView, String> {
    service
        .update_service(
            &session_id,
            key,
            expected_snapshot,
            protect_threshold,
            metadata,
        )
        .await
}
#[tauri::command]
pub async fn nacos_update_instance(
    service: tauri::State<'_, NacosService>,
    session_id: String,
    key: ServiceKey,
    target: InstanceKey,
    expected_snapshot: String,
    enabled: bool,
    weight: f64,
    metadata: BTreeMap<String, String>,
) -> Result<NacosInstance, String> {
    service
        .update_instance(
            &session_id,
            key,
            target,
            expected_snapshot,
            enabled,
            weight,
            metadata,
        )
        .await
}
