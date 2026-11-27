//! User authentication and authorization.
//!
//! File-based user store with Argon2id password hashing and random bearer tokens.
//! Legacy SHA-256 hashes are upgraded transparently after a successful login.

use crate::Result;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// A registered user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// Unique user ID
    pub id: String,
    /// Display name
    pub nickname: String,
    /// Email (used for login)
    pub email: String,
    /// PHC-encoded Argon2id hash. Legacy SHA-256 hex hashes are migrated on login.
    pub password_hash: String,
    /// User role: "admin" or "normal"
    pub role: String,
    /// Optional profile avatar (data URL or asset path).
    #[serde(default)]
    pub avatar: String,
    /// Optional IANA-style timezone display string.
    #[serde(default)]
    pub timezone: String,
    /// Account creation timestamp (Unix ms)
    pub created_at: u64,
}

/// In-memory token store (token → user_id).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenEntry {
    user_id: String,
    expires_at: u64,
}

/// User database with file persistence.
pub struct UserStore {
    /// All registered users (email → User)
    users: RwLock<HashMap<String, User>>,
    /// Active tokens (token → TokenEntry)
    tokens: RwLock<HashMap<String, TokenEntry>>,
    /// Path to users.json file
    file_path: String,
    save_lock: Mutex<()>,
    password_transport: Option<Arc<crate::password_transport::PasswordTransport>>,
}

/// Resolve an email to its stored user.
///
/// Every write path stores the lower-cased address (`register` and
/// `create_user` both call `to_ascii_lowercase`), and upstream's MySQL
/// comparison is case-insensitive too, but a store file written by an older
/// build can still hold mixed case — so an exact hit wins and a
/// case-insensitive scan follows. Without this fallback a user who signed up as
/// `Ada@Example.com` was stored as `ada@example.com` and could never sign in
/// with the address they typed.
fn find_user<'a>(users: &'a HashMap<String, User>, email: &str) -> Option<&'a User> {
    let email = email.trim();
    if let Some(user) = users.get(email) {
        return Some(user);
    }
    users
        .values()
        .find(|user| user.email.eq_ignore_ascii_case(email))
}

impl UserStore {
    /// Create or load user store from file.
    pub fn new(file_path: &str) -> Result<Self> {
        let password_transport =
            crate::password_transport::PasswordTransport::from_env()?.map(Arc::new);
        Self::new_with_password_transport(file_path, password_transport)
    }

    fn new_with_password_transport(
        file_path: &str,
        password_transport: Option<Arc<crate::password_transport::PasswordTransport>>,
    ) -> Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(file_path))?;
        let users = if std::path::Path::new(file_path).exists() {
            let data = std::fs::read_to_string(file_path)?;
            let user_list: Vec<User> = serde_json::from_str(&data)?;
            user_list
                .into_iter()
                .map(|u| (u.email.clone(), u))
                .collect()
        } else {
            // No account is pre-created, ever. The first registration becomes the RayRAG
            // administrator; the legacy RAYRAG_ADMIN_* seed variables are deliberately
            // ignored so a deployment can never start with a known admin account.
            HashMap::new()
        };

        let store = Self {
            users: RwLock::new(users),
            tokens: RwLock::new(HashMap::new()),
            file_path: file_path.to_string(),
            save_lock: Mutex::new(()),
            password_transport,
        };
        store.persist_current()?;
        Ok(store)
    }

    /// Decode a password that may have travelled through the RSA transport, so a
    /// caller can reuse the plaintext for an out-of-band credential update.
    pub fn decode_password_input(&self, password: &str) -> Result<String> {
        Ok(self.decode_password(password)?.into_owned())
    }

    /// Remove a user that was just created but whose follow-up provisioning
    /// failed, so no half-configured administrator account is left behind.
    pub fn discard_user(&self, user_id: &str) -> Result<()> {
        self.mutate(|users| {
            let email = users
                .values()
                .find(|user| user.id == user_id)
                .map(|user| user.email.clone());
            match email {
                Some(email) => {
                    users.remove(&email);
                    Ok(((), true))
                }
                None => Ok(((), false)),
            }
        })
        .map(|_| ())
    }

    /// Authenticate with email + password. Returns token on success.
    pub fn login(&self, email: &str, password: &str) -> Result<Option<String>> {
        let password = self.decode_password(password)?;
        let (user_id, verified_hash, needs_upgrade) = {
            let users = self.users.read().unwrap();
            let user = match find_user(&users, email) {
                Some(user) => user,
                None => return Ok(None),
            };
            match verify_password(password.as_ref(), &user.password_hash) {
                PasswordVerification::Valid { needs_upgrade } => {
                    (user.id.clone(), user.password_hash.clone(), needs_upgrade)
                }
                PasswordVerification::Invalid => return Ok(None),
            }
        };

        if needs_upgrade
            && !self.upgrade_legacy_password(&user_id, password.as_ref(), &verified_hash)?
        {
            return Ok(None);
        }

        // Generate token
        let token = generate_token();
        let entry = TokenEntry {
            user_id,
            expires_at: now_ms() + 24 * 3600 * 1000, // 24 hours
        };

        self.tokens.write().unwrap().insert(token.clone(), entry);
        Ok(Some(token))
    }

    fn upgrade_legacy_password(
        &self,
        user_id: &str,
        password: &str,
        verified_hash: &str,
    ) -> Result<bool> {
        let password_hash = hash_password(password)?;
        self.mutate_if_changed(|users| {
            let Some(user) = users.values_mut().find(|user| user.id == user_id) else {
                return Ok((false, false));
            };
            // A password reset or another login may have replaced the verified hash.
            if user.password_hash != verified_hash {
                let valid = matches!(
                    verify_password(password, &user.password_hash),
                    PasswordVerification::Valid { .. }
                );
                return Ok((valid, false));
            }
            user.password_hash = password_hash;
            Ok((true, true))
        })
        .map_err(|error| anyhow::anyhow!(
            "Cannot save the upgraded account password. Check account storage permissions and available disk space, then log in again: {error}"
        ))
    }

    /// 修改密码（校验当前密码后更新哈希）。
    pub fn change_password(&self, user_id: &str, current: &str, new_password: &str) -> Result<()> {
        if !self.update_password_with(user_id, Some(current), new_password, |_| Ok(()))? {
            anyhow::bail!("User not found");
        }
        Ok(())
    }

    /// Hold account mutation locks through administrator database synchronization.
    pub(crate) fn update_password_with(
        &self,
        user_id: &str,
        current: Option<&str>,
        new_password: &str,
        synchronize_admin: impl FnOnce(&str) -> Result<()>,
    ) -> Result<bool> {
        let current = current
            .map(|value| self.decode_password(value))
            .transpose()?;
        let new_password = self.decode_password(new_password)?;
        if new_password.len() < 6 {
            anyhow::bail!("New password must be at least 6 characters");
        }
        let password_hash = hash_password(new_password.as_ref())?;
        let _save_guard = self.save_lock.lock().unwrap();
        let mut users = self.users.write().unwrap();
        let Some(user) = users.values().find(|user| user.id == user_id) else {
            return Ok(false);
        };
        if let Some(current) = current
            && matches!(
                verify_password(current.as_ref(), &user.password_hash),
                PasswordVerification::Invalid
            )
        {
            anyhow::bail!("Current password is incorrect");
        }
        let is_admin = user.role == "admin";
        let mut snapshot: Vec<User> = users.values().cloned().collect();
        let updated_user = snapshot.iter_mut().find(|user| user.id == user_id).unwrap();
        updated_user.password_hash = password_hash.clone();

        // Save the proposed hash before changing the external database credential.
        // Readers and other mutations remain blocked until both steps succeed.
        self.persist(&snapshot).map_err(|error| anyhow::anyhow!(
            "Cannot save the account password. Check account storage permissions and available disk space, then retry: {error}"
        ))?;
        if is_admin && let Err(error) = synchronize_admin(new_password.as_ref()) {
            let previous: Vec<User> = users.values().cloned().collect();
            if let Err(recovery) = self.persist(&previous) {
                anyhow::bail!(
                    "Administrator database credential synchronization failed: {error}. Account file recovery also failed: {recovery}. Repair account storage permissions or disk space and restore the account file from a trusted backup before retrying."
                );
            }
            anyhow::bail!(
                "Administrator database credential synchronization failed: {error}. The previous account password was restored. Check the database connection, credential files and env permissions before retrying."
            );
        }
        users
            .values_mut()
            .find(|user| user.id == user_id)
            .unwrap()
            .password_hash = password_hash;
        Ok(true)
    }

    /// Update the editable profile fields (nickname / avatar / timezone) for
    /// the user with the given id. Only `Some` fields are applied.
    pub fn update_profile(
        &self,
        user_id: &str,
        nickname: Option<String>,
        avatar: Option<String>,
        timezone: Option<String>,
    ) -> Result<()> {
        self.mutate_if_changed(|users| {
            let Some(user) = users.values_mut().find(|user| user.id == user_id) else {
                anyhow::bail!("User not found");
            };
            if let Some(nickname) = nickname {
                user.nickname = nickname;
            }
            if let Some(avatar) = avatar {
                user.avatar = avatar;
            }
            if let Some(timezone) = timezone {
                user.timezone = timezone;
            }
            Ok(((), true))
        })
    }

    /// 管理员：删除用户（保留主管理员，禁止删自己）。
    pub fn delete_user(&self, user_id: &str, acting_user: &str) -> Result<bool> {
        if user_id == acting_user {
            anyhow::bail!("Cannot delete the currently logged-in user");
        }
        self.mutate_if_changed(|users| {
            let Some(email) = users
                .iter()
                .find(|(_, user)| user.id == user_id)
                .map(|(email, _)| email.clone())
            else {
                return Ok((false, false));
            };
            users.remove(&email);
            Ok((true, true))
        })
    }

    /// 管理员：重置指定用户密码。
    pub fn reset_password(&self, user_id: &str, new_password: &str) -> Result<bool> {
        self.update_password_with(user_id, None, new_password, |_| Ok(()))
    }

    /// 管理员：以显式角色创建用户（RAGFlow admin create_user 移植）。
    ///
    /// 用户名兼作登录名（本存储按 email 建索引）；密码沿用自助注册的
    /// 最小长度策略。
    pub fn create_user(&self, username: &str, password: &str, role: &str) -> Result<User> {
        let username = username.trim();
        let password = self.decode_password(password)?;
        if username.is_empty() {
            anyhow::bail!("Username is required");
        }
        if password.len() < 12 {
            anyhow::bail!("Password must be at least 12 characters");
        }
        let role = if role == "admin" { "admin" } else { "normal" };
        let user = User {
            id: uuid::Uuid::new_v4().to_string(),
            nickname: username.to_string(),
            email: username.to_ascii_lowercase(),
            password_hash: hash_password(password.as_ref())?,
            role: role.to_string(),
            avatar: String::new(),
            timezone: String::new(),
            created_at: now_ms(),
        };
        self.mutate(|users| {
            if users.contains_key(&user.email) {
                anyhow::bail!("User '{}' already exists", user.email);
            }
            users.insert(user.email.clone(), user.clone());
            Ok(user)
        })
    }

    /// 管理员：授予或撤销管理员角色（RAGFlow grant_admin/revoke_admin 移植）。
    pub fn set_role(&self, user_id: &str, role: &str) -> Result<bool> {
        let role = if role == "admin" { "admin" } else { "normal" };
        self.mutate_if_changed(|users| {
            let Some(user) = users.values_mut().find(|user| user.id == user_id) else {
                return Ok((false, false));
            };
            if user.role == role {
                return Ok((true, false));
            }
            user.role = role.to_string();
            Ok((true, true))
        })
    }

    /// Issue a session token for an already-authenticated user (OAuth callback).
    pub fn issue_token_for(&self, user_id: &str) -> Option<String> {
        let exists = self
            .users
            .read()
            .unwrap()
            .values()
            .any(|user| user.id == user_id);
        if !exists {
            return None;
        }
        let token = generate_token();
        self.tokens.write().unwrap().insert(
            token.clone(),
            TokenEntry {
                user_id: user_id.to_string(),
                expires_at: now_ms() + 24 * 3600 * 1000,
            },
        );
        Some(token)
    }

    /// Validate a token and return the user_id if valid.
    pub fn validate_token(&self, token: &str) -> Option<String> {
        let tokens = self.tokens.read().unwrap();
        let entry = tokens.get(token)?;
        if entry.expires_at < now_ms() {
            return None;
        }
        Some(entry.user_id.clone())
    }

    /// Register a new user.
    pub fn register(&self, nickname: &str, email: &str, password: &str) -> Result<User> {
        self.register_with(nickname, email, password, |_| Ok(()))
    }

    pub(crate) fn register_with(
        &self,
        nickname: &str,
        email: &str,
        password: &str,
        synchronize_admin: impl FnOnce(&str) -> Result<()>,
    ) -> Result<User> {
        let nickname = nickname.trim();
        let email = email.trim().to_ascii_lowercase();
        let password = self.decode_password(password)?;
        if nickname.is_empty() {
            anyhow::bail!("Nickname is required");
        }
        if email.len() > 254
            || !email.contains('@')
            || email.starts_with('@')
            || email.ends_with('@')
        {
            anyhow::bail!("A valid email address is required");
        }
        if password.len() < 12 {
            anyhow::bail!("Password must be at least 12 characters");
        }

        let mut user = User {
            id: uuid::Uuid::new_v4().to_string(),
            nickname: nickname.to_string(),
            email: email.clone(),
            password_hash: hash_password(password.as_ref())?,
            role: "normal".into(),
            avatar: String::new(),
            timezone: String::new(),
            created_at: now_ms(),
        };
        let _save_guard = self.save_lock.lock().unwrap();
        let mut users = self.users.write().unwrap();
        if users.contains_key(&email) {
            anyhow::bail!("User with email '{}' already exists", email);
        }
        if users.is_empty() {
            user.role = "admin".into();
        }
        let mut snapshot: Vec<User> = users.values().cloned().collect();
        snapshot.push(user.clone());
        self.persist(&snapshot).map_err(|error| anyhow::anyhow!(
            "Cannot save the new account. Check account storage permissions and available disk space, then register again: {error}"
        ))?;
        if user.role == "admin"
            && let Err(error) = synchronize_admin(password.as_ref())
        {
            let previous: Vec<User> = users.values().cloned().collect();
            if let Err(recovery) = self.persist(&previous) {
                anyhow::bail!(
                    "Administrator database credential synchronization failed: {error}. Registration account file recovery also failed: {recovery}. Repair account storage and restore its trusted backup before registering again."
                );
            }
            anyhow::bail!(
                "Administrator database credential synchronization failed: {error}. Registration was rolled back. Check the database connection, credential files and env permissions, then register again."
            );
        }
        users.insert(email, user.clone());
        Ok(user)
    }

    /// Get user by token.
    pub fn get_user_by_token(&self, token: &str) -> Option<User> {
        let user_id = self.validate_token(token)?;
        let users = self.users.read().unwrap();
        users.values().find(|u| u.id == user_id).cloned()
    }

    /// Get user by email.
    pub fn get_user(&self, email: &str) -> Option<User> {
        find_user(&self.users.read().unwrap(), email).cloned()
    }

    /// Get user by stable id.
    pub fn get_user_by_id(&self, user_id: &str) -> Option<User> {
        self.users
            .read()
            .unwrap()
            .values()
            .find(|user| user.id == user_id)
            .cloned()
    }

    pub fn is_admin(&self, user_id: &str) -> bool {
        self.get_user_by_id(user_id)
            .is_some_and(|user| user.role == "admin")
    }

    pub fn list_users(&self) -> Vec<User> {
        self.users.read().unwrap().values().cloned().collect()
    }

    /// Public half of the optional deployment-specific password transport key.
    pub fn password_public_key(&self) -> Option<&str> {
        self.password_transport
            .as_deref()
            .map(crate::password_transport::PasswordTransport::public_key_pem)
    }

    /// Logout (invalidate token).
    pub fn logout(&self, token: &str) {
        self.tokens.write().unwrap().remove(token);
    }

    /// Log out every active token belonging to a user (used after a password
    /// change, mirroring upstream's access-token invalidation).
    pub fn logout_user_tokens(&self, user_id: &str) {
        self.tokens
            .write()
            .unwrap()
            .retain(|_, entry| entry.user_id != user_id);
    }

    fn decode_password<'a>(&self, value: &'a str) -> Result<std::borrow::Cow<'a, str>> {
        match self.password_transport.as_deref() {
            Some(transport) => transport.decode(value),
            None => Ok(std::borrow::Cow::Borrowed(value)),
        }
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, User>) -> Result<T>,
    ) -> Result<T> {
        self.mutate_if_changed(|users| mutation(users).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, User>) -> Result<(T, bool)>,
    ) -> Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut users = self.users.write().unwrap();
        let previous = users.clone();
        let (value, changed) = mutation(&mut users)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<User> = users.values().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *users = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist_current(&self) -> Result<()> {
        let _save_guard = self.save_lock.lock().unwrap();
        let users: Vec<User> = self.users.read().unwrap().values().cloned().collect();
        self.persist(&users)
    }

    fn persist(&self, users: &[User]) -> Result<()> {
        let data = serde_json::to_vec_pretty(&users)?;
        crate::persistence::atomic_write(std::path::Path::new(&self.file_path), &data)
    }
}

/// Hash a password using Argon2id with a unique random salt.
#[cfg(test)]
pub(crate) fn hash_password_for_test(password: &str) -> String {
    hash_password(password).expect("test password hashing must succeed")
}

fn hash_password(password: &str) -> Result<String> {
    let salt_bytes: [u8; 16] = rand::rng().random();
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|error| anyhow::anyhow!("failed to generate password salt: {error}"))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| anyhow::anyhow!("failed to hash password: {error}"))
}

enum PasswordVerification {
    Valid { needs_upgrade: bool },
    Invalid,
}

fn verify_password(password: &str, encoded_hash: &str) -> PasswordVerification {
    if encoded_hash.starts_with("$argon2") {
        let valid = PasswordHash::new(encoded_hash)
            .ok()
            .and_then(|hash| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &hash)
                    .ok()
            })
            .is_some();
        return if valid {
            PasswordVerification::Valid {
                needs_upgrade: false,
            }
        } else {
            PasswordVerification::Invalid
        };
    }

    if is_legacy_sha256_hash(encoded_hash)
        && constant_time_eq(encoded_hash.as_bytes(), legacy_sha256(password).as_bytes())
    {
        PasswordVerification::Valid {
            needs_upgrade: true,
        }
    } else {
        PasswordVerification::Invalid
    }
}

fn is_legacy_sha256_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn legacy_sha256(password: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.update(b"rayrag-salt-v1");
    format!("{:x}", hasher.finalize())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

/// Generate a random token string (64 hex chars = 256 bits).
fn generate_token() -> String {
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Current Unix timestamp in milliseconds.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use rsa::pkcs8::{DecodePublicKey, EncodePrivateKey, LineEnding};
    use rsa::rand_core::OsRng;
    use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};

    fn temp_path(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("rayrag-{name}-{}.json", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned()
    }

    fn password_fixture(root: &std::path::Path, role: &str) -> (UserStore, User) {
        let path = root.join("users.json");
        std::fs::write(&path, b"[]").unwrap();
        let store = UserStore::new(path.to_str().unwrap()).unwrap();
        let user = store
            .create_user("owner@example.test", "initial-password", role)
            .unwrap();
        (store, user)
    }

    #[test]
    fn legacy_password_login_upgrades_with_normalized_email() {
        let root = tempfile::tempdir().unwrap();
        let (store, user) = password_fixture(root.path(), "admin");
        store
            .mutate(|users| {
                users.get_mut(&user.email).unwrap().password_hash =
                    legacy_sha256("initial-password");
                Ok(())
            })
            .unwrap();
        let token = store
            .login(" OWNER@EXAMPLE.TEST ", "initial-password")
            .unwrap()
            .unwrap();
        assert_eq!(store.validate_token(&token).unwrap(), user.id);
        assert!(
            store
                .get_user(&user.email)
                .unwrap()
                .password_hash
                .starts_with("$argon2")
        );
        let reopened = UserStore::new(root.path().join("users.json").to_str().unwrap()).unwrap();
        assert!(
            reopened
                .get_user(&user.email)
                .unwrap()
                .password_hash
                .starts_with("$argon2")
        );
        assert!(
            reopened
                .login(&user.email, "initial-password")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn legacy_password_upgrade_cannot_overwrite_synchronized_password_reset() {
        let root = tempfile::tempdir().unwrap();
        let (store, user) = password_fixture(root.path(), "admin");
        let legacy_hash = legacy_sha256("initial-password");
        store
            .mutate(|users| {
                users.get_mut(&user.email).unwrap().password_hash = legacy_hash.clone();
                Ok(())
            })
            .unwrap();
        let mut synchronized_password = String::new();
        store
            .update_password_with(&user.id, None, "replacement-password", |password| {
                synchronized_password = password.to_owned();
                Ok(())
            })
            .unwrap();
        let path = root.path().join("users.json");
        let saved = std::fs::read(&path).unwrap();
        assert!(
            !store
                .upgrade_legacy_password(&user.id, "initial-password", &legacy_hash)
                .unwrap()
        );
        assert_eq!(synchronized_password, "replacement-password");
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        assert!(
            store
                .login(&user.email, "initial-password")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .login(&user.email, "replacement-password")
                .unwrap()
                .is_some()
        );
        let reopened = UserStore::new(path.to_str().unwrap()).unwrap();
        assert!(
            reopened
                .login(&user.email, "initial-password")
                .unwrap()
                .is_none()
        );
        assert!(
            reopened
                .login(&user.email, "replacement-password")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn overlapping_legacy_upgrades_preserve_saved_hash_and_deleted_accounts() {
        let root = tempfile::tempdir().unwrap();
        let (store, user) = password_fixture(root.path(), "normal");
        let legacy_hash = legacy_sha256("initial-password");
        store
            .mutate(|users| {
                users.get_mut(&user.email).unwrap().password_hash = legacy_hash.clone();
                Ok(())
            })
            .unwrap();
        assert!(
            store
                .upgrade_legacy_password(&user.id, "initial-password", &legacy_hash)
                .unwrap()
        );
        let path = root.path().join("users.json");
        let saved = std::fs::read(&path).unwrap();
        assert!(
            store
                .upgrade_legacy_password(&user.id, "initial-password", &legacy_hash)
                .unwrap()
        );
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        store.discard_user(&user.id).unwrap();
        assert!(
            !store
                .upgrade_legacy_password(&user.id, "initial-password", &legacy_hash)
                .unwrap()
        );
        assert!(store.list_users().is_empty());
        assert!(
            UserStore::new(path.to_str().unwrap())
                .unwrap()
                .list_users()
                .is_empty()
        );
    }

    #[test]
    fn legacy_password_upgrade_storage_failure_has_repair_guidance() {
        let root = tempfile::tempdir().unwrap();
        let (store, user) = password_fixture(root.path(), "admin");
        let legacy_hash = legacy_sha256("initial-password");
        store
            .mutate(|users| {
                users.get_mut(&user.email).unwrap().password_hash = legacy_hash.clone();
                Ok(())
            })
            .unwrap();
        let path = root.path().join("users.json");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let error = store
            .login(&user.email, "initial-password")
            .unwrap_err()
            .to_string();
        assert!(error.contains("storage permissions"), "{error}");
        assert!(error.contains("disk space"), "{error}");
        assert!(error.contains("log in again"), "{error}");
        assert_eq!(
            store.get_user(&user.email).unwrap().password_hash,
            legacy_hash
        );
        assert!(store.tokens.read().unwrap().is_empty());
    }

    #[test]
    fn administrator_password_sync_failure_restores_memory_and_file() {
        let root = tempfile::tempdir().unwrap();
        let (store, user) = password_fixture(root.path(), "admin");
        let path = root.path().join("users.json");
        let previous = std::fs::read(&path).unwrap();
        let error = store
            .update_password_with(
                &user.id,
                Some("initial-password"),
                "replacement-password",
                |password| {
                    assert_eq!(password, "replacement-password");
                    anyhow::bail!("fixture database unavailable")
                },
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("fixture database unavailable"), "{error}");
        assert!(
            error.contains("previous account password was restored"),
            "{error}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        assert!(
            store
                .login(&user.email, "initial-password")
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .login(&user.email, "replacement-password")
                .unwrap()
                .is_none()
        );
        let reopened = UserStore::new(path.to_str().unwrap()).unwrap();
        assert!(
            reopened
                .login(&user.email, "initial-password")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn account_save_failure_precedes_administrator_database_sync() {
        let root = tempfile::tempdir().unwrap();
        let (store, user) = password_fixture(root.path(), "admin");
        let path = root.path().join("users.json");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let error = store
            .update_password_with(&user.id, None, "replacement-password", |_| {
                panic!("database sync must follow account persistence")
            })
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Cannot save the account password"),
            "{error}"
        );
        assert!(error.contains("permissions"), "{error}");
        assert!(
            store
                .login(&user.email, "initial-password")
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .login(&user.email, "replacement-password")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn account_password_recovery_failure_is_reported() {
        let root = tempfile::tempdir().unwrap();
        let (store, user) = password_fixture(root.path(), "admin");
        let path = root.path().join("users.json");
        let error = store
            .update_password_with(&user.id, None, "replacement-password", |_| {
                std::fs::remove_file(&path).unwrap();
                std::fs::create_dir(&path).unwrap();
                anyhow::bail!("fixture database unavailable")
            })
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Account file recovery also failed"),
            "{error}"
        );
        assert!(error.contains("trusted backup"), "{error}");
        assert!(
            !error.contains("previous account password was restored"),
            "{error}"
        );
        assert!(
            store
                .login(&user.email, "initial-password")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn normal_account_password_update_preserves_database_credentials() {
        let root = tempfile::tempdir().unwrap();
        let (store, user) = password_fixture(root.path(), "normal");
        assert!(
            store
                .update_password_with(
                    &user.id,
                    Some("initial-password"),
                    "replacement-password",
                    |_| panic!("normal user requested database synchronization"),
                )
                .unwrap()
        );
        let reopened = UserStore::new(root.path().join("users.json").to_str().unwrap()).unwrap();
        assert!(
            reopened
                .login(&user.email, "replacement-password")
                .unwrap()
                .is_some()
        );
        assert!(
            !store
                .update_password_with("missing", None, "replacement-password", |_| panic!(
                    "missing user requested database synchronization"
                ))
                .unwrap()
        );
    }

    #[test]
    fn concurrent_password_changes_validate_the_latest_password_under_lock() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };
        let root = tempfile::tempdir().unwrap();
        let (store, user) = password_fixture(root.path(), "admin");
        let store = Arc::new(store);
        let barrier = Arc::new(Barrier::new(2));
        let synchronizations = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = ["first-replacement", "second-replacement"]
            .into_iter()
            .map(|password| {
                let store = store.clone();
                let user_id = user.id.clone();
                let barrier = barrier.clone();
                let synchronizations = synchronizations.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let result = store.update_password_with(
                        &user_id,
                        Some("initial-password"),
                        password,
                        |_| {
                            synchronizations.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        },
                    );
                    (password, result)
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(
            results
                .iter()
                .filter(|(_, result)| matches!(result, Ok(true)))
                .count(),
            1
        );
        assert_eq!(synchronizations.load(Ordering::SeqCst), 1);
        for (password, result) in results {
            let succeeded = result.is_ok();
            if let Err(error) = result {
                assert!(
                    error.to_string().contains("Current password is incorrect"),
                    "{error}"
                );
            }
            assert_eq!(
                store.login(&user.email, password).unwrap().is_some(),
                succeeded
            );
            let reopened =
                UserStore::new(root.path().join("users.json").to_str().unwrap()).unwrap();
            assert_eq!(
                reopened.login(&user.email, password).unwrap().is_some(),
                succeeded
            );
        }
    }

    #[test]
    fn failed_administrator_registration_can_retry_as_first_account() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("users.json");
        std::fs::write(&path, b"[]").unwrap();
        let store = UserStore::new(path.to_str().unwrap()).unwrap();
        let error = store
            .register_with("Owner", "owner@example.test", "initial-password", |_| {
                anyhow::bail!("fixture database unavailable")
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("Registration was rolled back"), "{error}");
        assert!(error.contains("env permissions"), "{error}");
        assert!(store.get_user("owner@example.test").is_none());
        let reopened = UserStore::new(path.to_str().unwrap()).unwrap();
        assert!(reopened.get_user("owner@example.test").is_none());
        let admin = store
            .register_with(
                "Owner",
                "owner@example.test",
                "initial-password",
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(admin.role, "admin");
        let normal = store
            .register_with("Next", "next@example.test", "another-password", |_| {
                panic!("normal registration changed the database password")
            })
            .unwrap();
        assert_eq!(normal.role, "normal");
    }

    #[test]
    fn registration_recovery_failure_reports_both_errors() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("users.json");
        std::fs::write(&path, b"[]").unwrap();
        let store = UserStore::new(path.to_str().unwrap()).unwrap();
        let error = store
            .register_with("Owner", "owner@example.test", "initial-password", |_| {
                std::fs::remove_file(&path).unwrap();
                std::fs::create_dir(&path).unwrap();
                anyhow::bail!("fixture database unavailable")
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("fixture database unavailable"), "{error}");
        assert!(
            error.contains("account file recovery also failed"),
            "{error}"
        );
        assert!(!error.contains("Registration was rolled back"), "{error}");
        assert!(store.get_user("owner@example.test").is_none());
    }

    #[test]
    fn concurrent_registration_retries_first_administrator_after_sync_failure() {
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("users.json");
        std::fs::write(&path, b"[]").unwrap();
        let store = Arc::new(UserStore::new(path.to_str().unwrap()).unwrap());
        let barrier = Arc::new(Barrier::new(2));
        let attempts = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = ["first@example.test", "second@example.test"]
            .into_iter()
            .map(|email| {
                let store = store.clone();
                let barrier = barrier.clone();
                let attempts = attempts.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.register_with("Owner", email, "initial-password", |_| {
                        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                            anyhow::bail!("fixture first synchronization failed");
                        }
                        Ok(())
                    })
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let registered = results.into_iter().find_map(Result::ok).unwrap();
        assert_eq!(registered.role, "admin");
        let reopened = UserStore::new(path.to_str().unwrap()).unwrap();
        assert_eq!(reopened.list_users().len(), 1);
        assert_eq!(reopened.get_user(&registered.email).unwrap().role, "admin");
    }

    /// The operator requirement: no preset admin account. The first registration
    /// becomes the administrator; every later sign-up is a normal user.
    #[test]
    fn first_registration_becomes_the_administrator() {
        let path = temp_path("first-admin");
        std::fs::write(&path, "[]").unwrap();
        let store = UserStore::new(&path).unwrap();
        assert!(store.list_users().is_empty());
        let first = store
            .register("First Owner", "owner@example.com", "correct horse battery")
            .unwrap();
        assert_eq!(first.role, "admin");
        let second = store
            .register("Second", "second@example.com", "another correct horse")
            .unwrap();
        assert_eq!(second.role, "normal");
        // A rollback after failed provisioning removes only the just-created account.
        store.discard_user(&second.id).unwrap();
        assert_eq!(store.list_users().len(), 1);
    }

    /// The operator requirement: no preset admin account, even when a deployment
    /// inherits the legacy RAYRAG_ADMIN_* variables from an older environment.
    #[test]
    fn legacy_admin_seed_variables_never_create_an_account() {
        let path = temp_path("legacy-admin-seed");
        std::fs::remove_file(&path).ok();
        let previous: Vec<(&str, Option<String>)> = [
            "RAYRAG_ADMIN_EMAIL",
            "RAYRAG_ADMIN_NICKNAME",
            "RAYRAG_ADMIN_PASSWORD",
        ]
        .into_iter()
        .map(|key| (key, std::env::var(key).ok()))
        .collect();
        // SAFETY: this test only mutates variables that the store no longer reads;
        // they are restored before returning.
        unsafe {
            std::env::set_var("RAYRAG_ADMIN_EMAIL", "admin");
            std::env::set_var("RAYRAG_ADMIN_NICKNAME", "admin");
            std::env::set_var("RAYRAG_ADMIN_PASSWORD", "preset-admin-password");
        }
        let store = UserStore::new(&path).unwrap();
        assert!(
            store.list_users().is_empty(),
            "legacy RAYRAG_ADMIN_* variables must not seed an account"
        );
        let first = store
            .register("First Owner", "owner@example.com", "correct horse battery")
            .unwrap();
        assert_eq!(first.role, "admin");
        unsafe {
            for (key, value) in previous {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        std::fs::remove_file(path).ok();
    }

    fn encrypt_ragflow_password(public_key_pem: &str, password: &str) -> String {
        let public_key = RsaPublicKey::from_public_key_pem(public_key_pem).unwrap();
        let inner = STANDARD.encode(password);
        let encrypted = public_key
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, inner.as_bytes())
            .unwrap();
        STANDARD.encode(encrypted)
    }

    #[test]
    fn encrypted_ragflow_passwords_reach_all_user_store_auth_boundaries() {
        let path = temp_path("rsa-password-transport");
        std::fs::write(&path, b"[]").unwrap();
        let private_key = RsaPrivateKey::new(&mut OsRng, 2_048).unwrap();
        let private_pem = private_key.to_pkcs8_pem(LineEnding::LF).unwrap();
        let transport = Arc::new(
            crate::password_transport::PasswordTransport::from_private_key_pem(
                private_pem.as_str(),
            )
            .unwrap(),
        );
        let public_key = transport.public_key_pem().to_owned();
        let store = UserStore::new_with_password_transport(&path, Some(transport)).unwrap();

        let initial = encrypt_ragflow_password(&public_key, "correct horse battery staple");
        let user = store
            .register("RSA User", "rsa@example.com", &initial)
            .unwrap();
        assert!(store.login("rsa@example.com", &initial).unwrap().is_some());

        let next = encrypt_ragflow_password(&public_key, "another correct horse battery staple");
        store.change_password(&user.id, &initial, &next).unwrap();
        assert!(
            store
                .login("rsa@example.com", "another correct horse battery staple")
                .unwrap()
                .is_some()
        );
        assert_eq!(store.password_public_key(), Some(public_key.as_str()));
        std::fs::remove_file(path).ok();
    }

    /// `register` stores the address lower-cased, so a lookup that used the
    /// typed address verbatim locked out anyone who signed up as
    /// `Ada@Example.com`: the account existed under `ada@example.com` and the
    /// login answered "Invalid credentials" forever. Upstream compares through
    /// MySQL's case-insensitive collation and never sees the difference.
    #[test]
    fn email_lookup_ignores_case_and_surrounding_space() {
        let path = temp_path("email-case");
        let seed = User {
            id: "seed-user".into(),
            nickname: "Seed".into(),
            email: "seed@example.com".into(),
            password_hash: hash_password("correct horse battery staple").unwrap(),
            role: "admin".into(),
            avatar: String::new(),
            timezone: String::new(),
            created_at: now_ms(),
        };
        std::fs::write(&path, serde_json::to_vec(&vec![seed]).unwrap()).unwrap();
        let store = UserStore::new(&path).unwrap();
        let user = store
            .register("Ada", "Ada@Example.com", "correct horse battery staple")
            .unwrap();
        assert_eq!(
            user.email, "ada@example.com",
            "writes normalise the address"
        );

        // Every spelling of the same mailbox reaches the same account.
        for typed in [
            "ada@example.com",
            "Ada@Example.com",
            "ADA@EXAMPLE.COM",
            "  ada@example.com  ",
        ] {
            assert!(
                store
                    .login(typed, "correct horse battery staple")
                    .unwrap()
                    .is_some(),
                "login must accept {typed}"
            );
            assert_eq!(
                store.get_user(typed).map(|user| user.id),
                Some(user.id.clone()),
                "get_user must resolve {typed}"
            );
        }
        // A genuinely different mailbox still misses.
        assert!(store.get_user("ada@example.org").is_none());
        assert!(
            store
                .login("ada@example.org", "correct horse battery staple")
                .unwrap()
                .is_none()
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn legacy_password_is_migrated_and_token_expires_normally() {
        let path = temp_path("legacy-auth");
        let user = User {
            id: "legacy-user".into(),
            nickname: "Legacy".into(),
            email: "legacy@example.com".into(),
            password_hash: legacy_sha256("correct horse battery staple"),
            role: "admin".into(),
            avatar: String::new(),
            timezone: String::new(),
            created_at: now_ms(),
        };
        std::fs::write(&path, serde_json::to_vec(&vec![user]).unwrap()).unwrap();

        let store = UserStore::new(&path).unwrap();
        assert!(
            store
                .login("legacy@example.com", "wrong")
                .unwrap()
                .is_none()
        );
        let token = store
            .login("legacy@example.com", "correct horse battery staple")
            .unwrap()
            .unwrap();
        assert_eq!(store.validate_token(&token).as_deref(), Some("legacy-user"));
        assert!(
            store
                .get_user("legacy@example.com")
                .unwrap()
                .password_hash
                .starts_with("$argon2id$")
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("$argon2id$")
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn rejects_malformed_or_unknown_tokens() {
        let path = temp_path("token-auth");
        let user = User {
            id: "token-user".into(),
            nickname: "Token".into(),
            email: "token@example.com".into(),
            password_hash: hash_password("correct horse battery staple").unwrap(),
            role: "normal".into(),
            avatar: String::new(),
            timezone: String::new(),
            created_at: now_ms(),
        };
        std::fs::write(&path, serde_json::to_vec(&vec![user]).unwrap()).unwrap();
        let store = UserStore::new(&path).unwrap();
        assert!(store.validate_token("").is_none());
        assert!(store.validate_token("not-a-real-token").is_none());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn failed_persistence_rolls_back_user_registration() {
        let path = temp_path("user-rollback");
        let user = User {
            id: "existing-user".into(),
            nickname: "Existing".into(),
            email: "existing@example.com".into(),
            password_hash: hash_password("correct horse battery staple").unwrap(),
            role: "normal".into(),
            avatar: String::new(),
            timezone: String::new(),
            created_at: now_ms(),
        };
        std::fs::write(&path, serde_json::to_vec(&vec![user]).unwrap()).unwrap();
        let store = UserStore::new(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            store
                .register(
                    "New User",
                    "new@example.com",
                    "another correct horse battery staple"
                )
                .is_err()
        );
        assert!(store.get_user("new@example.com").is_none());
        assert!(store.get_user("existing@example.com").is_some());
        std::fs::remove_dir_all(path).ok();
    }
}
