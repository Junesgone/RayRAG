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
        if let Some(previous) = previous {
            atomic_write_local(path, &previous)?;
        } else {
            std::fs::remove_file(path).ok();
            sync_parent(path)?;
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
        let mut client = Client::connect(url, NoTls)?;
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
            let mirror = start_mirror(url.clone(), Some("18.4".into()), namespace.clone()).unwrap();
            let path = Path::new("web/postgres-smoke.json");
            mirror.store(path, b"postgres-18.4-snapshot").unwrap();
            assert_eq!(
                mirror.load(path).unwrap().unwrap(),
                b"postgres-18.4-snapshot"
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
        let root = std::env::temp_dir().join(format!(
            "rayrag-persistence-json-{}",
            uuid::Uuid::new_v4()
        ));
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
