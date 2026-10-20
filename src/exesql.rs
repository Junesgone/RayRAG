//! ExeSQL 连接器 — RAGFlow `exesql.py` 的 Rust 实现（PostgreSQL 版）
//!
//! 上游语义：执行 SQL 查询，返回 JSON 结果（NaN/Infinity → null，Decimal → float）。
//! 本实现只支持 postgres（postgres-backend feature，postgres crate）；其他 db_type
//! 返回明确的不支持错误。安全：拒绝连接名为 rag_flow 的数据库（对齐上游 check 拒止）。
//! 结果按 max_records 截断（默认 1024 行）。

use anyhow::{Result, bail};
use serde_json::{Map, Value};

/// 支持的数据库类型（当前仅 postgres）。
pub const SUPPORTED_DB_TYPES: &[&str] = &["postgres"];

/// ExeSQL 配置（对齐上游 ExeSQLParam 配置项）。
#[derive(Debug, Clone, Default)]
pub struct ExeSqlConfig {
    pub db_type: String,
    pub database: String,
    pub username: String,
    pub host: String,
    pub port: u16,
    pub password: String,
    pub max_records: usize,
}

/// 执行 SQL；返回 JSON 数组（每行一个对象）。
pub async fn execute_sql(config: &ExeSqlConfig, sql: &str) -> Result<String> {
    validate_config(config)?;
    let sql = sql.trim();
    if sql.is_empty() {
        bail!("SQL for `ExeSQL` MUST not be empty.");
    }
    if !config.db_type.eq_ignore_ascii_case("postgres") {
        bail!(
            "Unsupported db_type '{}'; supported: {}",
            config.db_type,
            SUPPORTED_DB_TYPES.join(", ")
        );
    }
    execute_postgres(config, sql).await
}

/// 配置校验（对齐上游 check()）。
fn validate_config(config: &ExeSqlConfig) -> Result<()> {
    if config.database.trim().is_empty() {
        bail!("Database name must not be empty");
    }
    if config.username.trim().is_empty() {
        bail!("Database username must not be empty");
    }
    if config.host.trim().is_empty() {
        bail!("IP Address must not be empty");
    }
    if config.port == 0 {
        bail!("IP Port must be positive");
    }
    if config.max_records == 0 {
        bail!("Maximum number of records must be positive");
    }
    // 安全拒止（对齐上游：不允许连接 rag_flow 数据库）
    if config.database == "rag_flow"
        && (config.host == "ragflow-mysql" || config.password == "infini_rag_flow")
    {
        bail!("For the security reason, it does not support database named rag_flow.");
    }
    Ok(())
}

/// PostgreSQL 执行（postgres-backend feature）。
#[cfg(feature = "postgres-backend")]
async fn execute_postgres(config: &ExeSqlConfig, sql: &str) -> Result<String> {
    use postgres::{Client, NoTls};
    let connection_string = format!(
        "host={} port={} user={} password={} dbname={}",
        config.host, config.port, config.username, config.password, config.database
    );
    let mut client = Client::connect(&connection_string, NoTls)
        .map_err(|error| anyhow::anyhow!("Failed to connect to PostgreSQL: {error}"))?;
    let rows = client
        .query(sql, &[])
        .map_err(|error| anyhow::anyhow!("PostgreSQL query error: {error}"))?;
    let mut output = Vec::new();
    for row in rows.into_iter().take(config.max_records) {
        let mut object = Map::new();
        for (index, column) in row.columns().iter().enumerate() {
            let value: Value = match row.try_get::<_, postgres::types::Json<Value>>(index) {
                Ok(postgres::types::Json(value)) => value,
                Err(_) => match row.try_get::<_, String>(index) {
                    Ok(text) => Value::String(text),
                    Err(_) => Value::Null,
                },
            };
            object.insert(column.name().to_string(), value);
        }
        output.push(Value::Object(object));
    }
    Ok(serde_json::to_string(&Value::Array(output))?)
}

/// 非 postgres-backend feature 时的占位（返回明确错误而非静默缺失）。
#[cfg(not(feature = "postgres-backend"))]
async fn execute_postgres(config: &ExeSqlConfig, _sql: &str) -> Result<String> {
    bail!(
        "PostgreSQL support requires the `postgres-backend` feature (db_type={}, host={})",
        config.db_type,
        config.host
    );
}

/// 从节点参数/环境变量构造配置（运行时由 agent.rs 调用）。
impl ExeSqlConfig {
    pub fn from_params_and_env(params: &Map<String, Value>) -> Self {
        let read = |key: &str, env: &str| -> String {
            params
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| std::env::var(env).unwrap_or_default())
        };
        let port = params
            .get("port")
            .and_then(Value::as_u64)
            .map(|value| value as u16)
            .unwrap_or_else(|| {
                std::env::var("EXESQL_PORT")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(5432)
            });
        let max_records = params
            .get("max_records")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(1024);
        Self {
            db_type: read("db_type", "EXESQL_DB_TYPE"),
            database: read("database", "EXESQL_DATABASE"),
            username: read("username", "EXESQL_USERNAME"),
            host: read("host", "EXESQL_HOST"),
            port,
            password: read("password", "EXESQL_PASSWORD"),
            max_records,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_config() -> ExeSqlConfig {
        ExeSqlConfig {
            db_type: "postgres".into(),
            database: "app".into(),
            username: "postgres".into(),
            host: "127.0.0.1".into(),
            port: 5432,
            password: "secret".into(),
            max_records: 1024,
        }
    }

    #[tokio::test]
    async fn rejects_empty_sql() {
        let result = execute_sql(&sample_config(), "  ").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("MUST not be empty")
        );
    }

    #[tokio::test]
    async fn rejects_missing_config_fields() {
        let config = ExeSqlConfig {
            database: "".into(),
            ..sample_config()
        };
        let result = execute_sql(&config, "select 1").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Database name"));
    }

    #[tokio::test]
    async fn rejects_rag_flow_database() {
        let config = ExeSqlConfig {
            database: "rag_flow".into(),
            host: "ragflow-mysql".into(),
            ..sample_config()
        };
        let result = execute_sql(&config, "select 1").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("does not support database named rag_flow")
        );
    }

    #[tokio::test]
    async fn rejects_mysql_db_type() {
        // Boss 决策：RayRAG 不使用 mysql，任何 mysql 请求都明确报错
        let config = ExeSqlConfig {
            db_type: "mysql".into(),
            ..sample_config()
        };
        let result = execute_sql(&config, "select 1").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unsupported db_type")
        );
    }

    #[test]
    fn config_from_params_and_env_prefers_params() {
        let params = Map::from_iter([
            ("db_type".into(), json!("postgres")),
            ("host".into(), json!("db.internal")),
        ]);
        unsafe { std::env::set_var("EXESQL_DB_TYPE", "mysql") };
        let config = ExeSqlConfig::from_params_and_env(&params);
        assert_eq!(config.db_type, "postgres");
        assert_eq!(config.port, 5432);
        assert_eq!(config.max_records, 1024);
        unsafe { std::env::remove_var("EXESQL_DB_TYPE") };
    }
}
