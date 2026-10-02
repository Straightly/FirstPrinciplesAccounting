//! User records and the AKA identity table (Impl Spec §2.9).
//!
//! M1 note: users are held in backend memory. Durable user records belong to
//! the AccountingBook and arrive with book storage in M3/M4. Before any book
//! exists, the only durable authority is the bootstrap owner (spec §5.3), so
//! in-memory user records are sufficient for the walking skeleton.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
}

#[derive(Default)]
pub struct UserStore {
    inner: RwLock<Inner>,
}

#[derive(Default)]
struct Inner {
    users: HashMap<Uuid, User>,
    /// AKA table: (auth_provider, subject_id) -> user_id (Impl Spec §2.9).
    aka: HashMap<(String, String), Uuid>,
    by_email: HashMap<String, Uuid>,
}

impl UserStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, user_id: Uuid) -> Option<User> {
        self.inner
            .read()
            .expect("user store lock poisoned")
            .users
            .get(&user_id)
            .cloned()
    }

    pub fn list(&self) -> Vec<User> {
        let mut users: Vec<User> = self
            .inner
            .read()
            .expect("user store lock poisoned")
            .users
            .values()
            .cloned()
            .collect();
        users.sort_by_key(|user| user.email.to_lowercase());
        users
    }

    /// Resolve a role-assignment target without requiring that person to
    /// have an active browser session. User ids are derived from normalized
    /// verified email, so assignments survive backend restarts and attach to
    /// the same person when any trusted identity provider later authenticates
    /// that address.
    pub fn resolve_email(&self, email: &str) -> User {
        let email_key = email.trim().to_lowercase();
        let mut inner = self.inner.write().expect("user store lock poisoned");
        if let Some(user_id) = inner.by_email.get(&email_key).copied() {
            return inner
                .users
                .get(&user_id)
                .cloned()
                .expect("email index points to existing user");
        }
        let user_id = stable_user_id(&email_key);
        let user = User {
            user_id,
            display_name: email_key.clone(),
            email: email_key.clone(),
        };
        inner.users.insert(user_id, user.clone());
        inner.by_email.insert(email_key, user_id);
        user
    }

    /// Find-or-create the authorized user for an authenticated identity.
    /// A known (provider, subject) pair resolves through the AKA table; an
    /// unknown pair with a known email attaches to the existing user (same
    /// trusted verified email = same person); otherwise a new user is created.
    pub fn resolve_identity(
        &self,
        provider: &str,
        subject: &str,
        email: &str,
        display_name: &str,
    ) -> User {
        let mut inner = self.inner.write().expect("user store lock poisoned");
        let key = (provider.to_string(), subject.to_string());
        if let Some(user_id) = inner.aka.get(&key).copied() {
            return inner
                .users
                .get(&user_id)
                .cloned()
                .expect("AKA entry points to existing user");
        }
        let email_key = email.to_lowercase();
        let user_id = match inner.by_email.get(&email_key).copied() {
            Some(existing) => existing,
            None => {
                let user_id = stable_user_id(&email_key);
                inner.users.insert(
                    user_id,
                    User {
                        user_id,
                        display_name: display_name.to_string(),
                        email: email.to_string(),
                    },
                );
                inner.by_email.insert(email_key, user_id);
                user_id
            }
        };
        inner.aka.insert(key, user_id);
        inner
            .users
            .get(&user_id)
            .cloned()
            .expect("user just ensured")
    }
}

fn stable_user_id(normalized_email: &str) -> Uuid {
    let digest = Sha256::digest(
        format!("https://firstprinciplesaccounting/users/{normalized_email}").as_bytes(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562-compatible deterministic UUID layout (version 5/variant 1)
    // while using the SHA-256 dependency already present in the backend.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_identity_is_stable_across_stores_and_provider_subjects() {
        let first = UserStore::new();
        let a = first.resolve_email(" Person@Example.COM ");
        let b = first.resolve_identity("google", "subject-1", "person@example.com", "Person");
        let second = UserStore::new();
        let c = second.resolve_identity("oidc", "subject-2", "PERSON@example.com", "P");
        assert_eq!(a.user_id, b.user_id);
        assert_eq!(b.user_id, c.user_id);
        assert_eq!(a.email, "person@example.com");
    }
}
