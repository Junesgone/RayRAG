//! Runtime tracing target-level configuration compatible with RAGFlow's log API.

use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use tracing_subscriber::filter::{LevelFilter, Targets};

const ROOT_TARGET: &str = "root";

type ReloadFn = dyn Fn(Targets) -> Result<(), String> + Send + Sync;

#[derive(Clone)]
pub struct LogLevelManager {
    levels: Arc<RwLock<BTreeMap<String, String>>>,
    reload: Option<Arc<ReloadFn>>,
}

impl LogLevelManager {
    pub fn from_env() -> Self {
        let mut levels = BTreeMap::from([
            ("pdfminer".to_owned(), "WARNING".to_owned()),
            ("peewee".to_owned(), "WARNING".to_owned()),
            (ROOT_TARGET.to_owned(), "INFO".to_owned()),
        ]);
        if let Ok(config) = std::env::var("LOG_LEVELS") {
            for assignment in config.split(',') {
                let Some((target, level)) = assignment.split_once('=') else {
                    continue;
                };
                let target = target.trim();
                if target.is_empty() {
                    continue;
                }
                let canonical = canonical_level(level).unwrap_or("INFO");
                levels.insert(target.to_owned(), canonical.to_owned());
            }
        }
        Self {
            levels: Arc::new(RwLock::new(levels)),
            reload: None,
        }
    }

    pub fn in_memory() -> Self {
        Self::from_levels(BTreeMap::from([(
            ROOT_TARGET.to_owned(),
            "INFO".to_owned(),
        )]))
    }

    fn from_levels(levels: BTreeMap<String, String>) -> Self {
        Self {
            levels: Arc::new(RwLock::new(levels)),
            reload: None,
        }
    }

    fn with_reload(
        mut self,
        reload: impl Fn(Targets) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.reload = Some(Arc::new(reload));
        self
    }

    pub fn levels(&self) -> BTreeMap<String, String> {
        self.levels.read().unwrap().clone()
    }

    pub fn set_level(&self, target: &str, level: &str) -> Result<String, SetLogLevelError> {
        let canonical = canonical_level(level).ok_or(SetLogLevelError::InvalidLevel)?;
        let mut next = self.levels();
        next.insert(target.to_owned(), canonical.to_owned());
        if let Some(reload) = &self.reload {
            reload(targets_for(&next)).map_err(SetLogLevelError::Reload)?;
        }
        *self.levels.write().unwrap() = next;
        Ok(canonical.to_owned())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SetLogLevelError {
    #[error("invalid log level")]
    InvalidLevel,
    #[error("failed to reload tracing filter: {0}")]
    Reload(String),
}

fn canonical_level(level: &str) -> Option<&'static str> {
    match level.trim().to_ascii_uppercase().as_str() {
        "CRITICAL" | "FATAL" => Some("CRITICAL"),
        "ERROR" => Some("ERROR"),
        "WARNING" | "WARN" => Some("WARNING"),
        "INFO" => Some("INFO"),
        "DEBUG" => Some("DEBUG"),
        "NOTSET" => Some("NOTSET"),
        _ => None,
    }
}

fn filter_level(level: &str) -> LevelFilter {
    match level {
        "CRITICAL" | "ERROR" => LevelFilter::ERROR,
        "WARNING" => LevelFilter::WARN,
        "INFO" => LevelFilter::INFO,
        "DEBUG" | "NOTSET" => LevelFilter::DEBUG,
        _ => LevelFilter::INFO,
    }
}

fn targets_for(levels: &BTreeMap<String, String>) -> Targets {
    let mut targets = Targets::new().with_default(filter_level(
        levels
            .get(ROOT_TARGET)
            .map(String::as_str)
            .unwrap_or("INFO"),
    ));
    for (target, level) in levels {
        if target != ROOT_TARGET && level != "NOTSET" {
            targets = targets.with_target(target.clone(), filter_level(level));
        }
    }
    targets
}

pub fn init() -> LogLevelManager {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let manager = LogLevelManager::from_env();
    let (filter, handle) = tracing_subscriber::reload::Layer::new(targets_for(&manager.levels()));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
    manager.with_reload(move |targets| handle.reload(targets).map_err(|error| error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn runtime_level_update_is_canonical_and_reloads_before_commit() {
        let reloads = Arc::new(AtomicUsize::new(0));
        let observed = reloads.clone();
        let manager = LogLevelManager::in_memory().with_reload(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        assert_eq!(
            manager.set_level("rayrag::search", "warn").unwrap(),
            "WARNING"
        );
        assert_eq!(manager.levels()["rayrag::search"], "WARNING");
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
        assert!(matches!(
            manager.set_level("rayrag", "verbose"),
            Err(SetLogLevelError::InvalidLevel)
        ));
    }
}
