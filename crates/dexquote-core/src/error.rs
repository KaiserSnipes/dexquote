use thiserror::Error;

#[derive(Debug, Error)]
pub enum DexQuoteError {
    #[error("unknown token symbol `{0}` on chain {1}")]
    UnknownSymbol(String, &'static str),

    #[error("invalid token input `{0}`: {1}")]
    InvalidTokenInput(String, String),

    #[error("invalid amount `{0}`: {1}")]
    InvalidAmount(String, String),

    #[error("unsupported chain `{0}`")]
    UnsupportedChain(String),

    #[error("no route found for {backend}")]
    NoRoute { backend: &'static str },

    #[error("backend {backend} timed out after {ms}ms")]
    Timeout { backend: &'static str, ms: u64 },

    #[error("rpc endpoint required for on-chain backend {0}")]
    RpcRequired(&'static str),

    #[error("rpc error in {backend}: {source}")]
    Rpc {
        backend: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("http error in {backend}: {source}")]
    Http {
        backend: &'static str,
        #[source]
        source: reqwest::Error,
    },

    #[error("decode error in {backend}: {message}")]
    Decode {
        backend: &'static str,
        message: String,
    },
}

impl DexQuoteError {
    pub fn rpc<E>(backend: &'static str, err: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Rpc {
            backend,
            source: Box::new(err),
        }
    }

    pub fn decode(backend: &'static str, message: impl Into<String>) -> Self {
        Self::Decode {
            backend,
            message: message.into(),
        }
    }
}
