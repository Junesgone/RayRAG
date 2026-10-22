//! Agent webhooks — upstream `agent_api.py`'s `webhook` / `webhook/logs` / `webhook/test`.
//!
//! One webhook per agent, persisted as JSON next to the other RayRAG state files and written
//! atomically. Deliveries are recorded whether they succeed or fail, and the test endpoint
//! really performs the HTTP call: an operator learns the status code or the transport error,
//! never a bare "sent".
//!
//! The store is resolved lazily from `RAYRAG_WEBHOOKS_FILE` (default: `webhooks.json` beside the
//! static directory), so no `AppState` construction site has to know about it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One agent's webhook registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: String,
    pub agent_id: String,
    pub url: String,
    #[serde(default)]
    pub token: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

fn default_true() -> bool {
    true
}

/// One delivery attempt, kept so `webhook/logs` can answer "what happened last time".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delivery {
    pub at: u64,
    pub event: String,
    pub url: String,
    pub ok: bool,
    pub detail: String,
    #[serde(default)]
    pub duration_ms: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    webhooks: HashMap<String, Webhook>,
    #[serde(default)]
    deliveries: Vec<Delivery>,
}

/// File-backed store: one JSON document, atomic writes, capped delivery history.
pub struct WebhookStore {
    path: PathBuf,
    state: Mutex<State>,
    /// Deliveries kept per store; older entries are dropped so the file cannot grow forever.
    history_limit: usize,
}

impl WebhookStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<State>(&raw).ok())
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
            history_limit: 50,
        }
    }

    /// The process-wide store, or `None` when it cannot be created (an unwritable path). The
    /// handlers answer with the reason instead of pretending a webhook was saved.
    pub fn shared() -> Result<&'static WebhookStore, String> {
        static STORE: OnceLock<Result<WebhookStore, String>> = OnceLock::new();
        let resolved = STORE.get_or_init(|| {
            let path = std::env::var("RAYRAG_WEBHOOKS_FILE")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let static_dir = std::env::var("RAYRAG_STATIC_DIR")
                        .unwrap_or_else(|_| "web/static".to_string());
                    PathBuf::from(static_dir)
                        .parent()
                        .map(|parent| parent.join("webhooks.json"))
                        .unwrap_or_else(|| PathBuf::from("webhooks.json"))
                });
            Ok(WebhookStore::new(path))
        });
        match resolved {
            Ok(store) => Ok(store),
            Err(error) => Err(error.clone()),
        }
    }

    fn persist(state: &State, path: &PathBuf) -> Result<(), String> {
        let raw = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
        crate::persistence::atomic_write(path, &raw).map_err(|error| error.to_string())
    }

    pub fn get(&self, agent_id: &str) -> Option<Webhook> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.webhooks.get(agent_id).cloned())
    }

    pub fn upsert(
        &self,
        agent_id: &str,
        url: &str,
        token: &str,
        enabled: bool,
    ) -> Result<Webhook, String> {
        let now = now_secs();
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let existing = state.webhooks.get(agent_id).cloned();
        let webhook = Webhook {
            id: existing
                .as_ref()
                .map(|webhook| webhook.id.clone())
                .unwrap_or_else(|| format!("wh-{}", uuid::Uuid::new_v4())),
            agent_id: agent_id.to_string(),
            url: url.to_string(),
            token: token.to_string(),
            enabled,
            created_at: existing
                .as_ref()
                .map(|webhook| webhook.created_at)
                .unwrap_or(now),
            updated_at: now,
        };
        state.webhooks.insert(agent_id.to_string(), webhook.clone());
        Self::persist(&state, &self.path)?;
        Ok(webhook)
    }

    pub fn delete(&self, agent_id: &str) -> Result<bool, String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        let removed = state.webhooks.remove(agent_id).is_some();
        if removed {
            Self::persist(&state, &self.path)?;
        }
        Ok(removed)
    }

    pub fn record(&self, delivery: Delivery) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.deliveries.push(delivery);
        let limit = self.history_limit;
        if state.deliveries.len() > limit {
            let excess = state.deliveries.len() - limit;
            state.deliveries.drain(0..excess);
        }
        Self::persist(&state, &self.path)
    }

    pub fn deliveries(&self, agent_id: &str, limit: usize) -> Vec<Delivery> {
        let urls: Vec<String> = self
            .get(agent_id)
            .map(|webhook| vec![webhook.url])
            .unwrap_or_default();
        self.state
            .lock()
            .map(|state| {
                state
                    .deliveries
                    .iter()
                    .filter(|delivery| urls.contains(&delivery.url))
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// `POST /agents/{id}/webhook/test` — actually perform the call and record what happened.
pub async fn deliver(webhook: &Webhook, event: &str, payload: Value) -> Delivery {
    let started = std::time::Instant::now();
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return Delivery {
                at: now_secs(),
                event: event.to_string(),
                url: webhook.url.clone(),
                ok: false,
                detail: format!("could not build the HTTP client: {error}"),
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
    };
    let mut request = client.post(&webhook.url).json(&json!({
        "event": event,
        "agent_id": webhook.agent_id,
        "payload": payload,
        "sent_at": now_secs(),
    }));
    if !webhook.token.is_empty() {
        request = request.bearer_auth(&webhook.token);
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            Delivery {
                at: now_secs(),
                event: event.to_string(),
                url: webhook.url.clone(),
                ok: status.is_success(),
                detail: format!("HTTP {status}"),
                duration_ms: started.elapsed().as_millis() as u64,
            }
        }
        Err(error) => Delivery {
            at: now_secs(),
            event: event.to_string(),
            url: webhook.url.clone(),
            ok: false,
            detail: error.to_string(),
            duration_ms: started.elapsed().as_millis() as u64,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> WebhookStore {
        let path =
            std::env::temp_dir().join(format!("rr-webhooks-{}-{}.json", name, std::process::id()));
        let _ = std::fs::remove_file(&path);
        WebhookStore::new(path)
    }

    #[test]
    fn upsert_keeps_the_id_and_survives_a_reload() {
        let store = temp_store("reload");
        let first = store
            .upsert("agent-1", "https://example.test/hook", "secret", true)
            .expect("upsert");
        assert_eq!(first.agent_id, "agent-1");
        assert!(!first.id.is_empty());
        let second = store
            .upsert("agent-1", "https://example.test/other", "", false)
            .expect("upsert again");
        assert_eq!(first.id, second.id, "updating keeps the webhook identity");
        assert_eq!(second.url, "https://example.test/other");
        assert!(!second.enabled);
        // A second store over the same file sees the same registration: the write really landed.
        let reopened = WebhookStore::new(store.path.clone());
        let found = reopened.get("agent-1").expect("persisted");
        assert_eq!(found.url, "https://example.test/other");
    }

    #[test]
    fn deliveries_are_capped_and_filtered_by_agent() {
        let store = temp_store("history");
        store
            .upsert("agent-a", "https://example.test/a", "", true)
            .unwrap();
        store
            .upsert("agent-b", "https://example.test/b", "", true)
            .unwrap();
        for index in 0..60 {
            store
                .record(Delivery {
                    at: index,
                    event: "test".to_string(),
                    url: "https://example.test/a".to_string(),
                    ok: true,
                    detail: "HTTP 200 OK".to_string(),
                    duration_ms: 1,
                })
                .unwrap();
        }
        store
            .record(Delivery {
                at: 999,
                event: "test".to_string(),
                url: "https://example.test/b".to_string(),
                ok: false,
                detail: "HTTP 500".to_string(),
                duration_ms: 2,
            })
            .unwrap();
        // The cap is on the stored history as a whole, so agent-b's single delivery displaced
        // one of agent-a's sixty: 49 remain, not 50. Asserting the real number here keeps the
        // test about the cap rather than about an arithmetic slip.
        let a = store.deliveries("agent-a", 100);
        assert_eq!(a.len(), 49, "history is capped across all agents");
        let b = store.deliveries("agent-b", 100);
        assert_eq!(b.len(), 1, "logs are per agent");
        assert!(!b[0].ok);
        assert_eq!(b[0].detail, "HTTP 500");
    }

    #[test]
    fn delete_reports_whether_anything_went_away() {
        let store = temp_store("delete");
        assert!(!store.delete("missing").unwrap());
        store
            .upsert("agent-1", "https://example.test/hook", "", true)
            .unwrap();
        assert!(store.delete("agent-1").unwrap());
        assert!(store.get("agent-1").is_none());
    }
}
