use crate::{
    credential_tests::MemoryBackend,
    credentials::CredentialStore,
    profiles::{
        CredentialChange, DatabaseSettings, LegacyProfile, ProfileDraft, ProfileSettings,
        ProfileStore,
    },
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (
    Scratch,
    Arc<MemoryBackend>,
    Arc<CredentialStore>,
    Arc<ProfileStore>,
) {
    let root =
        Scratch(std::env::temp_dir().join(format!("ops-profile-test-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&root.0).unwrap();
    let backend = Arc::new(MemoryBackend::default());
    let credentials = Arc::new(CredentialStore::with_backend(backend.clone()));
    let repo =
        Arc::new(ProfileStore::open(root.0.join("profiles.json"), credentials.clone()).unwrap());
    (root, backend, credentials, repo)
}
fn settings(host: &str) -> ProfileSettings {
    ProfileSettings::Mysql(DatabaseSettings {
        host: host.into(),
        port: 3306,
        username: "reader".into(),
        database: "demo".into(),
        tls: "verifyIdentity".into(),
    })
}
fn draft(name: &str) -> ProfileDraft {
    ProfileDraft {
        id: None,
        expected_revision: None,
        name: name.into(),
        settings: settings("db.example.test"),
    }
}

#[test]
fn profile_metadata_and_public_dto_never_contain_passwords_and_target_edits_cannot_keep_them() {
    let (root, _, _, repo) = fixture();
    let first = repo
        .save(
            draft("saved"),
            CredentialChange::Replace(b"sensitive-test-value".to_vec()),
        )
        .unwrap()
        .profile
        .unwrap();
    assert!(first.credential_saved);
    assert!(
        !std::fs::read_to_string(root.0.join("profiles.json"))
            .unwrap()
            .contains("sensitive-test-value")
    );
    let public = serde_json::to_string(&repo.list().unwrap()).unwrap();
    assert!(!public.contains("sensitive-test-value") && !public.contains("secretRef"));
    let rename = ProfileDraft {
        id: Some(first.id.clone()),
        expected_revision: Some(first.revision),
        name: "renamed".into(),
        settings: first.settings.clone(),
    };
    let current = repo
        .save(rename, CredentialChange::Keep)
        .unwrap()
        .profile
        .unwrap();
    assert!(
        repo.resolve(&current.id, current.revision)
            .unwrap()
            .secret
            .unwrap()
            == b"sensitive-test-value"
    );
    let changed = ProfileDraft {
        id: Some(current.id.clone()),
        expected_revision: Some(current.revision),
        name: "moved".into(),
        settings: settings("other.example.test"),
    };
    assert!(repo.save(changed, CredentialChange::Keep).is_err());
    assert!(
        repo.resolve(&current.id, current.revision).is_ok(),
        "rejected edit leaves the old valid profile unchanged"
    );
}
#[test]
fn cleanup_failure_after_replace_is_committed_and_retried_by_exact_reference_after_reload() {
    let (root, backend, credentials, repo) = fixture();
    let first = repo
        .save(
            draft("saved"),
            CredentialChange::Replace(b"old-test-value".to_vec()),
        )
        .unwrap()
        .profile
        .unwrap();
    backend.fail_delete.store(true, Ordering::SeqCst);
    let result = repo
        .save(
            ProfileDraft {
                id: Some(first.id.clone()),
                expected_revision: Some(first.revision),
                name: first.name,
                settings: first.settings,
            },
            CredentialChange::Replace(b"new-test-value".to_vec()),
        )
        .unwrap();
    assert!(result.cleanup_pending > 0);
    let current = result.profile.unwrap();
    let reloaded = ProfileStore::open(root.0.join("profiles.json"), credentials).unwrap();
    assert!(
        reloaded
            .resolve(&current.id, current.revision)
            .unwrap()
            .secret
            .unwrap()
            == b"new-test-value"
    );
    backend.fail_delete.store(false, Ordering::SeqCst);
    assert_eq!(reloaded.retry_cleanup().unwrap(), 0);
    assert!(
        reloaded.resolve(&current.id, current.revision).is_ok(),
        "cleanup must not delete the new active reference"
    );
}
#[test]
fn failed_final_metadata_commit_preserves_old_profile_and_a_durable_new_reference_cleanup_intent() {
    let (root, backend, credentials, repo) = fixture();
    let first = repo
        .save(
            draft("saved"),
            CredentialChange::Replace(b"old-test-value".to_vec()),
        )
        .unwrap()
        .profile
        .unwrap();
    let path = root.0.join("profiles.json");
    let backup = root.0.join("intent.json");
    let p = path.clone();
    let b = backup.clone();
    *backend.put_hook.lock().unwrap() = Some(Box::new(move || {
        std::fs::rename(&p, &b).unwrap();
        std::fs::create_dir(&p).unwrap();
    }));
    let update = ProfileDraft {
        id: Some(first.id.clone()),
        expected_revision: Some(first.revision),
        name: "new".into(),
        settings: first.settings.clone(),
    };
    assert!(
        repo.save(
            update,
            CredentialChange::Replace(b"new-test-value".to_vec())
        )
        .is_err()
    );
    assert!(
        repo.resolve(&first.id, first.revision)
            .unwrap()
            .secret
            .unwrap()
            == b"old-test-value"
    );
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(backup, &path).unwrap();
    let reloaded = ProfileStore::open(path, credentials).unwrap();
    assert!(reloaded.list().unwrap().cleanup_pending > 0);
    assert_eq!(reloaded.retry_cleanup().unwrap(), 0);
    assert!(reloaded.resolve(&first.id, first.revision).is_ok());
}
#[test]
fn delete_and_remove_commit_metadata_even_when_credential_cleanup_is_pending() {
    let (_root, backend, _credentials, repo) = fixture();
    let first = repo
        .save(
            draft("saved"),
            CredentialChange::Replace(b"test-value".to_vec()),
        )
        .unwrap()
        .profile
        .unwrap();
    backend.fail_delete.store(true, Ordering::SeqCst);
    let removed = repo
        .save(
            ProfileDraft {
                id: Some(first.id.clone()),
                expected_revision: Some(first.revision),
                name: first.name,
                settings: first.settings,
            },
            CredentialChange::Remove,
        )
        .unwrap();
    assert!(removed.cleanup_pending > 0);
    let no_secret = removed.profile.unwrap();
    assert!(!no_secret.credential_saved);
    assert!(
        repo.resolve(&no_secret.id, no_secret.revision)
            .unwrap()
            .secret
            .is_none()
    );
    repo.remove(&no_secret.id, no_secret.revision).unwrap();
    assert!(repo.list().unwrap().profiles.is_empty());
}
#[test]
fn stale_or_concurrent_updates_do_not_resurrect_deleted_profiles_or_delete_new_credentials() {
    let (_root, _backend, _credentials, repo) = fixture();
    let first = repo
        .save(
            draft("saved"),
            CredentialChange::Replace(b"test-value".to_vec()),
        )
        .unwrap()
        .profile
        .unwrap();
    let a = repo.clone();
    let b = repo.clone();
    let p = first.clone();
    let id = first.id.clone();
    let revision = first.revision;
    let update = std::thread::spawn(move || {
        a.save(
            ProfileDraft {
                id: Some(p.id),
                expected_revision: Some(p.revision),
                name: p.name,
                settings: p.settings,
            },
            CredentialChange::Replace(b"new-test-value".to_vec()),
        )
    });
    let delete = std::thread::spawn(move || b.remove(&id, revision));
    assert_ne!(
        update.join().unwrap().is_ok(),
        delete.join().unwrap().is_ok()
    );
    let list = repo.list().unwrap();
    if let Some(current) = list.profiles.first() {
        assert!(
            repo.resolve(&current.id, current.revision)
                .unwrap()
                .secret
                .unwrap()
                == b"new-test-value"
        );
    }
    assert!(
        repo.save(
            ProfileDraft {
                id: Some(first.id),
                expected_revision: Some(first.revision),
                name: "stale".into(),
                settings: settings("db.example.test")
            },
            CredentialChange::Keep
        )
        .is_err()
    );
}
#[test]
fn legacy_source_markers_commit_with_profiles_and_import_is_idempotent() {
    let (root, _, credentials, repo) = fixture();
    let legacy = LegacyProfile {
        source_id: "legacy-db-entry".into(),
        name: "legacy".into(),
        settings: settings("db.example.test"),
    };
    repo.import_legacy(vec![legacy.clone()]).unwrap();
    let reloaded = ProfileStore::open(root.0.join("profiles.json"), credentials).unwrap();
    reloaded.import_legacy(vec![legacy]).unwrap();
    let list = reloaded.list().unwrap();
    assert_eq!(list.profiles.len(), 1);
    assert!(!list.profiles[0].credential_saved);
}

#[test]
fn already_imported_legacy_records_do_not_attempt_another_metadata_write() {
    let (root, _, _, repo) = fixture();
    let legacy = LegacyProfile {
        source_id: "same".into(),
        name: "legacy".into(),
        settings: settings("db.example.test"),
    };
    assert_eq!(repo.import_legacy(vec![legacy.clone()]).unwrap(), 1);
    let path = root.0.join("profiles.json");
    let backup = root.0.join("saved.json");
    std::fs::rename(&path, &backup).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(repo.import_legacy(vec![legacy]).unwrap(), 0);
    assert_eq!(repo.list().unwrap().profiles.len(), 1);
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(backup, path).unwrap();
}

#[test]
fn automatic_mysql_profiles_reopen_with_credentials_without_changing_legacy_policy() {
    let (root, _, credentials, repo) = fixture();
    let legacy = repo
        .save(
            draft("legacy"),
            CredentialChange::Replace(b"legacy-test".to_vec()),
        )
        .unwrap()
        .profile
        .unwrap();
    let mut auto = draft("automatic");
    if let ProfileSettings::Mysql(ref mut settings) = auto.settings {
        settings.tls = "auto".into();
    }
    let saved = repo
        .save(auto, CredentialChange::Replace(b"automatic-test".to_vec()))
        .expect("auto profile should save")
        .profile
        .unwrap();
    let reloaded = ProfileStore::open(root.0.join("profiles.json"), credentials).unwrap();
    assert_eq!(
        reloaded
            .resolve(&saved.id, saved.revision)
            .unwrap()
            .secret
            .unwrap(),
        b"automatic-test"
    );
    let old = reloaded.resolve(&legacy.id, legacy.revision).unwrap();
    if let ProfileSettings::Mysql(ref settings) = old.profile.settings {
        assert_eq!(settings.tls, "verifyIdentity");
    } else {
        panic!("legacy driver changed");
    }
}

#[test]
fn minio_keys_stay_in_system_store_and_endpoint_edits_cannot_reuse_them() {
    let (root, _, credentials, repo) = fixture();
    let value = serde_json::json!({"driver":"minio","endpoint":"https://objects.example.test:9000","region":"us-east-1","bucket":""});
    let config: ProfileSettings =
        serde_json::from_value(value.clone()).expect("MinIO must be a supported profile");
    let material = br#"{"accessKey":"synthetic-access-only","secretKey":"synthetic-secret-only"}"#;
    let saved = repo
        .save(
            ProfileDraft {
                id: None,
                expected_revision: None,
                name: "objects".into(),
                settings: config,
            },
            CredentialChange::Replace(material.to_vec()),
        )
        .unwrap()
        .profile
        .unwrap();
    let public = serde_json::to_string(&repo.list().unwrap()).unwrap();
    let disk = std::fs::read_to_string(root.0.join("profiles.json")).unwrap();
    for secret in ["synthetic-access-only", "synthetic-secret-only"] {
        assert!(!public.contains(secret) && !disk.contains(secret));
    }
    let reloaded = ProfileStore::open(root.0.join("profiles.json"), credentials).unwrap();
    assert!(
        reloaded
            .resolve(&saved.id, saved.revision)
            .unwrap()
            .secret
            .unwrap()
            == material
    );
    let mut next = value.clone();
    next["bucket"] = "photos".into();
    let changed = repo
        .save(
            ProfileDraft {
                id: Some(saved.id.clone()),
                expected_revision: Some(saved.revision),
                name: "photos".into(),
                settings: serde_json::from_value(next.clone()).unwrap(),
            },
            CredentialChange::Keep,
        )
        .unwrap()
        .profile
        .unwrap();
    next["endpoint"] = "https://other.example.test:9000".into();
    assert!(
        repo.save(
            ProfileDraft {
                id: Some(changed.id),
                expected_revision: Some(changed.revision),
                name: "changed".into(),
                settings: serde_json::from_value(next).unwrap()
            },
            CredentialChange::Keep
        )
        .is_err()
    );
}

#[test]
fn invalid_minio_endpoint_and_material_are_rejected_without_saving() {
    let (_, _, _, repo) = fixture();
    for endpoint in [
        "ftp://objects.example.test",
        "https://user:pass@objects.example.test",
        "https://objects.example.test/path",
        "https://objects.example.test?token=value",
        "https://objects.example.test/#fragment",
    ] {
        if let Ok(settings) = serde_json::from_value::<ProfileSettings>(
            serde_json::json!({"driver":"minio","endpoint":endpoint,"region":"us-east-1","bucket":""}),
        ) {
            assert!(
                repo.save(
                    ProfileDraft {
                        id: None,
                        expected_revision: None,
                        name: "invalid".into(),
                        settings
                    },
                    CredentialChange::Remove
                )
                .is_err()
            );
        }
    }
    let settings: ProfileSettings = serde_json::from_value(serde_json::json!({"driver":"minio","endpoint":"http://localhost:9000","region":"us-east-1","bucket":""})).expect("MinIO supported");
    assert!(
        repo.save(
            ProfileDraft {
                id: None,
                expected_revision: None,
                name: "invalid".into(),
                settings
            },
            CredentialChange::Replace(b"not-a-key-pair".to_vec())
        )
        .is_err()
    );
    assert!(repo.list().unwrap().profiles.is_empty());
}
