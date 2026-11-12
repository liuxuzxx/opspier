use crate::credentials::{CredentialError, CredentialStore, SecretBackend, SecretOwner, SecretRef};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[derive(Default)]
pub(crate) struct MemoryBackend {
    values: Mutex<HashMap<String, Vec<u8>>>,
    pub(crate) reject: AtomicBool,
    pub(crate) reads: AtomicUsize,
    pub(crate) fail_delete: AtomicBool,
    pub(crate) put_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl SecretBackend for MemoryBackend {
    fn put(&self, key: &str, value: &[u8]) -> Result<(), CredentialError> {
        if self.reject.load(Ordering::SeqCst) {
            return Err(CredentialError::Unavailable);
        }
        self.values
            .lock()
            .unwrap()
            .insert(key.into(), value.to_vec());
        if let Some(hook) = self.put_hook.lock().unwrap().take() {
            hook();
        }
        Ok(())
    }
    fn get(&self, key: &str) -> Result<Vec<u8>, CredentialError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.values
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(CredentialError::Missing)
    }
    fn delete(&self, key: &str) -> Result<(), CredentialError> {
        if self.reject.load(Ordering::SeqCst) || self.fail_delete.load(Ordering::SeqCst) {
            return Err(CredentialError::Unavailable);
        }
        self.values
            .lock()
            .unwrap()
            .remove(key)
            .map(|_| ())
            .ok_or(CredentialError::Missing)
    }
}
fn owner(target: &str) -> SecretOwner {
    SecretOwner {
        profile_id: "d072d150-c54c-43b4-8e32-52e0f38b127c".into(),
        target: target.into(),
    }
}

#[test]
fn credentials_require_both_owner_and_target_and_do_not_serialize_material_in_references() {
    let backend = Arc::new(MemoryBackend::default());
    let store = CredentialStore::with_backend(backend);
    let first = owner("mysql-target-a");
    let reference = SecretRef::new(first.clone()).unwrap();
    store
        .put(&first, &reference, b"synthetic-sensitive-value")
        .unwrap();
    assert!(store.get(&first, &reference).unwrap() == b"synthetic-sensitive-value");
    assert!(store.get(&owner("mysql-target-b"), &reference).is_err());
    let other = SecretOwner {
        profile_id: "80dc71b9-d48d-4d50-9d3e-9b0eca71554d".into(),
        target: first.target.clone(),
    };
    assert!(store.get(&other, &reference).is_err());
    let encoded = serde_json::to_string(&reference).unwrap();
    assert!(!encoded.contains("synthetic-sensitive-value"));
    let forged: SecretRef =
        serde_json::from_str(&encoded.replace("mysql-target-a", "mysql-target-b")).unwrap();
    assert!(
        store.get(&owner("mysql-target-b"), &forged).is_err(),
        "changing public metadata cannot rebind the sealed envelope"
    );
}
#[test]
fn failed_persistent_write_does_not_create_a_session_fallback() {
    let backend = Arc::new(MemoryBackend::default());
    backend.reject.store(true, Ordering::SeqCst);
    let store = CredentialStore::with_backend(backend);
    let owner = owner("ssh-target");
    let reference = SecretRef::new(owner.clone()).unwrap();
    assert!(matches!(
        store.put(&owner, &reference, b"synthetic-password"),
        Err(CredentialError::Unavailable)
    ));
    assert!(
        matches!(store.get(&owner, &reference), Err(CredentialError::Missing)),
        "failed save must not succeed from an internal memory fallback"
    );
}
#[test]
fn exact_reference_deletion_cannot_delete_a_different_owner_or_target() {
    let backend = Arc::new(MemoryBackend::default());
    let store = CredentialStore::with_backend(backend);
    let owner = owner("kube-snapshot-a");
    let reference = SecretRef::new(owner.clone()).unwrap();
    store.put(&owner, &reference, b"synthetic-config").unwrap();
    assert!(
        store
            .delete(
                &SecretOwner {
                    target: "other-snapshot".into(),
                    ..owner.clone()
                },
                &reference
            )
            .is_err()
    );
    assert!(store.get(&owner, &reference).is_ok());
    store.delete(&owner, &reference).unwrap();
    assert!(matches!(
        store.get(&owner, &reference),
        Err(CredentialError::Missing)
    ));
}
