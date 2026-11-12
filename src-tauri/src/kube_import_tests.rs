use crate::kube_import::{decode_snapshot, import_file, import_text};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::path::PathBuf;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn scratch() -> Scratch {
    let p = Scratch(std::env::temp_dir().join(format!("ops-kube-import-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&p.0).unwrap();
    p
}
fn config(cluster: &str, user: &str) -> String {
    format!(
        "apiVersion: v1\nkind: Config\nclusters:\n- name: cluster-original\n  cluster:\n    server: http://127.0.0.1:1\n{cluster}\nusers:\n- name: user-original\n  user:\n{user}\ncontexts:\n- name: context-original\n  context: {{cluster: cluster-original, user: user-original, namespace: ops}}\n"
    )
}

#[test]
fn file_import_snapshots_relative_references_and_preserves_all_internal_names() {
    let root = scratch();
    std::fs::create_dir(root.0.join("certs")).unwrap();
    for (name, value) in [
        ("ca", "CA"),
        ("cert", "CERT"),
        ("key", "synthetic-key"),
        ("token", "synthetic-token"),
    ] {
        std::fs::write(root.0.join("certs").join(name), value).unwrap();
    }
    let path = root.0.join("config.yaml");
    std::fs::write(&path,config("    certificate-authority: certs/ca","    client-certificate: certs/cert\n    client-key: certs/key\n    tokenFile: certs/token")).unwrap();
    let imported = import_file(path.to_str().unwrap()).unwrap();
    assert!(uuid::Uuid::parse_str(&imported.settings.snapshot_id).is_ok());
    let public = serde_json::to_string(&imported.settings).unwrap();
    assert!(!public.contains("synthetic-token") && !public.contains("synthetic-key"));
    std::fs::remove_dir_all(root.0.join("certs")).unwrap();
    std::fs::remove_file(path).unwrap();
    let decoded = decode_snapshot(&imported.material, &imported.settings).unwrap();
    assert_eq!(decoded.clusters[0].name, "cluster-original");
    assert_eq!(decoded.contexts[0].name, "context-original");
    let cluster = decoded.clusters[0].cluster.as_ref().unwrap();
    assert!(cluster.certificate_authority.is_none());
    assert_eq!(
        cluster.certificate_authority_data.as_deref(),
        Some(STANDARD.encode("CA").as_str())
    );
    let auth = decoded.auth_infos[0].auth_info.as_ref().unwrap();
    assert!(
        auth.token_file.is_none() && auth.client_key.is_none() && auth.client_certificate.is_none()
    );
    let serialized = serde_yaml::to_string(&decoded).unwrap();
    assert!(
        serialized.contains("synthetic-token"),
        "token must be retained only in the sealed material"
    );
    assert_eq!(
        decoded.contexts[0].context.as_ref().unwrap().cluster,
        "cluster-original"
    );
    assert!(
        imported.settings.context.is_none(),
        "missing current-context is supported for explicit selection at open"
    );
}
#[test]
fn pasted_config_rejects_used_relative_paths_but_inline_values_override_unused_file_references() {
    assert!(
        import_text(&config(
            "    certificate-authority: relative/ca",
            "    token: synthetic-token"
        ))
        .is_err()
    );
    let yaml = config(
        &format!(
            "    certificate-authority: ignored/ca\n    certificate-authority-data: {}",
            STANDARD.encode("CA")
        ),
        "    token: synthetic-inline\n    tokenFile: ignored/token",
    );
    let imported = import_text(&yaml).unwrap();
    let decoded = decode_snapshot(&imported.material, &imported.settings).unwrap();
    assert!(
        decoded.clusters[0]
            .cluster
            .as_ref()
            .unwrap()
            .certificate_authority
            .is_none()
    );
    assert!(
        decoded.auth_infos[0]
            .auth_info
            .as_ref()
            .unwrap()
            .token_file
            .is_none()
    );
}
#[test]
fn import_rejects_external_auth_plugins_and_invalid_or_oversize_file_references() {
    let exec = config("", "    exec: {command: must-never-execute}");
    assert!(import_text(&exec).is_err());
    let provider = config("", "    auth-provider: {name: must-never-execute}");
    assert!(import_text(&provider).is_err());
    let root = scratch();
    let path = root.0.join("config.yaml");
    std::fs::write(
        &path,
        config(
            "    certificate-authority: missing",
            "    token: synthetic-token",
        ),
    )
    .unwrap();
    assert!(import_file(path.to_str().unwrap()).is_err());
    std::fs::write(root.0.join("large"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    std::fs::write(
        &path,
        config(
            "    certificate-authority: large",
            "    token: synthetic-token",
        ),
    )
    .unwrap();
    assert!(import_file(path.to_str().unwrap()).is_err());
    assert!(import_text(&("#".to_owned() + &"x".repeat(1024 * 1024))).is_err());
}
#[test]
fn normalized_snapshot_limit_is_checked_and_opaque_identity_cannot_be_rebound() {
    let root = scratch();
    for name in ["ca", "cert", "key"] {
        std::fs::write(root.0.join(name), vec![b'x'; 800 * 1024]).unwrap();
    }
    let path = root.0.join("config.yaml");
    std::fs::write(
        &path,
        config(
            "    certificate-authority: ca",
            "    client-certificate: cert\n    client-key: key",
        ),
    )
    .unwrap();
    assert!(import_file(path.to_str().unwrap()).is_err());
    let imported = import_text(&config("", "    token: synthetic-token")).unwrap();
    let mut changed = imported.settings.clone();
    changed.snapshot_id = uuid::Uuid::new_v4().to_string();
    assert!(decode_snapshot(&imported.material, &changed).is_err());
}
