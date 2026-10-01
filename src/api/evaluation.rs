//! Persistent retrieval evaluation datasets, cases, runs, and recommendations.

use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use crate::search::HybridSearchQuery;
use crate::server::{AppState, AuthContext, all_kbs_accessible, kb_embedder_for};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationDataset {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub description: String,
    pub kb_ids: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationCase {
    pub id: String,
    pub dataset_id: String,
    pub question: String,
    #[serde(default)]
    pub reference_answer: Option<String>,
    #[serde(default)]
    pub relevant_doc_ids: Vec<String>,
    #[serde(default)]
    pub relevant_chunk_ids: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationRun {
    pub id: String,
    pub dataset_id: String,
    pub owner_id: String,
    pub name: String,
    pub status: String,
    pub top_k: usize,
    pub vector_weight: f32,
    pub metrics_summary: HashMap<String, f32>,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: u64,
    #[serde(default)]
    pub completed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub id: String,
    pub run_id: String,
    pub case_id: String,
    pub retrieved_chunk_ids: Vec<String>,
    pub retrieved_doc_ids: Vec<String>,
    pub metrics: HashMap<String, f32>,
    pub execution_time_ms: f32,
    pub created_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct EvaluationState {
    datasets: Vec<EvaluationDataset>,
    cases: Vec<EvaluationCase>,
    runs: Vec<EvaluationRun>,
    results: Vec<EvaluationResult>,
}

pub struct EvaluationStore {
    state: RwLock<EvaluationState>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl EvaluationStore {
    pub fn new(path: impl AsRef<FsPath>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let state = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            EvaluationState::default()
        };
        validate_state(&state)?;
        let store = Self {
            state: RwLock::new(state),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self {
            state: RwLock::new(EvaluationState::default()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut EvaluationState) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut state = self.state.write().unwrap();
        let previous = state.clone();
        let value = mutation(&mut state)?;
        validate_state(&state)?;
        if let Err(error) = self.persist(&state) {
            *state = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let _save_guard = self.save_lock.lock().unwrap();
        self.persist(&self.state.read().unwrap())
    }

    fn persist(&self, state: &EvaluationState) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(state)?)
    }

    pub fn create_dataset(
        &self,
        owner_id: &str,
        request: DatasetRequest,
    ) -> anyhow::Result<EvaluationDataset> {
        let name = request.name.trim().to_string();
        if name.is_empty() || request.kb_ids.is_empty() {
            anyhow::bail!("name and kb_ids are required");
        }
        let now = now_ms();
        let dataset = EvaluationDataset {
            id: uuid::Uuid::new_v4().to_string(),
            owner_id: owner_id.into(),
            name,
            description: request.description.trim().to_string(),
            kb_ids: deduplicate(request.kb_ids),
            created_at: now,
            updated_at: now,
        };
        self.mutate(|state| {
            state.datasets.push(dataset.clone());
            Ok(dataset)
        })
    }

    pub fn list_datasets(&self, owner_id: &str, is_admin: bool) -> Vec<EvaluationDataset> {
        let mut datasets: Vec<_> = self
            .state
            .read()
            .unwrap()
            .datasets
            .iter()
            .filter(|dataset| is_admin || dataset.owner_id == owner_id)
            .cloned()
            .collect();
        datasets.sort_by_key(|dataset| std::cmp::Reverse(dataset.created_at));
        datasets
    }

    pub fn dataset_for(
        &self,
        id: &str,
        owner_id: &str,
        is_admin: bool,
    ) -> Option<EvaluationDataset> {
        self.state
            .read()
            .unwrap()
            .datasets
            .iter()
            .find(|dataset| dataset.id == id && (is_admin || dataset.owner_id == owner_id))
            .cloned()
    }

    pub fn delete_dataset(&self, id: &str, owner_id: &str, is_admin: bool) -> anyhow::Result<bool> {
        self.mutate(|state| {
            let Some(dataset) = state
                .datasets
                .iter()
                .find(|dataset| dataset.id == id && (is_admin || dataset.owner_id == owner_id))
            else {
                return Ok(false);
            };
            let case_ids: HashSet<_> = state
                .cases
                .iter()
                .filter(|case| case.dataset_id == dataset.id)
                .map(|case| case.id.clone())
                .collect();
            let run_ids: HashSet<_> = state
                .runs
                .iter()
                .filter(|run| run.dataset_id == dataset.id)
                .map(|run| run.id.clone())
                .collect();
            state.datasets.retain(|dataset| dataset.id != id);
            state.cases.retain(|case| !case_ids.contains(&case.id));
            state.runs.retain(|run| !run_ids.contains(&run.id));
            state
                .results
                .retain(|result| !run_ids.contains(&result.run_id));
            Ok(true)
        })
    }

    pub fn add_case(
        &self,
        dataset_id: &str,
        owner_id: &str,
        is_admin: bool,
        request: CaseRequest,
    ) -> anyhow::Result<Option<EvaluationCase>> {
        let question = request.question.trim().to_string();
        if question.is_empty() {
            anyhow::bail!("question is required");
        }
        self.mutate(|state| {
            if !state.datasets.iter().any(|dataset| {
                dataset.id == dataset_id && (is_admin || dataset.owner_id == owner_id)
            }) {
                return Ok(None);
            }
            let case = EvaluationCase {
                id: uuid::Uuid::new_v4().to_string(),
                dataset_id: dataset_id.into(),
                question,
                reference_answer: request.reference_answer,
                relevant_doc_ids: deduplicate(request.relevant_doc_ids),
                relevant_chunk_ids: deduplicate(request.relevant_chunk_ids),
                metadata: request.metadata,
                created_at: now_ms(),
            };
            state.cases.push(case.clone());
            Ok(Some(case))
        })
    }

    pub fn cases_for(
        &self,
        dataset_id: &str,
        owner_id: &str,
        is_admin: bool,
    ) -> Option<Vec<EvaluationCase>> {
        let state = self.state.read().unwrap();
        state
            .datasets
            .iter()
            .any(|dataset| dataset.id == dataset_id && (is_admin || dataset.owner_id == owner_id))
            .then(|| {
                let mut cases: Vec<_> = state
                    .cases
                    .iter()
                    .filter(|case| case.dataset_id == dataset_id)
                    .cloned()
                    .collect();
                cases.sort_by_key(|case| case.created_at);
                cases
            })
    }

    pub fn begin_run(
        &self,
        dataset: &EvaluationDataset,
        request: RunRequest,
    ) -> anyhow::Result<(EvaluationRun, Vec<EvaluationCase>)> {
        self.mutate(|state| {
            let cases: Vec<_> = state
                .cases
                .iter()
                .filter(|case| case.dataset_id == dataset.id)
                .cloned()
                .collect();
            if cases.is_empty() {
                anyhow::bail!("evaluation dataset has no cases");
            }
            let run = EvaluationRun {
                id: uuid::Uuid::new_v4().to_string(),
                dataset_id: dataset.id.clone(),
                owner_id: dataset.owner_id.clone(),
                name: request
                    .name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| format!("Evaluation {}", now_ms())),
                status: "RUNNING".into(),
                top_k: request.top_k.clamp(1, 100),
                vector_weight: request.vector_weight.clamp(0.0, 1.0),
                metrics_summary: HashMap::new(),
                error: None,
                created_at: now_ms(),
                completed_at: None,
            };
            state.runs.push(run.clone());
            Ok((run, cases))
        })
    }

    pub fn finish_run(
        &self,
        run_id: &str,
        results: Vec<EvaluationResult>,
    ) -> anyhow::Result<EvaluationRun> {
        self.mutate(|state| {
            let run = state
                .runs
                .iter_mut()
                .find(|run| run.id == run_id)
                .ok_or_else(|| anyhow::anyhow!("run not found"))?;
            run.status = "COMPLETED".into();
            run.metrics_summary = summary_metrics(&results);
            run.completed_at = Some(now_ms());
            state.results.retain(|result| result.run_id != run_id);
            state.results.extend(results);
            Ok(run.clone())
        })
    }

    pub fn fail_run(&self, run_id: &str, error: &str) -> anyhow::Result<()> {
        self.mutate(|state| {
            if let Some(run) = state.runs.iter_mut().find(|run| run.id == run_id) {
                run.status = "FAILED".into();
                run.error = Some(error.into());
                run.completed_at = Some(now_ms());
            }
            Ok(())
        })
    }

    pub fn run_results(
        &self,
        run_id: &str,
        owner_id: &str,
        is_admin: bool,
    ) -> Option<(EvaluationRun, Vec<EvaluationResult>)> {
        let state = self.state.read().unwrap();
        let run = state
            .runs
            .iter()
            .find(|run| run.id == run_id && (is_admin || run.owner_id == owner_id))?
            .clone();
        let results = state
            .results
            .iter()
            .filter(|result| result.run_id == run_id)
            .cloned()
            .collect();
        Some((run, results))
    }
}

#[derive(Debug, Deserialize)]
pub struct DatasetRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub kb_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CaseRequest {
    pub question: String,
    #[serde(default)]
    pub reference_answer: Option<String>,
    #[serde(default)]
    pub relevant_doc_ids: Vec<String>,
    #[serde(default)]
    pub relevant_chunk_ids: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct RunRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub vector_weight: f32,
}

#[derive(Debug, Deserialize, Default)]
pub struct EvaluationQuery {
    #[serde(default)]
    pub dataset_id: Option<String>,
}

pub async fn list_datasets(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": state.evaluations.list_datasets(&auth.user_id, auth.is_admin)
    }))
}

pub async fn create_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<DatasetRequest>,
) -> axum::response::Response {
    if !all_kbs_accessible(&state, &request.kb_ids, &auth) {
        return bad_request("At least one accessible kb_id is required");
    }
    match state.evaluations.create_dataset(&auth.user_id, request) {
        Ok(dataset) => (
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({ "code": 0, "data": dataset })),
        )
            .into_response(),
        Err(error) => bad_request(&error.to_string()),
    }
}

pub async fn get_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> axum::response::Response {
    match state
        .evaluations
        .dataset_for(&id, &auth.user_id, auth.is_admin)
    {
        Some(dataset) => Json(serde_json::json!({ "code": 0, "data": dataset })).into_response(),
        None => not_found("Evaluation dataset not found"),
    }
}

pub async fn delete_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> axum::response::Response {
    match state
        .evaluations
        .delete_dataset(&id, &auth.user_id, auth.is_admin)
    {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => not_found("Evaluation dataset not found"),
        Err(error) => server_error(&error.to_string()),
    }
}

pub async fn list_cases(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> axum::response::Response {
    match state
        .evaluations
        .cases_for(&id, &auth.user_id, auth.is_admin)
    {
        Some(cases) => Json(serde_json::json!({ "code": 0, "data": cases })).into_response(),
        None => not_found("Evaluation dataset not found"),
    }
}

pub async fn add_case(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(request): Json<CaseRequest>,
) -> axum::response::Response {
    match state
        .evaluations
        .add_case(&id, &auth.user_id, auth.is_admin, request)
    {
        Ok(Some(case)) => (
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({ "code": 0, "data": case })),
        )
            .into_response(),
        Ok(None) => not_found("Evaluation dataset not found"),
        Err(error) => bad_request(&error.to_string()),
    }
}

pub async fn start_run(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(request): Json<RunRequest>,
) -> axum::response::Response {
    let Some(dataset) = state
        .evaluations
        .dataset_for(&id, &auth.user_id, auth.is_admin)
    else {
        return not_found("Evaluation dataset not found");
    };
    if !all_kbs_accessible(&state, &dataset.kb_ids, &auth) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "code": 403, "message": "Knowledge base access changed" })),
        )
            .into_response();
    }
    let (run, cases) = match state.evaluations.begin_run(&dataset, request) {
        Ok(value) => value,
        Err(error) => return bad_request(&error.to_string()),
    };
    let result = execute_retrieval_evaluation(&state, &dataset, &run, &cases).await;
    match result {
        Ok(results) => match state.evaluations.finish_run(&run.id, results) {
            Ok(run) => Json(serde_json::json!({ "code": 0, "data": run })).into_response(),
            Err(error) => server_error(&error.to_string()),
        },
        Err(error) => {
            state.evaluations.fail_run(&run.id, &error.to_string()).ok();
            bad_request(&error.to_string())
        }
    }
}

pub async fn get_run(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> axum::response::Response {
    match state
        .evaluations
        .run_results(&id, &auth.user_id, auth.is_admin)
    {
        Some((run, results)) => Json(serde_json::json!({
            "code": 0,
            "data": { "run": run, "results": results }
        }))
        .into_response(),
        None => not_found("Evaluation run not found"),
    }
}

pub async fn recommendations(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(_query): Query<EvaluationQuery>,
) -> axum::response::Response {
    let Some((run, _)) = state
        .evaluations
        .run_results(&id, &auth.user_id, auth.is_admin)
    else {
        return not_found("Evaluation run not found");
    };
    Json(serde_json::json!({ "code": 0, "data": recommendations_for(&run) })).into_response()
}

async fn execute_retrieval_evaluation(
    state: &AppState,
    dataset: &EvaluationDataset,
    run: &EvaluationRun,
    cases: &[EvaluationCase],
) -> anyhow::Result<Vec<EvaluationResult>> {
    let embedder = if run.vector_weight > 0.0 {
        Some(kb_embedder_for(state, &dataset.kb_ids)?)
    } else {
        None
    };
    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        let started = Instant::now();
        let embedding = if let Some(embedder) = &embedder {
            embedder
                .embed(&[case.question.as_str()])
                .await?
                .into_iter()
                .next()
        } else {
            None
        };
        let matches = state
            .engine
            .read()
            .unwrap()
            .hybrid_search_kbs(HybridSearchQuery {
                query: &case.question,
                query_embedding: embedding.as_deref(),
                top_k: run.top_k,
                kb_ids: &dataset.kb_ids,
                vector_weight: run.vector_weight,
                doc_ids: None,
                rank_feature: None,
            });
        let retrieved_chunk_ids: Vec<_> = matches
            .iter()
            .map(|result| result.chunk.id.clone())
            .collect();
        let retrieved_doc_ids: Vec<_> = matches
            .iter()
            .filter_map(|result| result.chunk.metadata.get("doc_id").cloned())
            .collect();
        let metrics = retrieval_metrics(
            &retrieved_chunk_ids,
            &case.relevant_chunk_ids,
            &retrieved_doc_ids,
            &case.relevant_doc_ids,
        );
        results.push(EvaluationResult {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: run.id.clone(),
            case_id: case.id.clone(),
            retrieved_chunk_ids,
            retrieved_doc_ids,
            metrics,
            execution_time_ms: started.elapsed().as_secs_f32() * 1000.0,
            created_at: now_ms(),
        });
    }
    Ok(results)
}

fn retrieval_metrics(
    retrieved_chunk_ids: &[String],
    relevant_chunk_ids: &[String],
    retrieved_doc_ids: &[String],
    relevant_doc_ids: &[String],
) -> HashMap<String, f32> {
    let (retrieved, relevant) = if !relevant_chunk_ids.is_empty() {
        (retrieved_chunk_ids, relevant_chunk_ids)
    } else {
        (retrieved_doc_ids, relevant_doc_ids)
    };
    if relevant.is_empty() {
        return HashMap::new();
    }
    let retrieved_set: HashSet<_> = retrieved.iter().collect();
    let relevant_set: HashSet<_> = relevant.iter().collect();
    let hits = retrieved_set.intersection(&relevant_set).count() as f32;
    let precision = if retrieved_set.is_empty() {
        0.0
    } else {
        hits / retrieved_set.len() as f32
    };
    let recall = hits / relevant_set.len() as f32;
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    let mrr = retrieved
        .iter()
        .position(|id| relevant_set.contains(id))
        .map(|index| 1.0 / (index + 1) as f32)
        .unwrap_or(0.0);
    HashMap::from([
        ("precision".into(), precision),
        ("recall".into(), recall),
        ("f1_score".into(), f1),
        ("hit_rate".into(), (hits > 0.0) as u8 as f32),
        ("mrr".into(), mrr),
    ])
}

fn summary_metrics(results: &[EvaluationResult]) -> HashMap<String, f32> {
    if results.is_empty() {
        return HashMap::new();
    }
    let mut sums: HashMap<String, (f32, usize)> = HashMap::new();
    for result in results {
        for (name, value) in &result.metrics {
            if value.is_finite() {
                let entry = sums.entry(name.clone()).or_default();
                entry.0 += value;
                entry.1 += 1;
            }
        }
    }
    let mut summary = HashMap::from([
        ("total_cases".into(), results.len() as f32),
        (
            "avg_execution_time_ms".into(),
            results
                .iter()
                .map(|result| result.execution_time_ms)
                .sum::<f32>()
                / results.len() as f32,
        ),
    ]);
    for (name, (sum, count)) in sums {
        summary.insert(format!("avg_{name}"), sum / count as f32);
    }
    summary
}

fn recommendations_for(run: &EvaluationRun) -> Vec<serde_json::Value> {
    let metrics = &run.metrics_summary;
    let mut recommendations = Vec::new();
    if metrics.get("avg_precision").copied().unwrap_or(1.0) < 0.7 {
        recommendations.push(serde_json::json!({
            "issue": "Low Precision",
            "severity": "high",
            "suggestions": [
                "Increase similarity_threshold",
                "Enable reranking",
                "Reduce top_k"
            ]
        }));
    }
    if metrics.get("avg_recall").copied().unwrap_or(1.0) < 0.7 {
        recommendations.push(serde_json::json!({
            "issue": "Low Recall",
            "severity": "high",
            "suggestions": [
                "Decrease similarity_threshold",
                "Increase top_k",
                "Review chunk size and overlap"
            ]
        }));
    }
    if metrics.get("avg_mrr").copied().unwrap_or(1.0) < 0.5 {
        recommendations.push(serde_json::json!({
            "issue": "Low Ranking Quality",
            "severity": "medium",
            "suggestions": ["Enable or tune reranking", "Review embedding model quality"]
        }));
    }
    recommendations
}

fn validate_state(state: &EvaluationState) -> anyhow::Result<()> {
    let dataset_ids: HashSet<_> = state.datasets.iter().map(|item| item.id.as_str()).collect();
    let case_ids: HashSet<_> = state.cases.iter().map(|item| item.id.as_str()).collect();
    let run_ids: HashSet<_> = state.runs.iter().map(|item| item.id.as_str()).collect();
    if dataset_ids.len() != state.datasets.len()
        || case_ids.len() != state.cases.len()
        || run_ids.len() != state.runs.len()
    {
        anyhow::bail!("duplicate evaluation id");
    }
    if state
        .cases
        .iter()
        .any(|case| !dataset_ids.contains(case.dataset_id.as_str()))
        || state
            .runs
            .iter()
            .any(|run| !dataset_ids.contains(run.dataset_id.as_str()))
        || state.results.iter().any(|result| {
            !run_ids.contains(result.run_id.as_str()) || !case_ids.contains(result.case_id.as_str())
        })
    {
        anyhow::bail!("orphan evaluation record");
    }
    Ok(())
}

fn deduplicate(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect()
}

fn default_top_k() -> usize {
    10
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn bad_request(message: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 400, "message": message })),
    )
        .into_response()
}

fn not_found(message: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "code": 404, "message": message })),
    )
        .into_response()
}

fn server_error(message: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "code": 500, "message": message })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieval_metrics_match_ragflow_formulas() {
        let retrieved = vec!["x".into(), "b".into(), "a".into()];
        let relevant = vec!["a".into(), "b".into(), "c".into()];
        let metrics = retrieval_metrics(&retrieved, &relevant, &[], &[]);
        assert!((metrics["precision"] - 2.0 / 3.0).abs() < 1e-6);
        assert!((metrics["recall"] - 2.0 / 3.0).abs() < 1e-6);
        assert!((metrics["f1_score"] - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(metrics["hit_rate"], 1.0);
        assert_eq!(metrics["mrr"], 0.5);
    }

    #[test]
    fn persistence_failure_rolls_back_dataset_creation() {
        let root = std::env::temp_dir().join(format!("rayrag-eval-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("evaluations.json");
        let store = EvaluationStore::new(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(
            store
                .create_dataset(
                    "owner",
                    DatasetRequest {
                        name: "Evaluation".into(),
                        description: String::new(),
                        kb_ids: vec!["kb-a".into()],
                    },
                )
                .is_err()
        );
        assert!(store.list_datasets("owner", false).is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn evaluation_state_survives_restart_and_dataset_delete_cascades() {
        let root =
            std::env::temp_dir().join(format!("rayrag-eval-restart-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("evaluations.json");
        let store = EvaluationStore::new(&path).unwrap();
        let dataset = store
            .create_dataset(
                "owner",
                DatasetRequest {
                    name: "Evaluation".into(),
                    description: "Restart".into(),
                    kb_ids: vec!["kb-a".into()],
                },
            )
            .unwrap();
        let case = store
            .add_case(
                &dataset.id,
                "owner",
                false,
                CaseRequest {
                    question: "water quality".into(),
                    reference_answer: None,
                    relevant_doc_ids: vec!["doc-a".into()],
                    relevant_chunk_ids: vec!["chunk-a".into()],
                    metadata: HashMap::new(),
                },
            )
            .unwrap()
            .unwrap();
        let (run, _) = store
            .begin_run(
                &dataset,
                RunRequest {
                    name: Some("Baseline".into()),
                    top_k: 5,
                    vector_weight: 0.0,
                },
            )
            .unwrap();
        store
            .finish_run(
                &run.id,
                vec![EvaluationResult {
                    id: "result-a".into(),
                    run_id: run.id.clone(),
                    case_id: case.id,
                    retrieved_chunk_ids: vec!["chunk-a".into()],
                    retrieved_doc_ids: vec!["doc-a".into()],
                    metrics: HashMap::from([("mrr".into(), 1.0)]),
                    execution_time_ms: 1.0,
                    created_at: now_ms(),
                }],
            )
            .unwrap();
        drop(store);

        let restored = EvaluationStore::new(&path).unwrap();
        assert_eq!(restored.list_datasets("owner", false).len(), 1);
        assert_eq!(
            restored
                .cases_for(&dataset.id, "owner", false)
                .unwrap()
                .len(),
            1
        );
        let (restored_run, restored_results) =
            restored.run_results(&run.id, "owner", false).unwrap();
        assert_eq!(restored_run.status, "COMPLETED");
        assert_eq!(restored_results.len(), 1);
        assert!(
            restored
                .delete_dataset(&dataset.id, "owner", false)
                .unwrap()
        );
        assert!(restored.list_datasets("owner", false).is_empty());
        assert!(restored.run_results(&run.id, "owner", false).is_none());
        drop(restored);

        let after_delete = EvaluationStore::new(&path).unwrap();
        assert!(after_delete.list_datasets("owner", false).is_empty());
        assert!(after_delete.run_results(&run.id, "owner", false).is_none());
        std::fs::remove_dir_all(root).ok();
    }
}
