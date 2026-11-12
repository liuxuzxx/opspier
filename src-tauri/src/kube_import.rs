//! Import is offline; only the resulting OS-keyring material contains config/keys.
use crate::{
    credentials::MAX_SECRET_BYTES,
    profiles::{KubeMaterial, KubeSettings},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use kube::config::Kubeconfig;
use secrecy::ExposeSecret;
use std::{io::Read, os::unix::fs::OpenOptionsExt, path::Path};
use uuid::Uuid;

const SOURCE_LIMIT: usize = 1024 * 1024;
const REFERENCE_LIMIT: usize = 1024 * 1024;
const BAD: &str = "kubeconfig格式、引用或内部Context配置无效";
const TOO_LARGE: &str = "配置源文件/单个引用最多1MiB，规范化快照总计最多2MiB";
pub struct ImportedKube {
    pub settings: KubeSettings,
    pub material: Vec<u8>,
}
fn limited_file(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .map_err(|_| "无法读取明确引用的配置、证书或密钥文件")?;
    if !file.metadata().map_err(|_| BAD)?.is_file() {
        return Err("配置及引用必须是普通文件".into());
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| BAD)?;
    if bytes.len() > limit {
        return Err(TOO_LARGE.into());
    }
    Ok(bytes)
}
fn reference_file(value: &str, base: Option<&Path>, used: &mut usize) -> Result<Vec<u8>, String> {
    if value.len() > 4096 {
        return Err(BAD.into());
    }
    let p = Path::new(value);
    let path = if p.is_absolute() {
        p.to_owned()
    } else {
        base.ok_or("粘贴配置含相对文件引用，请选择原文件导入或使用内联数据")?
            .join(p)
    };
    let bytes = limited_file(&path, REFERENCE_LIMIT)?;
    *used = used.saturating_add(bytes.len().saturating_mul(4) / 3 + 64);
    if *used > MAX_SECRET_BYTES {
        return Err(TOO_LARGE.into());
    }
    Ok(bytes)
}
fn validate(config: &Kubeconfig) -> Result<(), String> {
    crate::kubernetes::validate_config(config)?;
    if config.contexts.is_empty() {
        return Err("配置至少需要一个Context".into());
    }
    for c in &config.contexts {
        if c.name.chars().any(char::is_control) {
            return Err(BAD.into());
        }
        let context = c.context.as_ref().ok_or(BAD)?;
        if !config
            .clusters
            .iter()
            .any(|cluster| cluster.name == context.cluster && cluster.cluster.is_some())
            || (!context.user.is_empty()
                && !config
                    .auth_infos
                    .iter()
                    .any(|user| user.name == context.user))
        {
            return Err(BAD.into());
        }
    }
    Ok(())
}
pub fn import_file(path: &str) -> Result<ImportedKube, String> {
    if !Path::new(path).is_absolute() || path.len() > 4096 {
        return Err("请选择配置文件的绝对路径".into());
    }
    let bytes = limited_file(Path::new(path), SOURCE_LIMIT)?;
    let text = String::from_utf8(bytes).map_err(|_| BAD)?;
    normalize(&text, Path::new(path).parent())
}
pub fn import_text(text: &str) -> Result<ImportedKube, String> {
    normalize(text, None)
}
fn normalize(text: &str, base: Option<&Path>) -> Result<ImportedKube, String> {
    if text.len() > SOURCE_LIMIT {
        return Err(TOO_LARGE.into());
    }
    let mut config = Kubeconfig::from_yaml(text).map_err(|_| BAD)?;
    validate(&config)?;
    let mut used = text.len();
    for named in &mut config.clusters {
        if let Some(cluster) = &mut named.cluster {
            if let Some(value) = &cluster.certificate_authority_data {
                STANDARD.decode(value).map_err(|_| BAD)?;
            } else if let Some(path) = &cluster.certificate_authority {
                cluster.certificate_authority_data =
                    Some(STANDARD.encode(reference_file(path, base, &mut used)?));
            }
            cluster.certificate_authority = None;
        }
    }
    for named in &mut config.auth_infos {
        if let Some(auth) = &mut named.auth_info {
            if let Some(value) = &auth.client_certificate_data {
                STANDARD.decode(value).map_err(|_| BAD)?;
            } else if let Some(path) = &auth.client_certificate {
                auth.client_certificate_data =
                    Some(STANDARD.encode(reference_file(path, base, &mut used)?));
            }
            auth.client_certificate = None;
            if let Some(value) = &auth.client_key_data {
                STANDARD.decode(value.expose_secret()).map_err(|_| BAD)?;
            } else if let Some(path) = &auth.client_key {
                auth.client_key_data = Some(
                    STANDARD
                        .encode(reference_file(path, base, &mut used)?)
                        .into(),
                );
            }
            auth.client_key = None;
            if auth.token.is_none() {
                if let Some(path) = &auth.token_file {
                    auth.token = Some(
                        String::from_utf8(reference_file(path, base, &mut used)?)
                            .map_err(|_| BAD)?
                            .into(),
                    );
                }
            }
            auth.token_file = None;
        }
    }
    let contexts: Vec<String> = config.contexts.iter().map(|c| c.name.clone()).collect();
    let context = config
        .current_context
        .clone()
        .filter(|c| contexts.contains(c));
    let namespace = context
        .as_ref()
        .and_then(|name| config.contexts.iter().find(|c| &c.name == name))
        .and_then(|c| c.context.as_ref())
        .and_then(|c| c.namespace.clone());
    let snapshot_id = Uuid::new_v4().to_string();
    let settings = KubeSettings {
        context,
        namespace,
        contexts,
        snapshot_id: snapshot_id.clone(),
    };
    let material = serde_json::to_vec(&KubeMaterial {
        snapshot_id,
        kubeconfig: serde_yaml::to_string(&config).map_err(|_| BAD)?,
    })
    .map_err(|_| BAD)?;
    if material.len() > MAX_SECRET_BYTES {
        return Err(TOO_LARGE.into());
    }
    Ok(ImportedKube { settings, material })
}
pub fn decode_snapshot(material: &[u8], settings: &KubeSettings) -> Result<Kubeconfig, String> {
    if material.len() > MAX_SECRET_BYTES {
        return Err(TOO_LARGE.into());
    }
    let sealed: KubeMaterial = serde_json::from_slice(material).map_err(|_| BAD)?;
    if sealed.snapshot_id != settings.snapshot_id {
        return Err("已保存的集群快照身份不匹配".into());
    }
    let config = Kubeconfig::from_yaml(&sealed.kubeconfig).map_err(|_| BAD)?;
    validate(&config)?;
    if config
        .contexts
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>()
        != settings.contexts
    {
        return Err("集群Context摘要与安全快照不匹配".into());
    }
    if config
        .clusters
        .iter()
        .filter_map(|c| c.cluster.as_ref())
        .any(|c| c.certificate_authority.is_some())
        || config
            .auth_infos
            .iter()
            .filter_map(|u| u.auth_info.as_ref())
            .any(|u| {
                u.client_key.is_some() || u.client_certificate.is_some() || u.token_file.is_some()
            })
    {
        return Err("已保存快照仍包含文件依赖，请重新导入".into());
    }
    Ok(config)
}
