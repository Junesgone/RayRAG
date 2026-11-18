use std::ffi::CStr;

use crate::error::{check_error, Error, ErrorCode, Result};
use crate::types::LogLevel;

/// Configuration builder for initializing the zvec library.
///
/// This struct collects configuration values without touching the C library,
/// making it safe to use even before the library is initialized.
///
/// # Example
/// ```no_run
/// use zvec_rust::ConfigBuilder;
///
/// let config = ConfigBuilder::new()
///     .memory_limit(1024 * 1024 * 1024)
///     .num_threads(4)
///     .enable_console_log(true)
///     .build();
/// ```
pub struct ConfigBuilder {
    /// Memory limit in bytes (0 = use library default).
    pub memory_limit: u64,
    /// Number of threads for query and optimize (0 = use library default).
    pub num_threads: u32,
    /// Whether to enable console logging at Info level.
    pub enable_console_log: bool,
    /// FTS brute-force-by-keys ratio (None = use library default).
    pub fts_brute_force_by_keys_ratio: Option<f32>,
}

impl ConfigBuilder {
    /// Creates a new builder with default values.
    pub fn new() -> Self {
        ConfigBuilder {
            memory_limit: 0,
            num_threads: 0,
            enable_console_log: false,
            fts_brute_force_by_keys_ratio: None,
        }
    }

    /// Sets the memory limit in bytes.
    pub fn memory_limit(mut self, bytes: u64) -> Self {
        self.memory_limit = bytes;
        self
    }

    /// Sets the number of threads for both query and optimize.
    pub fn num_threads(mut self, count: u32) -> Self {
        self.num_threads = count;
        self
    }

    /// Enables or disables console logging at Info level.
    pub fn enable_console_log(mut self, enable: bool) -> Self {
        self.enable_console_log = enable;
        self
    }

    /// Sets the FTS brute-force-by-keys ratio.
    pub fn fts_brute_force_by_keys_ratio(mut self, ratio: f32) -> Self {
        self.fts_brute_force_by_keys_ratio = Some(ratio);
        self
    }

    /// Finalizes the builder configuration.
    ///
    /// This is a no-op that returns `self` for API consistency. The builder
    /// is a plain-data struct — no C resources are allocated here.
    /// To apply the configuration, pass the result to [`initialize`].
    pub fn build(self) -> Self {
        self
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Low-level configuration handle wrapping the C API config.
pub(crate) struct ConfigData {
    pub(crate) handle: *mut zvec_rust_sys::zvec_config_data_t,
}

impl ConfigData {
    pub(crate) fn new() -> Result<Self> {
        let handle = unsafe { zvec_rust_sys::zvec_config_data_create() };
        if handle.is_null() {
            return Err(Error {
                code: ErrorCode::InternalError,
                message: "failed to create config data".into(),
            });
        }
        Ok(ConfigData { handle })
    }

    pub(crate) fn set_memory_limit(&mut self, bytes: u64) -> Result<()> {
        check_error(unsafe { zvec_rust_sys::zvec_config_data_set_memory_limit(self.handle, bytes) })
    }

    pub(crate) fn set_query_thread_count(&mut self, count: u32) -> Result<()> {
        check_error(unsafe {
            zvec_rust_sys::zvec_config_data_set_query_thread_count(self.handle, count)
        })
    }

    pub(crate) fn set_optimize_thread_count(&mut self, count: u32) -> Result<()> {
        check_error(unsafe {
            zvec_rust_sys::zvec_config_data_set_optimize_thread_count(self.handle, count)
        })
    }

    pub(crate) fn set_fts_brute_force_by_keys_ratio(&mut self, ratio: f32) -> Result<()> {
        check_error(unsafe {
            zvec_rust_sys::zvec_config_data_set_fts_brute_force_by_keys_ratio(self.handle, ratio)
        })
    }

    pub(crate) fn set_console_log(&mut self, level: LogLevel) -> Result<()> {
        let log_config = unsafe { zvec_rust_sys::zvec_config_log_create_console(level as u32) };
        if log_config.is_null() {
            return Err(Error {
                code: ErrorCode::InternalError,
                message: "failed to create console log config".into(),
            });
        }
        // Ownership of log_config transfers to config_data on success.
        // On failure, we must free it manually to avoid a leak.
        let result = check_error(unsafe {
            zvec_rust_sys::zvec_config_data_set_log_config(self.handle, log_config)
        });
        if result.is_err() {
            unsafe { zvec_rust_sys::zvec_config_log_destroy(log_config) };
        }
        result
    }
}

impl Drop for ConfigData {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // Safety: handle was created by zvec_config_data_create
            unsafe { zvec_rust_sys::zvec_config_data_destroy(self.handle) };
        }
    }
}

/// Initializes the zvec library with optional configuration.
///
/// Pass `None` to use default configuration, or provide a [`ConfigBuilder`]
/// to customize memory limits, thread counts, and logging.
///
/// # Examples
///
/// ```no_run
/// use zvec_rust::*;
///
/// // Default initialization
/// initialize(None)?;
///
/// // With builder
/// let config = ConfigBuilder::new()
///     .memory_limit(1024 * 1024 * 1024)
///     .num_threads(4)
///     .build();
/// initialize(Some(&config))?;
/// # Ok::<(), zvec_rust::Error>(())
/// ```
pub fn initialize(config: Option<&ConfigBuilder>) -> Result<()> {
    match config {
        None => check_error(unsafe { zvec_rust_sys::zvec_initialize(std::ptr::null()) }),
        Some(builder) => {
            let mut cfg = ConfigData::new()?;
            if builder.memory_limit > 0 {
                cfg.set_memory_limit(builder.memory_limit)?;
            }
            if builder.num_threads > 0 {
                cfg.set_query_thread_count(builder.num_threads)?;
                cfg.set_optimize_thread_count(builder.num_threads)?;
            }
            if builder.enable_console_log {
                cfg.set_console_log(LogLevel::Info)?;
            }
            if let Some(ratio) = builder.fts_brute_force_by_keys_ratio {
                cfg.set_fts_brute_force_by_keys_ratio(ratio)?;
            }
            check_error(unsafe { zvec_rust_sys::zvec_initialize(cfg.handle as *const _) })
        }
    }
}

/// Shuts down the zvec library and releases all resources.
#[doc(hidden)]
pub fn shutdown() -> Result<()> {
    check_error(unsafe { zvec_rust_sys::zvec_shutdown() })
}

/// Returns `true` if the library has been initialized.
pub fn is_initialized() -> bool {
    unsafe { zvec_rust_sys::zvec_is_initialized() }
}

/// Returns the library version string.
pub fn version() -> String {
    unsafe {
        let ptr = zvec_rust_sys::zvec_get_version();
        if ptr.is_null() {
            return String::new();
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// Checks if the current library version meets the minimum requirements.
pub fn check_version(major: i32, minor: i32, patch: i32) -> bool {
    unsafe { zvec_rust_sys::zvec_check_version(major, minor, patch) }
}

/// Returns the major version number.
pub fn version_major() -> i32 {
    unsafe { zvec_rust_sys::zvec_get_version_major() }
}

/// Returns the minor version number.
pub fn version_minor() -> i32 {
    unsafe { zvec_rust_sys::zvec_get_version_minor() }
}

/// Returns the patch version number.
pub fn version_patch() -> i32 {
    unsafe { zvec_rust_sys::zvec_get_version_patch() }
}

/// Returns the I/O backend type code used for DiskAnn disk reads.
///
/// Compare against the `ZVEC_IO_BACKEND_TYPE_*` constants in [`crate::sys`]:
/// Linux selects io_uring, then libaio, then synchronous pread; macOS ARM64
/// uses pread; Windows uses overlapped I/O. Use [`io_backend_type_name`] for
/// the matching name.
pub fn io_backend_type() -> u32 {
    unsafe { zvec_rust_sys::zvec_get_io_backend_type() }
}

/// Returns the name of an I/O backend type code.
///
/// One of `"io_uring"`, `"libaio"`, `"pread"`, `"windows_overlapped"`, or
/// `"unknown"` for an unrecognized code.
pub fn io_backend_type_name(backend_type: u32) -> String {
    unsafe {
        let ptr = zvec_rust_sys::zvec_get_io_backend_type_name(backend_type);
        if ptr.is_null() {
            return String::new();
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// Returns a human-readable description of the active I/O backend.
///
/// On Linux the pread description also explains why io_uring and libaio were
/// unavailable. The returned string is a copy of a thread-local buffer owned
/// by the zvec library.
pub fn io_backend_description() -> String {
    unsafe {
        let ptr = zvec_rust_sys::zvec_get_io_backend_description();
        if ptr.is_null() {
            return String::new();
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}


