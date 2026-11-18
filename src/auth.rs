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
            // No account is pre-created. The first registration becomes the RayRAG
            // administrator, so a fresh deployment no longer starts with a well-known
            // admin/admin account and no random password is generated. An operator who
            // explicitly configures the legacy seed variables still gets that account.
            let configured_email = std::env::var("RAYRAG_ADMIN_EMAIL")
                .ok()
                .filter(|value| !value.trim().is_empty());
            let configured_password = std::env::var("RAYRAG_ADMIN_PASSWORD")
                .ok()
                .filter(|value| !value.trim().is_empty());
            if configured_email.is_none() && configured_password.is_none() {
                HashMap::new()
            } else {
                let initial_email = configured_email.unwrap_or_else(|| "admin".into());
                let initial_nickname = std::env::var("RAYRAG_ADMIN_NICKNAME")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "admin".into());
                let initial_password = configured_password.ok_or_else(|| {
                    anyhow::anyhow!(
                        "RAYRAG_ADMIN_EMAIL is set but RAYRAG_ADMIN_PASSWORD is not; set both to seed the account"
                    )
                })?;
                let admin = User {
                    id: uuid::Uuid::new_v4().to_string(),
                    nickname: initial_nickname,
                    email: initial_email,
                    password_hash: hash_password(&initial_password)?,
                    role: "admin".into(),
                    avatar: String::new(),
                    timezone: String::new(),
                    created_at: now_ms(),
                };
                let mut map = HashMap::new();
                map.insert(admin.email.clone(), admin);
                map
            }
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


