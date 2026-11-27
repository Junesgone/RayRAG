//! Crash-resistant local file persistence helpers.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    sync::OnceLock,
};

pub trait SnapshotMirror: Send + Sync {
    fn store(&self, path: &Path, data: &[u8]) -> anyhow::Result<()>;
    fn load(&self, path: &Path) -> anyhow::Result<Option<Vec<u8>>>;
    fn health(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

static SNAPSHOT_MIRROR: OnceLock<anyhow::Result<Option<Box<dyn SnapshotMirror>>>> = OnceLock::new();

/// Atomically replace a file and fsync both file content and the parent directory.
pub fn atomic_write(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let mirror = snapshot_mirror()?;
    atomic_write_with_mirror(path, data, mirror)
}

fn atomic_write_with_mirror(
    path: &Path,
    data: &[u8],
    mirror: Option<&dyn SnapshotMirror>,
) -> anyhow::Result<()> {
    let previous = match std::fs::read(path) {
        Ok(previous) => Some(previous),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    atomic_write_local(path, data)?;
    if let Some(mirror) = mirror
        && let Err(error) = mirror.store(path, data)
    {
        let rollback = match previous {
            Some(previous) => atomic_write_local(path, &previous),
            None => match std::fs::remove_file(path) {
                Ok(()) => sync_parent(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            },
        };
        if let Err(rollback_error) = rollback {
            tracing::error!(
                path = %path.display(), %error, %rollback_error,
                "Snapshot mirror write and local rollback both failed"
            );
            anyhow::bail!(
                "snapshot mirror write failed: {error:#}; local rollback failed for {}: {rollback_error:#}. Check state directory permissions and storage, then repair the local snapshot before retrying.",
                path.display()
            );
        }
        return Err(error);
    }
    Ok(())
}

fn atomic_write_local(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        file.write_all(data)?;
        // 容器 overlay 文件系统上 fsync 极慢（~10s/次）。RAYRAG_NO_FSYNC=1
        // 跳过 fsync 换取吞吐（崩溃时最多丢最后一次快照，JSON 可重建）。
        if std::env::var("RAYRAG_NO_FSYNC").is_err() {
            file.sync_all()?;
        }
        drop(file);
        std::fs::rename(&tmp, path)?;
        if std::env::var("RAYRAG_NO_FSYNC").is_err() {
            sync_parent(path)?;
        }
        Ok(())
    })();
    if result.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    result
}

/// Restore a missing local state file from the configured PostgreSQL mirror.
pub fn restore_if_missing(path: &Path) -> anyhow::Result<bool> {
    restore_if_missing_with_mirror(path, snapshot_mirror()?)
}

/// Atomically persist a serializable value as pretty JSON (mirror-aware).
///
/// Reuses [`atomic_write`] so a configured PostgreSQL snapshot mirror stores
/// the same payload that lands on disk, and a mirror failure rolls the local
/// file back. Used by the store layer for Infinity-style table lifecycle
/// state (per-table vector dimension registries).
pub fn save_json<T: serde::Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let data = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &data)
}

/// Load a deserializable value from a JSON state file.
///
/// A missing file yields `Ok(None)` (fresh-state semantics); corrupt JSON is
/// an error so callers never silently rebuild over a damaged snapshot.
pub fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(data) => Ok(Some(serde_json::from_slice(&data)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Load a JSON state file that must never be read as "no data".
///
/// The failure this guards against is quiet and destructive: a corrupt or old-schema
/// file deserialises to an empty collection, the page shows nothing, and the next save
/// writes that emptiness over the real records. Refusing to boot would be the other
/// extreme — one damaged side-file must not take the whole service down.
///
/// So the damaged file is **kept** (moved to `<path>.corrupt-<timestamp>`), the reason is
/// logged at error level, and the caller starts from the type's default. Nothing is
/// destroyed and nothing is silent.
pub fn load_json_or_quarantine<T>(path: &Path) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned + Default,
{
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(error.into()),
    };
    match serde_json::from_slice::<T>(&data) {
        Ok(value) => Ok(value),
        Err(error) => {
            let backup = path.with_extension(format!(
                "json.corrupt-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis())
                    .unwrap_or_default()
            ));
            tracing::error!(
                %error,
                path = %path.display(),
                backup = %backup.display(),
                "State file is not readable as this schema; keeping it as a backup and starting empty"
            );
            if let Err(rename_error) = std::fs::rename(path, &backup) {
                tracing::error!(
                    %rename_error,
                    path = %path.display(),
                    "Failed to move the damaged state file aside; it will be overwritten on the next save"
                );
            }
            Ok(T::default())
        }
    }
}

/// Probe the configured PostgreSQL snapshot mirror. `Ok(false)` means the
/// optional mirror is disabled; an enabled but unreachable mirror is an error.
pub fn snapshot_mirror_health() -> anyhow::Result<bool> {
    let Some(mirror) = snapshot_mirror()? else {
        return Ok(false);
    };
    mirror.health()?;
    Ok(true)
}

fn restore_if_missing_with_mirror(
    path: &Path,
    mirror: Option<&dyn SnapshotMirror>,
) -> anyhow::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    let Some(mirror) = mirror else {
        return Ok(false);
    };
    let Some(data) = mirror.load(path)? else {
        return Ok(false);
    };
    atomic_write_local(path, &data)?;
    Ok(true)
}

fn snapshot_mirror() -> anyhow::Result<Option<&'static dyn SnapshotMirror>> {
    let mirror = SNAPSHOT_MIRROR.get_or_init(configure_snapshot_mirror);
    match mirror {
        Ok(Some(mirror)) => Ok(Some(mirror.as_ref())),
        Ok(None) => Ok(None),
        Err(error) => anyhow::bail!(error.to_string()),
    }
}

fn configure_snapshot_mirror() -> anyhow::Result<Option<Box<dyn SnapshotMirror>>> {
    #[cfg(feature = "postgres-backend")]
    {
        postgres_backend::from_env()
            .map(|mirror| mirror.map(|mirror| Box::new(mirror) as Box<dyn SnapshotMirror>))
    }
    #[cfg(not(feature = "postgres-backend"))]
    {
        if std::env::var("RAYRAG_POSTGRES_URL")
            .ok()
            .is_some_and(|url| !url.trim().is_empty())
        {
            anyhow::bail!(
                "RAYRAG_POSTGRES_URL is set but RayRAG was built without --features postgres-backend"
            );
        }
        Ok(None)
    }
}

/// A DSN safe to print in a log or an error message.
///
/// Connection failures are the most common first-deployment error, so the message has
/// to name the endpoint — but a startup banner or an error report must never leak the
/// database password.
pub fn redact_dsn(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return "<unparsable DSN>".to_string();
    };
    let (scheme, rest) = url.split_at(scheme_end + 3);
    let Some(at) = rest.rfind('@') else {
        return format!("{scheme}{rest}");
    };
    let (credentials, host) = rest.split_at(at);
    match credentials.split_once(':') {
        Some((user, _)) => format!("{scheme}{user}:***{host}"),
        None => format!("{scheme}{credentials}{host}"),
    }
}

/// Where the rotated PostgreSQL DSN lives. It sits inside the state volume so a
/// container re-create keeps the credential even though the compose environment
/// still carries the old password; startup applies it over the process env.
pub const POSTGRES_CREDENTIALS_FILE: &str = "pg_credentials.json";

pub fn postgres_credentials_path(static_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(static_dir).join(POSTGRES_CREDENTIALS_FILE)
}

/// Persist the rotated DSN after ALTER ROLE, with owner-only permissions.
pub fn persist_postgres_url(static_dir: &str, url: &str) -> anyhow::Result<()> {
    let path = postgres_credentials_path(static_dir);
    let payload = serde_json::to_vec(&serde_json::json!({ "url": url }))?;
    atomic_write(&path, &payload)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Apply a persisted rotated DSN before any store opens its connection. An
/// explicit compose environment value is intentionally overridden: the saved
/// credential is the one that matches the live database role.
pub fn apply_persisted_postgres_url(static_dir: &str) -> anyhow::Result<()> {
    let path = postgres_credentials_path(static_dir);
    let url = load_postgres_url(&path).map_err(|error| {
        anyhow::anyhow!(
            "Cannot load the saved PostgreSQL credential at {}: {error}. Restore this file from a trusted backup or repair its JSON url field to match the current database credential, then restart RayRAG. Keep the password private.",
            path.display()
        )
    })?;
    let Some(url) = url else {
        return Ok(());
    };
    unsafe { std::env::set_var("RAYRAG_POSTGRES_URL", url) };
    tracing::info!(
        "applied the rotated PostgreSQL credential saved at {}",
        path.display()
    );
    Ok(())
}

fn load_postgres_url(path: &Path) -> anyhow::Result<Option<String>> {
    let payload = match std::fs::read(path) {
        Ok(payload) => payload,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // Serde errors can include the input value. Credential diagnostics must
    // describe the schema without reproducing any part of the saved secret.
    let value: serde_json::Value = serde_json::from_slice(&payload)
        .map_err(|_| anyhow::anyhow!("the credential file must contain valid UTF-8 JSON"))?;
    let url = value
        .get("url")
        .and_then(serde_json::Value::as_str)
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("the credential file requires a nonempty string url field")
        })?;
    let parsed = url::Url::parse(url)
        .map_err(|_| anyhow::anyhow!("the saved url is not a valid PostgreSQL connection URL"))?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") || parsed.username().is_empty() {
        anyhow::bail!(
            "the saved url must use postgres:// or postgresql:// and include a database user"
        );
    }
    Ok(Some(url.to_string()))
}
/// Persist both connection files before committing the role password change.
/// File failures roll back the database transaction and restore previous files.
#[cfg(feature = "postgres-backend")]
pub fn sync_postgres_admin_password(
    new_password: &str,
    static_dir: &str,
    env_path: &Path,
) -> anyhow::Result<Option<String>> {
    // The DSN, file backups and commit must use one credential generation.
    static ROTATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = ROTATION_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("the PostgreSQL credential rotation lock is poisoned"))?;
    let Some(url) = std::env::var("RAYRAG_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let updated = sync_postgres_admin_password_at(&url, new_password, static_dir, env_path)?;
    unsafe { std::env::set_var("RAYRAG_POSTGRES_URL", &updated) };
    Ok(Some(updated))
}

#[cfg(feature = "postgres-backend")]
fn sync_postgres_admin_password_at(
    url: &str,
    new_password: &str,
    static_dir: &str,
    env_path: &Path,
) -> anyhow::Result<String> {
    if new_password.len() < 6 || new_password.bytes().any(|byte| byte == 0) {
        anyhow::bail!(
            "the new password must be at least 6 characters and cannot contain NUL bytes"
        );
    }
    let mut parsed = url::Url::parse(url)
        .map_err(|_| anyhow::anyhow!("RAYRAG_POSTGRES_URL is not a valid PostgreSQL URL"))?;
    if parsed.username().is_empty() {
        anyhow::bail!("RAYRAG_POSTGRES_URL has no user; cannot sync the administrator password");
    }
    parsed
        .set_password(Some(
            &url::form_urlencoded::byte_serialize(new_password.as_bytes())
                .collect::<String>()
                .replace('+', "%20"),
        ))
        .map_err(|_| anyhow::anyhow!("cannot rewrite the password in RAYRAG_POSTGRES_URL"))?;
    let updated = parsed.to_string();
    let credential_path = postgres_credentials_path(static_dir);
    let credential_backup = ConnectionFileBackup::read(&credential_path)?;
    let env_backup = ConnectionFileBackup::read(env_path)?;
    let mut client = postgres::Client::connect(url, postgres::NoTls).map_err(|error| {
        anyhow::anyhow!(
            "cannot connect to PostgreSQL at {} to sync the administrator password: {error}",
            redact_dsn(url)
        )
    })?;
    let result = (|| -> anyhow::Result<()> {
        let mut transaction = client.transaction()?;
        // PostgreSQL quotes the authenticated role and password, including
        // encoded usernames, apostrophes and nonstandard string settings.
        let statement: String = transaction
            .query_one(
                "SELECT format('ALTER ROLE %I PASSWORD %L', current_user, $1::text)",
                &[&new_password],
            )?
            .get(0);
        transaction.batch_execute(&statement)?;
        persist_postgres_url(static_dir, &updated)?;
        crate::api::setup::write_env_file(
            env_path,
            &[("RAYRAG_POSTGRES_URL".to_string(), updated.clone())],
        )?;
        transaction.commit()?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut recovery_errors = Vec::new();
        for backup in [&credential_backup, &env_backup] {
            if let Err(recovery_error) = backup.restore() {
                recovery_errors.push(recovery_error.to_string());
            }
        }
        if !recovery_errors.is_empty() {
            anyhow::bail!(
                "PostgreSQL credential synchronization failed: {error}. Connection file recovery also failed: {}. Restore the saved credential and env from a trusted backup before restarting.",
                recovery_errors.join("; ")
            );
        }
        anyhow::bail!(
            "PostgreSQL credential synchronization failed: {error}; connection files restored"
        );
    }
    Ok(updated)
}

#[cfg(not(feature = "postgres-backend"))]
pub fn sync_postgres_admin_password(
    _new_password: &str,
    _static_dir: &str,
    _env_path: &Path,
) -> anyhow::Result<Option<String>> {
    Ok(None)
}

#[cfg(feature = "postgres-backend")]
struct ConnectionFileBackup {
    path: std::path::PathBuf,
    content: Option<Vec<u8>>,
    permissions: Option<std::fs::Permissions>,
}

#[cfg(feature = "postgres-backend")]
impl ConnectionFileBackup {
    fn read(path: &Path) -> anyhow::Result<Self> {
        let content = match std::fs::read(path) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let permissions = if content.is_some() {
            Some(std::fs::metadata(path)?.permissions())
        } else {
            None
        };
        Ok(Self {
            path: path.to_path_buf(),
            content,
            permissions,
        })
    }

    fn restore(&self) -> anyhow::Result<()> {
        if Self::read(&self.path)?.content == self.content {
            return Ok(());
        }
        match &self.content {
            Some(content) => {
                if let Err(error) = atomic_write(&self.path, content) {
                    // Environment files can be mounted individually by Docker.
                    if error
                        .downcast_ref::<std::io::Error>()
                        .and_then(|error| error.raw_os_error())
                        != Some(16)
                    {
                        return Err(error);
                    }
                    let mut file = OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(&self.path)?;
                    file.write_all(content)?;
                    file.sync_all()?;
                }
                if let Some(permissions) = &self.permissions {
                    std::fs::set_permissions(&self.path, permissions.clone())?;
                }
            }
            None => {
                std::fs::remove_file(&self.path)?;
                sync_parent(&self.path)?;
            }
        }
        Ok(())
    }
}
/// Fsync a directory after creating, renaming, or removing an entry in it.
pub fn sync_parent(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(feature = "postgres-backend")]
mod postgres_backend {
    use super::SnapshotMirror;
    use postgres::{Client, NoTls};
    use std::path::Path;
    use std::sync::mpsc;

    const CREATE_TABLE: &str = r#"
        CREATE TABLE IF NOT EXISTS rayrag_state_snapshots (
            path_key TEXT PRIMARY KEY,
            payload BYTEA NOT NULL,
            checksum TEXT NOT NULL,
            updated_at BIGINT NOT NULL
        )
    "#;

    pub struct PostgresSnapshotMirror {
        requests: mpsc::Sender<DatabaseRequest>,
        namespace: String,
    }

    enum DatabaseRequest {
        Store {
            path_key: String,
            payload: Vec<u8>,
            response: mpsc::Sender<anyhow::Result<()>>,
        },
        Load {
            path_key: String,
            response: mpsc::Sender<anyhow::Result<Option<Vec<u8>>>>,
        },
        Health {
            response: mpsc::Sender<anyhow::Result<()>>,
        },
    }

    pub fn from_env() -> anyhow::Result<Option<PostgresSnapshotMirror>> {
        let Some(url) = std::env::var("RAYRAG_POSTGRES_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
        else {
            return Ok(None);
        };
        let required_version = std::env::var("RAYRAG_POSTGRES_REQUIRED_VERSION")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let namespace = std::env::var("RAYRAG_POSTGRES_NAMESPACE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "default".into());
        start_mirror(url, required_version, namespace).map(Some)
    }

    fn start_mirror(
        url: String,
        required_version: Option<String>,
        namespace: String,
    ) -> anyhow::Result<PostgresSnapshotMirror> {
        let (requests, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("rayrag-postgres-snapshot".into())
            .spawn(move || {
                let result =
                    run_database_worker(&url, required_version.as_deref(), receiver, &ready_tx);
                if let Err(error) = result {
                    let _ = ready_tx.try_send(Err(error));
                }
            })?;
        ready_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("PostgreSQL snapshot worker stopped during startup"))??;
        Ok(PostgresSnapshotMirror {
            requests,
            namespace,
        })
    }

    fn run_database_worker(
        url: &str,
        required_version: Option<&str>,
        receiver: mpsc::Receiver<DatabaseRequest>,
        ready: &mpsc::SyncSender<anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        let mut client = Client::connect(url, NoTls).map_err(|error| {
            // The driver's own message is just "db error": name the endpoint and say what
            // to do, otherwise a first deployment fails with nothing to act on.
            anyhow::anyhow!(
                "cannot connect to PostgreSQL at {}: {error}. Start the bundled database \
                 (`docker compose up -d postgres`), or point RAYRAG_POSTGRES_URL at a \
                 reachable server, or leave RAYRAG_POSTGRES_URL empty to use the built-in \
                 file store.",
                super::redact_dsn(url)
            )
        })?;
        let version_num: i32 = client
            .query_one("SELECT current_setting('server_version_num')::int4", &[])?
            .get(0);
        let version: String = client.query_one("SHOW server_version", &[])?.get(0);
        if let Some(required) = required_version
            && !version.starts_with(required)
        {
            anyhow::bail!("PostgreSQL version mismatch: server={version}, required={required}");
        }
        if version_num / 10_000 != 18 {
            anyhow::bail!("Unsupported PostgreSQL server version: {version}; expected 18.x");
        }
        client.batch_execute(CREATE_TABLE)?;
        ready
            .send(Ok(()))
            .map_err(|_| anyhow::anyhow!("PostgreSQL snapshot startup receiver stopped"))?;
        for request in receiver {
            match request {
                DatabaseRequest::Store {
                    path_key,
                    payload,
                    response,
                } => {
                    // Retry deadlock/serialization aborts with exponential
                    // backoff (mirrors RAGFlow retry_deadlock_operation).
                    let _ = response.send(retry_pg_write(|| {
                        store_snapshot(&mut client, &path_key, &payload)
                    }));
                }
                DatabaseRequest::Load { path_key, response } => {
                    let _ = response.send(load_snapshot(&mut client, &path_key));
                }
                DatabaseRequest::Health { response } => {
                    let result = client
                        .simple_query("SELECT 1")
                        .map(|_| ())
                        .map_err(anyhow::Error::from);
                    let _ = response.send(result);
                }
            }
        }
        Ok(())
    }

    impl SnapshotMirror for PostgresSnapshotMirror {
        fn store(&self, path: &Path, data: &[u8]) -> anyhow::Result<()> {
            let path_key = snapshot_key(&self.namespace, path);
            let (response, result) = mpsc::channel();
            self.requests
                .send(DatabaseRequest::Store {
                    path_key,
                    payload: data.to_vec(),
                    response,
                })
                .map_err(|_| anyhow::anyhow!("PostgreSQL snapshot worker stopped"))?;
            result
                .recv()
                .map_err(|_| anyhow::anyhow!("PostgreSQL snapshot worker dropped response"))?
        }

        fn load(&self, path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
            let path_key = snapshot_key(&self.namespace, path);
            let (response, result) = mpsc::channel();
            self.requests
                .send(DatabaseRequest::Load { path_key, response })
                .map_err(|_| anyhow::anyhow!("PostgreSQL snapshot worker stopped"))?;
            result
                .recv()
                .map_err(|_| anyhow::anyhow!("PostgreSQL snapshot worker dropped response"))?
        }

        fn health(&self) -> anyhow::Result<()> {
            let (response, result) = mpsc::channel();
            self.requests
                .send(DatabaseRequest::Health { response })
                .map_err(|_| anyhow::anyhow!("PostgreSQL snapshot worker stopped"))?;
            result
                .recv_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| anyhow::anyhow!("PostgreSQL health probe timed out: {error}"))?
        }
    }

    fn store_snapshot(client: &mut Client, path_key: &str, data: &[u8]) -> anyhow::Result<()> {
        let mut transaction = client.transaction()?;
        transaction.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            &[&path_key],
        )?;
        transaction.execute(
            r#"
                INSERT INTO rayrag_state_snapshots
                    (path_key, payload, checksum, updated_at)
                VALUES
                    ($1, $2, $3, $4)
                ON CONFLICT (path_key) DO UPDATE SET
                    payload = EXCLUDED.payload,
                    checksum = EXCLUDED.checksum,
                    updated_at = EXCLUDED.updated_at
            "#,
            &[&path_key, &data, &checksum(data), &now_ms()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn load_snapshot(client: &mut Client, path_key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let row = client.query_opt(
            "SELECT payload, checksum FROM rayrag_state_snapshots WHERE path_key = $1",
            &[&path_key],
        )?;
        let Some(row) = row else {
            return Ok(None);
        };
        let payload: Vec<u8> = row.get(0);
        let expected_checksum: String = row.get(1);
        let actual_checksum = checksum(&payload);
        if actual_checksum != expected_checksum {
            anyhow::bail!(
                "PostgreSQL snapshot checksum mismatch for {path_key}: expected {expected_checksum}, got {actual_checksum}"
            );
        }
        Ok(Some(payload))
    }

    fn snapshot_key(namespace: &str, path: &Path) -> String {
        format!("{namespace}:{}", path.display())
    }

    fn checksum(data: &[u8]) -> String {
        format!("{:016x}", xxhash_rust::xxh3::xxh3_64(data))
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    /// True when the SQLSTATE is a retryable PostgreSQL failure: 40P01
    /// (deadlock_detected) or 40001 (serialization_failure). Mirrors
    /// RAGFlow `_is_deadlock_error` (common_service.py) which retries
    /// MySQL/OceanBase errorcode 1213.
    fn is_retryable_sqlstate(code: &str) -> bool {
        code == "40P01" || code == "40001"
    }

    /// True when the error is a retryable PostgreSQL failure (deadlock or
    /// serialization failure).
    fn is_retryable_pg_error(error: &anyhow::Error) -> bool {
        let Some(pg_error) = error.downcast_ref::<postgres::Error>() else {
            return false;
        };
        pg_error
            .code()
            .map(|code| is_retryable_sqlstate(code.code()))
            .unwrap_or(false)
    }

    /// Retry a full snapshot write with exponential backoff when PostgreSQL
    /// aborts the transaction with a deadlock or serialization failure.
    /// Mirrors RAGFlow `retry_deadlock_operation` (3 attempts, 0.1s base).
    fn retry_pg_write<F>(operation: F) -> anyhow::Result<()>
    where
        F: FnMut() -> anyhow::Result<()>,
    {
        retry_pg_write_with(operation, is_retryable_pg_error)
    }

    /// Predicate-injected retry loop (the production predicate cannot be
    /// unit-tested with a real postgres::Error because its constructors are
    /// crate-private, so the loop itself is tested with a stub predicate).
    fn retry_pg_write_with<F, P>(mut operation: F, is_retryable: P) -> anyhow::Result<()>
    where
        F: FnMut() -> anyhow::Result<()>,
        P: Fn(&anyhow::Error) -> bool,
    {
        let mut delay = 0.1f64;
        for attempt in 0..3 {
            match operation() {
                Ok(ok) => return Ok(ok),
                Err(error) => {
                    if !is_retryable(&error) || attempt >= 2 {
                        return Err(error);
                    }
                    tracing::warn!(
                        "postgres snapshot write aborted by deadlock/serialization, retrying ({}/3): {error}",
                        attempt + 1
                    );
                    std::thread::sleep(std::time::Duration::from_secs_f64(delay));
                    delay *= 2.0;
                }
            }
        }
        unreachable!("retry loop always returns")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn retryable_pg_error_classification() {
            assert!(is_retryable_sqlstate("40P01"));
            assert!(is_retryable_sqlstate("40001"));
            assert!(!is_retryable_sqlstate("23505"));
            assert!(!is_retryable_pg_error(&anyhow::anyhow!("io error")));
        }

        #[test]
        fn retry_pg_write_succeeds_after_transient_abort() {
            let mut attempts = 0;
            let result = retry_pg_write_with(
                || {
                    attempts += 1;
                    if attempts == 1 {
                        Err(anyhow::anyhow!("simulated 40P01"))
                    } else {
                        Ok(())
                    }
                },
                |error| error.to_string().contains("40P01"),
            );
            assert!(result.is_ok());
            assert_eq!(attempts, 2);
        }

        #[test]
        fn retry_pg_write_gives_up_after_three_attempts() {
            let mut attempts = 0;
            let result = retry_pg_write_with(
                || {
                    attempts += 1;
                    Err(anyhow::anyhow!("simulated 40001"))
                },
                |error| error.to_string().contains("40001"),
            );
            assert!(result.is_err());
            assert_eq!(attempts, 3);
        }

        #[test]
        fn retry_pg_write_does_not_retry_non_retryable_errors() {
            let mut attempts = 0;
            let result = retry_pg_write_with(
                || {
                    attempts += 1;
                    Err(anyhow::anyhow!("unique violation"))
                },
                |error| error.to_string().contains("40P01"),
            );
            assert!(result.is_err());
            assert_eq!(attempts, 1);
        }

        #[test]
        fn postgres_18_snapshot_round_trip() {
            let Some(url) = std::env::var("RAYRAG_POSTGRES_TEST_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
            else {
                return;
            };
            let namespace = format!("test-{}", uuid::Uuid::new_v4());
            let mirror = start_mirror(url.clone(), Some("18.7".into()), namespace.clone()).unwrap();
            let path = Path::new("web/postgres-smoke.json");
            mirror.store(path, b"postgres-18.7-snapshot").unwrap();
            assert_eq!(
                mirror.load(path).unwrap().unwrap(),
                b"postgres-18.7-snapshot"
            );

            let path_key = snapshot_key(&namespace, path);
            drop(mirror);
            Client::connect(&url, NoTls)
                .unwrap()
                .execute(
                    "DELETE FROM rayrag_state_snapshots WHERE path_key = $1",
                    &[&path_key],
                )
                .unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeMirror {
        fail_store: bool,
        payload: Mutex<Option<Vec<u8>>>,
    }

    impl SnapshotMirror for FakeMirror {
        fn store(&self, _path: &Path, data: &[u8]) -> anyhow::Result<()> {
            if self.fail_store {
                anyhow::bail!("mirror unavailable");
            }
            *self.payload.lock().unwrap() = Some(data.to_vec());
            Ok(())
        }

        fn load(&self, _path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(self.payload.lock().unwrap().clone())
        }
    }

    #[test]
    fn mirror_failure_restores_previous_local_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-persistence-mirror-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("state.json");
        atomic_write_local(&path, b"old").unwrap();
        let mirror = FakeMirror {
            fail_store: true,
            payload: Mutex::new(None),
        };
        assert!(atomic_write_with_mirror(&path, b"new", Some(&mirror)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_mirror_removes_new_local_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        let mirror = FakeMirror {
            fail_store: true,
            payload: Mutex::new(None),
        };
        let error = atomic_write_with_mirror(&path, b"new", Some(&mirror)).unwrap_err();
        assert!(error.to_string().contains("mirror unavailable"));
        assert!(!path.exists());
    }

    #[test]
    fn mirror_and_local_rollback_errors_are_both_reported() {
        struct RollbackFailureMirror;
        impl SnapshotMirror for RollbackFailureMirror {
            fn store(&self, path: &Path, _data: &[u8]) -> anyhow::Result<()> {
                std::fs::remove_file(path)?;
                std::fs::create_dir(path)?;
                anyhow::bail!("mirror unavailable fixture");
            }
            fn load(&self, _path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
        }
        for existing in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("state.json");
            if existing {
                atomic_write_local(&path, b"old").unwrap();
            }
            let error = atomic_write_with_mirror(&path, b"new", Some(&RollbackFailureMirror))
                .unwrap_err()
                .to_string();
            for detail in [
                "mirror unavailable fixture",
                "local rollback failed",
                "state.json",
                "repair the local snapshot",
            ] {
                assert!(error.contains(detail), "{error}");
            }
            assert!(path.is_dir());
        }
    }

    #[test]
    fn successful_mirror_receives_exact_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-persistence-mirror-success-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("state.json");
        let mirror = FakeMirror {
            fail_store: false,
            payload: Mutex::new(None),
        };
        atomic_write_with_mirror(&path, b"snapshot", Some(&mirror)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"snapshot");
        assert_eq!(mirror.load(&path).unwrap().unwrap(), b"snapshot");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_snapshot_is_restored_but_existing_local_state_wins() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-persistence-restore-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("state.json");
        let mirror = FakeMirror {
            fail_store: false,
            payload: Mutex::new(Some(b"postgres-copy".to_vec())),
        };
        assert!(restore_if_missing_with_mirror(&path, Some(&mirror)).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"postgres-copy");

        atomic_write_local(&path, b"local-newer").unwrap();
        assert!(!restore_if_missing_with_mirror(&path, Some(&mirror)).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"local-newer");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn json_snapshot_round_trips_through_atomic_write() {
        let root =
            std::env::temp_dir().join(format!("rayrag-persistence-json-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("table.json");
        let value = serde_json::json!({"dimensions": [1024, 1536], "kb": "kb-1"});
        save_json(&path, &value).unwrap();
        let loaded: serde_json::Value = load_json(&path).unwrap().unwrap();
        assert_eq!(loaded, value);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn json_loader_treats_missing_file_as_fresh_state() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-persistence-json-missing-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("absent.json");
        let loaded: Option<serde_json::Value> = load_json(&path).unwrap();
        assert!(loaded.is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn quarantined_state_files_are_kept_and_logged_instead_of_read_as_empty() {
        use std::collections::HashMap;
        let root = std::env::temp_dir().join(format!(
            "rayrag-persistence-quarantine-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();

        // Old schema (`[]` where a map is expected): the data must survive the load.
        let path = root.join("chat_apps.json");
        std::fs::write(&path, b"[]").unwrap();
        let loaded: HashMap<String, String> = load_json_or_quarantine(&path).unwrap();
        assert!(loaded.is_empty());
        assert!(!path.exists(), "the damaged file must be moved aside");
        let backups: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains("corrupt-"))
            .collect();
        assert_eq!(backups.len(), 1, "exactly one backup: {backups:?}");
        assert_eq!(
            std::fs::read(root.join(&backups[0])).unwrap(),
            b"[]",
            "the backup keeps the original bytes"
        );

        // A missing file is a fresh deployment, not a fault.
        let fresh: HashMap<String, String> =
            load_json_or_quarantine(&root.join("absent.json")).unwrap();
        assert!(fresh.is_empty());

        // A healthy file is returned untouched, and is not moved aside.
        let healthy = root.join("healthy.json");
        std::fs::write(&healthy, br#"{"a":"1"}"#).unwrap();
        let value: HashMap<String, String> = load_json_or_quarantine(&healthy).unwrap();
        assert_eq!(value.get("a").map(String::as_str), Some("1"));
        assert!(healthy.exists());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    #[cfg(feature = "postgres-backend")]
    fn postgres_password_rotation_validates_before_database_or_file_changes() {
        let root = tempfile::tempdir().unwrap();
        let env_path = root.path().join(".env");
        for password in ["", "short", "long\0password"] {
            let error = sync_postgres_admin_password_at(
                "invalid-url",
                password,
                root.path().to_str().unwrap(),
                &env_path,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("at least 6 characters"), "{error}");
            assert!(!env_path.exists());
            assert!(!postgres_credentials_path(root.path().to_str().unwrap()).exists());
        }
    }

    #[test]
    #[cfg(all(feature = "postgres-backend", unix))]
    #[ignore = "requires a disposable SCRAM PostgreSQL instance and an unprivileged test user"]
    fn postgres_password_rotation_restores_files_and_database_on_env_write_failure() {
        use std::os::unix::fs::PermissionsExt;
        let url = std::env::var("RAYRAG_POSTGRES_ROTATION_TEST_URL")
            .expect("set RAYRAG_POSTGRES_ROTATION_TEST_URL to a disposable database");
        let root = tempfile::tempdir().unwrap();
        let static_dir = root.path().join("static");
        let env_dir = root.path().join("config");
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::create_dir_all(&env_dir).unwrap();
        let env_path = env_dir.join(".env");
        std::fs::write(&env_path, "# retained\nRAYRAG_CHAT_MODEL=test model\n").unwrap();
        let static_dir = static_dir.to_str().unwrap();
        let initial_config: postgres::Config = url.parse().unwrap();
        let initial_password = std::str::from_utf8(initial_config.get_password().unwrap()).unwrap();
        let users_path = root.path().join("users.json");
        std::fs::write(&users_path, b"[]").unwrap();
        let users = crate::auth::UserStore::new(users_path.to_str().unwrap()).unwrap();
        std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o400)).unwrap();
        std::fs::set_permissions(&env_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let registration = users.register_with(
            "Owner",
            "owner@example.test",
            "rejected%41fixture",
            |password| {
                sync_postgres_admin_password_at(&url, password, static_dir, &env_path)?;
                Ok(())
            },
        );
        std::fs::set_permissions(&env_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = registration.unwrap_err().to_string();
        assert!(error.contains("Registration was rolled back"), "{error}");
        assert!(users.list_users().is_empty());
        assert!(!postgres_credentials_path(static_dir).exists());
        initial_config.connect(postgres::NoTls).unwrap();
        let mut rejected_registration = initial_config.clone();
        rejected_registration.password("rejected%41fixture");
        assert!(rejected_registration.connect(postgres::NoTls).is_err());
        let reopened = crate::auth::UserStore::new(users_path.to_str().unwrap()).unwrap();
        assert!(reopened.list_users().is_empty());
        let user = users
            .register_with(
                "Owner",
                "owner@example.test",
                initial_password,
                |password| {
                    sync_postgres_admin_password_at(&url, password, static_dir, &env_path)?;
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(user.role, "admin");
        let new_password = "rotate'd\\:%41@# /?";
        let mut updated = None;
        assert!(
            users
                .update_password_with(&user.id, Some(initial_password), new_password, |password| {
                    updated = Some(sync_postgres_admin_password_at(
                        &url, password, static_dir, &env_path,
                    )?);
                    Ok(())
                })
                .unwrap()
        );
        let updated = updated.unwrap();
        let config: postgres::Config = updated.parse().unwrap();
        assert_eq!(config.get_password(), Some(new_password.as_bytes()));
        config.connect(postgres::NoTls).unwrap();
        assert!(postgres::Client::connect(&url, postgres::NoTls).is_err());
        let credential_path = postgres_credentials_path(static_dir);
        assert_eq!(
            load_postgres_url(&credential_path).unwrap(),
            Some(updated.clone())
        );
        let saved = std::fs::read(&credential_path).unwrap();
        let saved_env = std::fs::read(&env_path).unwrap();
        let saved_users = std::fs::read(&users_path).unwrap();
        assert!(users.login(&user.email, new_password).unwrap().is_some());
        assert!(
            users
                .login(&user.email, initial_password)
                .unwrap()
                .is_none()
        );
        assert!(String::from_utf8_lossy(&saved_env).contains("RAYRAG_CHAT_MODEL=test model"));

        for first_rotation in [false, true] {
            if first_rotation {
                std::fs::remove_file(&credential_path).unwrap();
            }
            std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o400)).unwrap();
            std::fs::set_permissions(&env_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
            let result = users.update_password_with(
                &user.id,
                Some(new_password),
                "rejected%41fixture",
                |password| {
                    sync_postgres_admin_password_at(&updated, password, static_dir, &env_path)?;
                    Ok(())
                },
            );
            std::fs::set_permissions(&env_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let error = result
                .expect_err("the readonly env must reject rotation")
                .to_string();
            assert!(error.contains("connection files restored"), "{error}");
            assert!(
                error.contains("previous account password was restored"),
                "{error}"
            );
            assert_eq!(std::fs::read(&env_path).unwrap(), saved_env);
            assert_eq!(std::fs::read(&users_path).unwrap(), saved_users);
            let reopened = crate::auth::UserStore::new(users_path.to_str().unwrap()).unwrap();
            for store in [&users, &reopened] {
                assert!(store.login(&user.email, new_password).unwrap().is_some());
                assert!(
                    store
                        .login(&user.email, "rejected%41fixture")
                        .unwrap()
                        .is_none()
                );
            }
            if first_rotation {
                assert!(!credential_path.exists());
            } else {
                assert_eq!(std::fs::read(&credential_path).unwrap(), saved);
                assert_eq!(
                    std::fs::metadata(&credential_path)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
            config.connect(postgres::NoTls).unwrap();
            let mut rejected = config.clone();
            rejected.password("rejected%41fixture");
            assert!(rejected.connect(postgres::NoTls).is_err());
        }
    }

    #[test]
    fn saved_postgres_credentials_preserve_encoded_url_and_missing_file_semantics() {
        let root = tempfile::tempdir().unwrap();
        let path = postgres_credentials_path(root.path().to_str().unwrap());
        assert_eq!(load_postgres_url(&path).unwrap(), None);
        let url =
            "postgresql://admin%40example.com:p%40ss%3Aword@localhost:5432/rayrag?sslmode=disable";
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"url": url})).unwrap(),
        )
        .unwrap();
        assert_eq!(load_postgres_url(&path).unwrap().as_deref(), Some(url));
    }

    #[test]
    fn damaged_saved_postgres_credentials_report_recovery_without_leaking_secrets() {
        let root = tempfile::tempdir().unwrap();
        let path = postgres_credentials_path(root.path().to_str().unwrap());
        let secret = "private-password-marker";
        let payloads = [
            format!(r#"{{"url":"postgresql://admin:{secret}@localhost/rayrag""#),
            format!(r#"{{"url":{{"password":"{secret}"}}}}"#),
            r#"{"url":""}"#.to_string(),
            r#"{"url":"   "}"#.to_string(),
            format!(r#"{{"url":"https://admin:{secret}@localhost/rayrag"}}"#),
            format!(r#"{{"url":"{secret}"}}"#),
            r#"{"url":"postgresql://localhost/rayrag"}"#.to_string(),
            "null".to_string(),
        ];
        for payload in payloads {
            std::fs::write(&path, payload).unwrap();
            let error = apply_persisted_postgres_url(root.path().to_str().unwrap())
                .unwrap_err()
                .to_string();
            assert!(error.contains("pg_credentials.json"), "{error}");
            assert!(error.contains("Restore this file"), "{error}");
            assert!(!error.contains(secret), "{error}");
        }
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(
            load_postgres_url(&path)
                .unwrap_err()
                .to_string()
                .contains("UTF-8 JSON")
        );
    }

    #[test]
    fn unreadable_saved_postgres_credentials_are_reported() {
        let root = tempfile::tempdir().unwrap();
        let path = postgres_credentials_path(root.path().to_str().unwrap());
        std::fs::create_dir(&path).unwrap();
        let error = apply_persisted_postgres_url(root.path().to_str().unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Cannot load the saved PostgreSQL credential"));
        assert!(error.contains("pg_credentials.json"));
        assert!(error.contains("restart RayRAG"));
    }

    #[test]
    fn dsn_diagnostics_never_print_the_password() {
        assert_eq!(
            redact_dsn("postgresql://rayrag:change-me@127.0.0.1:5432/rayrag"),
            "postgresql://rayrag:***@127.0.0.1:5432/rayrag"
        );
        // No credentials at all: the endpoint still has to be recognisable.
        assert_eq!(
            redact_dsn("postgresql://127.0.0.1:55432/rayrag"),
            "postgresql://127.0.0.1:55432/rayrag"
        );
        // A password containing '@' keeps only the last separator.
        assert_eq!(
            redact_dsn("postgresql://rayrag:p@ss@db:5432/rayrag"),
            "postgresql://rayrag:***@db:5432/rayrag"
        );
        assert!(redact_dsn("not-a-dsn").contains("unparsable"));
        assert!(
            !redact_dsn("postgresql://rayrag:change-me@127.0.0.1:5432/rayrag")
                .contains("change-me")
        );
    }

    #[test]
    fn json_loader_rejects_corrupt_snapshots() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-persistence-json-corrupt-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("corrupt.json");
        atomic_write_local(&path, b"{not json").unwrap();
        let result: anyhow::Result<Option<serde_json::Value>> = load_json(&path);
        assert!(result.is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
