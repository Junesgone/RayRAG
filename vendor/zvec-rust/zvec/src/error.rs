use std::ffi::{CStr, CString};
use std::fmt;
use std::os::raw::c_void;

/// Error codes returned by zvec operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NotFound,
    AlreadyExists,
    InvalidArgument,
    PermissionDenied,
    FailedPrecondition,
    ResourceExhausted,
    Unavailable,
    InternalError,
    NotSupported,
    Unknown,
}

impl From<u32> for ErrorCode {
    fn from(code: u32) -> Self {
        match code {
            1 => ErrorCode::NotFound,
            2 => ErrorCode::AlreadyExists,
            3 => ErrorCode::InvalidArgument,
            4 => ErrorCode::PermissionDenied,
            5 => ErrorCode::FailedPrecondition,
            6 => ErrorCode::ResourceExhausted,
            7 => ErrorCode::Unavailable,
            8 => ErrorCode::InternalError,
            9 => ErrorCode::NotSupported,
            _ => ErrorCode::Unknown,
        }
    }
}

impl From<ErrorCode> for u32 {
    fn from(code: ErrorCode) -> Self {
        match code {
            ErrorCode::NotFound => 1,
            ErrorCode::AlreadyExists => 2,
            ErrorCode::InvalidArgument => 3,
            ErrorCode::PermissionDenied => 4,
            ErrorCode::FailedPrecondition => 5,
            ErrorCode::ResourceExhausted => 6,
            ErrorCode::Unavailable => 7,
            ErrorCode::InternalError => 8,
            ErrorCode::NotSupported => 9,
            ErrorCode::Unknown => 10,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorCode::NotFound => write!(f, "NotFound"),
            ErrorCode::AlreadyExists => write!(f, "AlreadyExists"),
            ErrorCode::InvalidArgument => write!(f, "InvalidArgument"),
            ErrorCode::PermissionDenied => write!(f, "PermissionDenied"),
            ErrorCode::FailedPrecondition => write!(f, "FailedPrecondition"),
            ErrorCode::ResourceExhausted => write!(f, "ResourceExhausted"),
            ErrorCode::Unavailable => write!(f, "Unavailable"),
            ErrorCode::InternalError => write!(f, "InternalError"),
            ErrorCode::NotSupported => write!(f, "NotSupported"),
            ErrorCode::Unknown => write!(f, "Unknown"),
        }
    }
}

/// An error returned by zvec operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    /// Returns `true` if this is a "not found" error.
    pub fn is_not_found(&self) -> bool {
        self.code == ErrorCode::NotFound
    }

    /// Returns `true` if this is an "already exists" error.
    pub fn is_already_exists(&self) -> bool {
        self.code == ErrorCode::AlreadyExists
    }

    /// Returns `true` if this is an "invalid argument" error.
    pub fn is_invalid_argument(&self) -> bool {
        self.code == ErrorCode::InvalidArgument
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "zvec error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

/// A specialized `Result` type for zvec operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Convert a C error code to a Rust `Result`.
///
/// Returns `Ok(())` if the code is `ZVEC_OK`, otherwise fetches the last error
/// message from the C library and returns an appropriate `Error`.
pub(crate) fn check_error(code: zvec_rust_sys::zvec_error_code_t) -> Result<()> {
    if code == zvec_rust_sys::ZVEC_OK {
        return Ok(());
    }

    let message = unsafe {
        let mut c_msg: *mut std::os::raw::c_char = std::ptr::null_mut();
        zvec_rust_sys::zvec_get_last_error(&mut c_msg);

        let msg = if c_msg.is_null() {
            "unknown error".to_string()
        } else {
            let s = CStr::from_ptr(c_msg).to_string_lossy().into_owned();
            zvec_rust_sys::zvec_free(c_msg as *mut c_void);
            s
        };
        msg
    };

    Err(Error {
        code: ErrorCode::from(code),
        message,
    })
}

/// Create a `CString` from a `&str`, returning an `InvalidArgument` error
/// if the string contains a null byte.
pub(crate) fn to_cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error {
        code: ErrorCode::InvalidArgument,
        message: "string contains null byte".into(),
    })
}


