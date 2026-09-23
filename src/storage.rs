//! S3-compatible object storage client with AWS SigV4 signing.
//!
//! Mirrors RAGFlow's `rag/utils/storage_factory.py` family — `minio_conn.py`,
//! `s3_conn.py`, `oss_conn.py`, `gcs_conn.py`, `azure_sas_conn.py` and
//! `ob_conn.py` all expose the same storage contract (`health`, `put`, `get`,
//! `rm`, `obj_exist`, `bucket_exists`, `get_presigned_url`, `copy`, `move`)
//! against their vendor's object store.
//!
//! RayRAG implements that contract once against the S3 API, which makes a
//! single client work with MinIO, AWS S3, Aliyun OSS, Tencent COS, Huawei
//! OBS, Oracle OCI and any other S3-compatible endpoint — including domestic
//! (China) deployments where MinIO / OSS / COS are the usual choices. The
//! endpoint, credentials and optional path-style addressing come from the
//! environment, so no code change is needed to switch vendors.
//!
//! Current scope: single-object operations (put/get/rm/head/exists + signed
//! URL). Multipart upload is intentionally deferred — RAGFlow's own put is a
//! single `client.put_object` call.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// S3-compatible object storage configuration.
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Endpoint without scheme handling: e.g. `http://127.0.0.1:9000` for
    /// MinIO, `https://s3.cn-north-1.amazonaws.com.cn` for AWS China,
    /// `https://oss-cn-shenzhen.aliyuncs.com` for Aliyun OSS,
    /// `https://cos.ap-guangzhou.myqcloud.com` for Tencent COS.
    pub endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// Use path-style addressing (`/bucket/key`). Required for MinIO and
    /// most self-hosted endpoints; leave `false` for AWS-style virtual-host.
    pub path_style: bool,
    /// Per-request timeout.
    pub timeout: Duration,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:9000".into(),
            region: "us-east-1".into(),
            access_key: String::new(),
            secret_key: String::new(),
            path_style: true,
            timeout: Duration::from_secs(60),
        }
    }
}

impl StorageConfig {
    /// Build from environment variables (RAYRAG_STORAGE_*). Empty
    /// endpoint disables the object store (local filesystem mode).
    pub fn from_env() -> Self {
        Self {
            endpoint: std::env::var("RAYRAG_STORAGE_ENDPOINT").unwrap_or_default(),
            region: std::env::var("RAYRAG_STORAGE_REGION").unwrap_or_else(|_| "us-east-1".into()),
            access_key: std::env::var("RAYRAG_STORAGE_ACCESS_KEY").unwrap_or_default(),
            secret_key: std::env::var("RAYRAG_STORAGE_SECRET_KEY").unwrap_or_default(),
            path_style: std::env::var("RAYRAG_STORAGE_PATH_STYLE")
                .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
                .unwrap_or(true),
            timeout: Duration::from_secs(60),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.endpoint.is_empty() && !self.access_key.is_empty() && !self.secret_key.is_empty()
    }
}

/// S3-compatible object storage client.
#[derive(Debug, Clone)]
pub struct StorageClient {
    pub config: StorageConfig,
    client: reqwest::Client,
}

impl StorageClient {
    pub fn new(config: StorageConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client build");
        Self { config, client }
    }

    fn endpoint(&self) -> &str {
        self.config.endpoint.trim_end_matches('/')
    }

    /// Bucket liveness check (RAGFlow `health` uses bucket_exists on the
    /// default bucket).
    pub async fn health(&self, bucket: &str) -> Result<bool> {
        self.bucket_exists(bucket).await
    }

    /// `HEAD /bucket/key` — mirrors MinIO `stat_object` / RAGFlow
    /// `obj_exist`.
    pub async fn obj_exist(&self, bucket: &str, key: &str) -> Result<bool> {
        let url = self.object_url(bucket, key);
        let response = self
            .client
            .head(&url)
            .headers(self.signed_headers("HEAD", &url, None, None)?)
            .send()
            .await
            .with_context(|| format!("HEAD failed: {url}"))?;
        match response.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            code => bail!("HEAD {url} -> {code}"),
        }
    }

    /// `PUT /bucket/key` — mirrors RAGFlow `put`.
    pub async fn put(&self, bucket: &str, key: &str, data: &[u8]) -> Result<()> {
        let url = self.object_url(bucket, key);
        let response = self
            .client
            .put(&url)
            .headers(self.signed_headers("PUT", &url, Some(data), None)?)
            .body(data.to_vec())
            .send()
            .await
            .with_context(|| format!("PUT failed: {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(
                "PUT {url} -> {status}: {}",
                body.chars().take(200).collect::<String>()
            );
        }
        Ok(())
    }

    /// `GET /bucket/key` — mirrors RAGFlow `get`.
    pub async fn get(&self, bucket: &str, key: &str) -> Result<Vec<u8>> {
        let url = self.object_url(bucket, key);
        let response = self
            .client
            .get(&url)
            .headers(self.signed_headers("GET", &url, None, None)?)
            .send()
            .await
            .with_context(|| format!("GET failed: {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(
                "GET {url} -> {status}: {}",
                body.chars().take(200).collect::<String>()
            );
        }
        // Bounded read: a storage backend can stream an arbitrarily large object and
        // this path must not translate that into process memory.
        Ok(crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::body_limit_bytes(),
            "Storage",
        )
        .await?)
    }

    /// `DELETE /bucket/key` — mirrors RAGFlow `rm`.
    pub async fn rm(&self, bucket: &str, key: &str) -> Result<()> {
        let url = self.object_url(bucket, key);
        let response = self
            .client
            .delete(&url)
            .headers(self.signed_headers("DELETE", &url, None, None)?)
            .send()
            .await
            .with_context(|| format!("DELETE failed: {url}"))?;
        let status = response.status();
        // 204 No Content is the canonical success; 404 is idempotent.
        if !status.is_success() && status.as_u16() != 404 {
            bail!("DELETE {url} -> {status}");
        }
        Ok(())
    }

    /// `HEAD /bucket` — mirrors MinIO `bucket_exists` / RAGFlow
    /// `bucket_exists`.
    pub async fn bucket_exists(&self, bucket: &str) -> Result<bool> {
        let url = self.bucket_url(bucket);
        let response = self
            .client
            .head(&url)
            .headers(self.signed_headers("HEAD", &url, None, None)?)
            .send()
            .await
            .with_context(|| format!("bucket HEAD failed: {url}"))?;
        match response.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            code => bail!("bucket HEAD {url} -> {code}"),
        }
    }

    /// `PUT /bucket` — mirrors MinIO `make_bucket` (RAGFlow creates the
    /// default bucket lazily).
    pub async fn make_bucket(&self, bucket: &str) -> Result<()> {
        let url = self.bucket_url(bucket);
        let response = self
            .client
            .put(&url)
            .headers(self.signed_headers("PUT", &url, None, None)?)
            .send()
            .await
            .with_context(|| format!("make_bucket failed: {url}"))?;
        let status = response.status();
        if !status.is_success() && status.as_u16() != 409 {
            bail!("make_bucket {url} -> {status}");
        }
        Ok(())
    }

    /// Presigned GET URL — mirrors MinIO `presigned_get_object` / RAGFlow
    /// `get_presigned_url`. `expires_secs` must be in 1..=604800 (SigV4 cap).
    pub fn presigned_get_url(&self, bucket: &str, key: &str, expires_secs: u32) -> Result<String> {
        let expires = expires_secs.clamp(1, 604_800);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let amz_date = amz_date(now);
        let date = &amz_date[..8];
        let url = self.object_url(bucket, key);
        let parsed = url::Url::parse(&url).with_context(|| format!("bad url {url}"))?;
        let query = format!(
            "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={}%2F{}%2F{}%2Fs3%2Faws4_request&X-Amz-Date={}&X-Amz-Expires={}&X-Amz-SignedHeaders=host",
            self.config.access_key, date, self.config.region, amz_date, expires
        );
        let canonical = format!(
            "GET\n{path}\n{query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD",
            path = parsed.path(),
            host = parsed.host_str().unwrap_or_default(),
        );
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{date}/{region}/s3/aws4_request\n{hash}",
            region = self.config.region,
            hash = hex::encode(Sha256::digest(canonical.as_bytes())),
        );
        let signature = self.sign(&string_to_sign, date)?;
        Ok(format!("{url}?{query}&X-Amz-Signature={signature}"))
    }

    // ---- internal helpers ----

    fn bucket_url(&self, bucket: &str) -> String {
        if self.config.path_style {
            format!("{}/{}", self.endpoint(), bucket)
        } else {
            // Virtual-host style: bucket.<host>[/]
            let rest = self
                .endpoint()
                .split_once("://")
                .map(|(scheme, host)| (scheme, host))
                .unwrap_or(("https", self.endpoint()));
            format!("{}://{}.{}", rest.0, bucket, rest.1)
        }
    }

    fn object_url(&self, bucket: &str, key: &str) -> String {
        let key = key.trim_start_matches('/');
        if self.config.path_style {
            format!("{}/{}/{}", self.endpoint(), bucket, key)
        } else {
            format!("{}/{}", self.bucket_url(bucket), key)
        }
    }

    /// Build AWS SigV4-signed headers for a single-object request.
    fn signed_headers(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
        _extra: Option<(&str, &str)>,
    ) -> Result<reqwest::header::HeaderMap> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let amz_date = amz_date(now);
        let date = &amz_date[..8];
        let parsed = url::Url::parse(url).with_context(|| format!("bad url {url}"))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow!("no host in {url}"))?;
        let payload_hash = match body {
            Some(data) => hex::encode(Sha256::digest(data)),
            None => "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(), // SHA-256 of empty string
        };
        let canonical = format!(
            "{method}\n{path}\n{query}\nhost:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n\nhost;x-amz-content-sha256;x-amz-date\n{payload_hash}",
            path = parsed.path(),
            query = parsed.query().unwrap_or(""),
        );
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{date}/{region}/s3/aws4_request\n{hash}",
            region = self.config.region,
            hash = hex::encode(Sha256::digest(canonical.as_bytes())),
        );
        let signature = self.sign(&string_to_sign, date)?;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::HOST,
            host.parse().map_err(|_| anyhow!("bad host"))?,
        );
        headers.insert(
            "x-amz-content-sha256",
            payload_hash
                .parse()
                .map_err(|_| anyhow!("bad hash header"))?,
        );
        headers.insert(
            "x-amz-date",
            amz_date.parse().map_err(|_| anyhow!("bad date header"))?,
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!(
                "AWS4-HMAC-SHA256 Credential={access}/{date}/{region}/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={signature}",
                access = self.config.access_key,
                region = self.config.region,
            )
            .parse()
            .map_err(|_| anyhow!("bad auth header"))?,
        );
        Ok(headers)
    }

    /// SigV4 signing key chain, then HMAC the string-to-sign.
    fn sign(&self, string_to_sign: &str, date: &str) -> Result<String> {
        let k_date = hmac_sha256(format!("AWS4{}", self.config.secret_key), date)?;
        let k_region = hmac_sha256(k_date, &self.config.region)?;
        let k_service = hmac_sha256(k_region, "s3")?;
        let k_signing = hmac_sha256(k_service, "aws4_request")?;
        let signature = hmac_sha256(k_signing, string_to_sign)?;
        Ok(hex::encode(signature))
    }
}

fn hmac_sha256(key: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(key.as_ref()).map_err(|_| anyhow!("hmac key"))?;
    mac.update(data.as_ref());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn amz_date(secs: u64) -> String {
    // Format UTC as YYYYMMDD'T'HHMMSS'Z' without chrono.
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z")
}

/// Howard Hinnant's civil-from-days algorithm (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_for(endpoint: &str) -> StorageConfig {
        StorageConfig {
            endpoint: endpoint.to_string(),
            region: "us-east-1".into(),
            access_key: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            path_style: true,
            timeout: Duration::from_secs(10),
        }
    }

    #[test]
    fn civil_from_days_matches_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_668), (2026, 8, 3));
    }

    #[test]
    fn amz_date_formats_utc() {
        // 2024-01-01T00:00:00Z = 1704067200
        assert_eq!(amz_date(1_704_067_200), "20240101T000000Z");
        // 2026-08-03T00:00:00Z
        assert_eq!(amz_date(1_785_715_200), "20260803T000000Z");
    }

    #[test]
    fn hmac_sha256_matches_sigv4_test_vector() {
        // AWS SigV4 signing-key derivation, cross-checked against an
        // independent Python implementation: kDate=68a9e453..., kRegion,
        // kService, kSigning=2c94c0cf... for 20150830/us-east-1/iam with the
        // documented example secret.
        let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let k_date = hmac_sha256(format!("AWS4{secret}"), "20150830").unwrap();
        assert_eq!(
            hex::encode(&k_date),
            "68a9e4535ffbb09dcb6d25807a9ba5e3aef7cd00b3c57ed4b0c4a04988649f51"
        );
        let k_region = hmac_sha256(k_date, "us-east-1").unwrap();
        let k_service = hmac_sha256(k_region, "iam").unwrap();
        let k_signing = hmac_sha256(k_service, "aws4_request").unwrap();
        let expected = "2c94c0cf5378ada6887f09bb697df8fc0affdb34ba1cdd5bda32b664bd55b73c";
        assert_eq!(hex::encode(k_signing), expected);
    }

    #[tokio::test]
    async fn presigned_url_is_well_formed() {
        let client = StorageClient::new(config_for("http://127.0.0.1:9000"));
        let url = client.presigned_get_url("docs", "a/b.txt", 3600).unwrap();
        assert!(
            url.starts_with("http://127.0.0.1:9000/docs/a/b.txt?X-Amz-Algorithm=AWS4-HMAC-SHA256")
        );
        assert!(url.contains("X-Amz-Expires=3600"));
        assert!(url.contains("X-Amz-Signature="), "{url}");
        // Signature is 64 lowercase hex chars.
        let sig = url.split("X-Amz-Signature=").nth(1).unwrap();
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn object_url_path_style_and_key_normalization() {
        let client = StorageClient::new(config_for("http://127.0.0.1:9000"));
        assert_eq!(
            client.object_url("docs", "/a//b.txt"),
            "http://127.0.0.1:9000/docs/a//b.txt"
        );
        // path-style virtual host off
        let mut cfg = config_for("http://127.0.0.1:9000");
        cfg.path_style = false;
        let client2 = StorageClient::new(cfg);
        assert_eq!(
            client2.object_url("docs", "a.txt"),
            "http://docs.127.0.0.1:9000/a.txt"
        );
    }

    #[tokio::test]
    async fn enabled_requires_endpoint_and_credentials() {
        let cfg = StorageConfig::default();
        assert!(!cfg.enabled());
        let cfg = config_for("http://127.0.0.1:9000");
        assert!(cfg.enabled());
    }

    // Real MinIO round-trip (ignored by default; requires RAYRAG_STORAGE_*).
    #[tokio::test]
    #[ignore]
    async fn minio_round_trip_live() {
        let cfg = StorageConfig::from_env();
        assert!(
            cfg.enabled(),
            "set RAYRAG_STORAGE_ENDPOINT/_ACCESS_KEY/_SECRET_KEY"
        );
        let client = StorageClient::new(cfg);
        let bucket = "rayrag-test";
        client.make_bucket(bucket).await.unwrap();
        assert!(client.health(bucket).await.unwrap());
        client
            .put(bucket, "hello.txt", b"hello storage")
            .await
            .unwrap();
        assert!(client.obj_exist(bucket, "hello.txt").await.unwrap());
        let data = client.get(bucket, "hello.txt").await.unwrap();
        assert_eq!(data, b"hello storage");
        let url = client.presigned_get_url(bucket, "hello.txt", 60).unwrap();
        assert!(!url.is_empty());
        client.rm(bucket, "hello.txt").await.unwrap();
        assert!(!client.obj_exist(bucket, "hello.txt").await.unwrap());
    }
}
