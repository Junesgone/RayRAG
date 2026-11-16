//! Bounded-cardinality Prometheus metrics for the Rust Agent Canvas runtime.

use anyhow::Context;
use prometheus::{CounterVec, HistogramOpts, HistogramVec, Opts, Registry, TextEncoder};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const RUNTIME_RUST: &str = "rust";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasRunOutcome {
    Success,
    Error,
    Cancelled,
}

impl CanvasRunOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

struct CanvasMetrics {
    registry: Registry,
    runs_total: CounterVec,
    run_duration: HistogramVec,
}

impl CanvasMetrics {
    fn new() -> anyhow::Result<Self> {
        let registry = Registry::new();
        let runs_total = CounterVec::new(
            Opts::new(
                "ragflow_canvas_runs_total",
                "Total canvas runs by runtime mode and outcome.",
            ),
            &["runtime", "outcome"],
        )?;
        let run_duration = HistogramVec::new(
            HistogramOpts::new(
                "ragflow_canvas_run_duration_seconds",
                "Canvas run latency in seconds.",
            ),
            &["runtime"],
        )?;
        registry.register(Box::new(runs_total.clone()))?;
        registry.register(Box::new(run_duration.clone()))?;
        for outcome in [
            CanvasRunOutcome::Success,
            CanvasRunOutcome::Error,
            CanvasRunOutcome::Cancelled,
        ] {
            let _ = runs_total.with_label_values(&[RUNTIME_RUST, outcome.label()]);
        }
        let _ = run_duration.with_label_values(&[RUNTIME_RUST]);
        Ok(Self {
            registry,
            runs_total,
            run_duration,
        })
    }

    fn observe(&self, outcome: CanvasRunOutcome, duration: Duration) {
        self.runs_total
            .with_label_values(&[RUNTIME_RUST, outcome.label()])
            .inc();
        if !duration.is_zero() {
            self.run_duration
                .with_label_values(&[RUNTIME_RUST])
                .observe(duration.as_secs_f64());
        }
    }

    fn encode(&self) -> anyhow::Result<String> {
        let encoder = TextEncoder::new();
        encoder
            .encode_to_string(&self.registry.gather())
            .context("Failed to encode Prometheus metrics")
    }
}

fn canvas_metrics() -> &'static CanvasMetrics {
    static METRICS: OnceLock<CanvasMetrics> = OnceLock::new();
    METRICS.get_or_init(|| CanvasMetrics::new().expect("static Canvas metrics are valid"))
}

pub fn encode_prometheus_metrics() -> anyhow::Result<String> {
    canvas_metrics().encode()
}

#[cfg(test)]
pub(crate) fn canvas_run_count(outcome: CanvasRunOutcome) -> f64 {
    canvas_metrics()
        .runs_total
        .with_label_values(&[RUNTIME_RUST, outcome.label()])
        .get()
}

pub struct CanvasRunMetricGuard {
    started_at: Instant,
    finished: bool,
}

impl CanvasRunMetricGuard {
    pub fn start() -> Self {
        // Instantiate and register collectors before execution so a cancelled
        // future is still observable when this guard is dropped.
        let _ = canvas_metrics();
        Self {
            started_at: Instant::now(),
            finished: false,
        }
    }

    pub fn finish(mut self, outcome: CanvasRunOutcome) {
        canvas_metrics().observe(outcome, self.started_at.elapsed());
        self.finished = true;
    }
}

impl Drop for CanvasRunMetricGuard {
    fn drop(&mut self) {
        if !self.finished {
            canvas_metrics().observe(CanvasRunOutcome::Cancelled, self.started_at.elapsed());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_metrics_keep_labels_closed_and_skip_zero_duration() {
        let metrics = CanvasMetrics::new().unwrap();
        metrics.observe(CanvasRunOutcome::Success, Duration::from_millis(250));
        metrics.observe(CanvasRunOutcome::Success, Duration::from_millis(500));
        metrics.observe(CanvasRunOutcome::Error, Duration::ZERO);
        let encoded = metrics.encode().unwrap();

        assert!(
            encoded.contains("ragflow_canvas_runs_total{outcome=\"success\",runtime=\"rust\"} 2")
        );
        assert!(
            encoded.contains("ragflow_canvas_runs_total{outcome=\"error\",runtime=\"rust\"} 1")
        );
        assert!(encoded.contains("ragflow_canvas_run_duration_seconds_count{runtime=\"rust\"} 2"));
        assert!(!encoded.contains("runtime=\"python\""));
    }

    #[test]
    fn unfinished_guard_records_cancellation() {
        let before = encode_prometheus_metrics().unwrap();
        {
            let _guard = CanvasRunMetricGuard::start();
        }
        let after = encode_prometheus_metrics().unwrap();
        assert_ne!(before, after);
        assert!(after.contains("outcome=\"cancelled\",runtime=\"rust\""));
    }
}
