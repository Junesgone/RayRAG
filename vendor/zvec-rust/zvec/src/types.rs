use std::fmt;

/// Data type of a field in a zvec collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DataType {
    /// Undefined or unknown type.
    Undefined = 0,
    /// Raw binary data (arbitrary bytes).
    Binary = 1,
    /// UTF-8 encoded string.
    String = 2,
    /// Boolean value.
    Bool = 3,
    /// 32-bit signed integer.
    Int32 = 4,
    /// 64-bit signed integer.
    Int64 = 5,
    /// 32-bit unsigned integer.
    Uint32 = 6,
    /// 64-bit unsigned integer.
    Uint64 = 7,
    /// 32-bit floating point (single precision).
    Float = 8,
    /// 64-bit floating point (double precision).
    Double = 9,
    /// Dense binary vector with 32-bit packing.
    VectorBinary32 = 20,
    /// Dense binary vector with 64-bit packing.
    VectorBinary64 = 21,
    /// Dense vector with 16-bit floating point elements (half precision).
    VectorFp16 = 22,
    /// Dense vector with 32-bit floating point elements (single precision).
    VectorFp32 = 23,
    /// Dense vector with 64-bit floating point elements (double precision).
    VectorFp64 = 24,
    /// Dense vector with 4-bit integer elements (packed, 2 values per byte).
    VectorInt4 = 25,
    /// Dense vector with 8-bit integer elements.
    VectorInt8 = 26,
    /// Dense vector with 16-bit integer elements.
    VectorInt16 = 27,
    /// Dense vector with unsigned 8-bit elements, used upstream for Flat
    /// reference storage.
    ///
    /// Not treated as a vector field type: the native layer rejects a schema
    /// that pairs it with vector index params.
    VectorUint8 = 28,
    /// Sparse vector with 16-bit floating point values.
    SparseVectorFp16 = 30,
    /// Sparse vector with 32-bit floating point values.
    SparseVectorFp32 = 31,
    /// Array of binary data.
    ArrayBinary = 40,
    /// Array of strings.
    ArrayString = 41,
    /// Array of booleans.
    ArrayBool = 42,
    /// Array of 32-bit signed integers.
    ArrayInt32 = 43,
    /// Array of 64-bit signed integers.
    ArrayInt64 = 44,
    /// Array of 32-bit unsigned integers.
    ArrayUint32 = 45,
    /// Array of 64-bit unsigned integers.
    ArrayUint64 = 46,
    /// Array of 32-bit floating point values.
    ArrayFloat = 47,
    /// Array of 64-bit floating point values.
    ArrayDouble = 48,
}

impl From<u32> for DataType {
    fn from(value: u32) -> Self {
        match value {
            0 => DataType::Undefined,
            1 => DataType::Binary,
            2 => DataType::String,
            3 => DataType::Bool,
            4 => DataType::Int32,
            5 => DataType::Int64,
            6 => DataType::Uint32,
            7 => DataType::Uint64,
            8 => DataType::Float,
            9 => DataType::Double,
            20 => DataType::VectorBinary32,
            21 => DataType::VectorBinary64,
            22 => DataType::VectorFp16,
            23 => DataType::VectorFp32,
            24 => DataType::VectorFp64,
            25 => DataType::VectorInt4,
            26 => DataType::VectorInt8,
            27 => DataType::VectorInt16,
            28 => DataType::VectorUint8,
            30 => DataType::SparseVectorFp16,
            31 => DataType::SparseVectorFp32,
            40 => DataType::ArrayBinary,
            41 => DataType::ArrayString,
            42 => DataType::ArrayBool,
            43 => DataType::ArrayInt32,
            44 => DataType::ArrayInt64,
            45 => DataType::ArrayUint32,
            46 => DataType::ArrayUint64,
            47 => DataType::ArrayFloat,
            48 => DataType::ArrayDouble,
            _ => DataType::Undefined,
        }
    }
}

impl From<DataType> for u32 {
    fn from(dt: DataType) -> Self {
        dt as u32
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Index type for a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum IndexType {
    /// Undefined or unknown index type.
    Undefined = 0,
    /// Hierarchical Navigable Small World graph index (recommended for most use cases).
    Hnsw = 1,
    /// Inverted File index with clustering.
    Ivf = 2,
    /// Flat (brute-force) index — exact search, no approximation.
    Flat = 3,
    /// DiskANN disk-based graph index for large datasets.
    Diskann = 5,
    /// IVF index with RaBitQ quantization.
    IvfRabitq = 7,
    /// Inverted index for scalar field filtering.
    Invert = 10,
    /// Full-text search index.
    Fts = 11,
}

impl From<u32> for IndexType {
    fn from(value: u32) -> Self {
        match value {
            1 => IndexType::Hnsw,
            2 => IndexType::Ivf,
            3 => IndexType::Flat,
            5 => IndexType::Diskann,
            7 => IndexType::IvfRabitq,
            10 => IndexType::Invert,
            11 => IndexType::Fts,
            _ => IndexType::Undefined,
        }
    }
}

impl From<IndexType> for u32 {
    fn from(it: IndexType) -> Self {
        it as u32
    }
}

impl fmt::Display for IndexType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Distance metric type for vector similarity search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MetricType {
    /// Undefined or unknown metric type.
    Undefined = 0,
    /// Euclidean distance (L2 norm). Smaller values indicate higher similarity.
    L2 = 1,
    /// Inner product. Larger values indicate higher similarity.
    Ip = 2,
    /// Cosine similarity. Values range from -1 to 1; closer to 1 means more similar.
    Cosine = 3,
    /// Maximum Inner Product Search with L2 normalization.
    MipsL2 = 4,
}

impl From<u32> for MetricType {
    fn from(value: u32) -> Self {
        match value {
            1 => MetricType::L2,
            2 => MetricType::Ip,
            3 => MetricType::Cosine,
            4 => MetricType::MipsL2,
            _ => MetricType::Undefined,
        }
    }
}

impl From<MetricType> for u32 {
    fn from(mt: MetricType) -> Self {
        mt as u32
    }
}

impl fmt::Display for MetricType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Quantization type for vector indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QuantizeType {
    /// No quantization.
    Undefined = 0,
    /// 16-bit floating point quantization (half precision).
    Fp16 = 1,
    /// 8-bit integer quantization.
    Int8 = 2,
    /// 4-bit integer quantization.
    Int4 = 3,
    /// RaBitQ quantization.
    Rabitq = 4,
}

impl From<u32> for QuantizeType {
    fn from(value: u32) -> Self {
        match value {
            1 => QuantizeType::Fp16,
            2 => QuantizeType::Int8,
            3 => QuantizeType::Int4,
            4 => QuantizeType::Rabitq,
            _ => QuantizeType::Undefined,
        }
    }
}

impl From<QuantizeType> for u32 {
    fn from(qt: QuantizeType) -> Self {
        qt as u32
    }
}

impl fmt::Display for QuantizeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Log level for the zvec library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum LogLevel {
    /// Verbose debug messages.
    Debug = 0,
    /// Informational messages.
    Info = 1,
    /// Warning messages for potentially harmful situations.
    Warn = 2,
    /// Error messages for failure events.
    Error = 3,
    /// Fatal messages indicating imminent abort.
    Fatal = 4,
}

impl From<u32> for LogLevel {
    fn from(value: u32) -> Self {
        match value {
            0 => LogLevel::Debug,
            1 => LogLevel::Info,
            2 => LogLevel::Warn,
            3 => LogLevel::Error,
            4 => LogLevel::Fatal,
            _ => LogLevel::Debug,
        }
    }
}

impl From<LogLevel> for u32 {
    fn from(ll: LogLevel) -> Self {
        ll as u32
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Document operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum DocOperator {
    /// Insert a new document (fails if primary key already exists).
    Insert = 0,
    /// Update an existing document (fails if primary key does not exist).
    Update = 1,
    /// Insert or update a document (creates if not exists, updates if exists).
    Upsert = 2,
    /// Delete a document by primary key.
    Delete = 3,
}

impl From<u32> for DocOperator {
    fn from(value: u32) -> Self {
        match value {
            0 => DocOperator::Insert,
            1 => DocOperator::Update,
            2 => DocOperator::Upsert,
            3 => DocOperator::Delete,
            _ => DocOperator::Insert,
        }
    }
}

impl From<DocOperator> for u32 {
    fn from(op: DocOperator) -> Self {
        op as u32
    }
}

impl fmt::Display for DocOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}


