pub use anyhow::Error;

/// Alias for a type-erased error type.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub type Result<T, E = Error> = ::std::result::Result<T, E>;

#[derive(Debug)]
pub struct Error1 {
    inner: Box<Inner>,
}

#[derive(Debug)]
struct Inner {
    kind: Kind,
    source: Option<BoxError>,
    // backtrace:
}

#[derive(Debug)]
enum Kind {
    HttpStatus(u16),
    Timeout,
    Request,
    JsonRpcError,
    RpcResponse,
    ChainId,
    StaleResponse,
    Batch,
    Decode,
    InvalidProof,
    NeedSync,
    StateStore,
    Unknown,
}

impl std::fmt::Display for Error1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for Error1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.source.as_ref().map(|e| &**e as _)
    }
}
