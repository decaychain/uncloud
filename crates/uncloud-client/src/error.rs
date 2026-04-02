use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HTTP {status}: {message}")]
    Api { status: u16, message: String },

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("not authenticated")]
    Unauthenticated,

    #[error("I/O error: {0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

impl ClientError {
    /// Build an `Api` error by reading the response body for an error message.
    pub(crate) async fn api(resp: reqwest::Response) -> Self {
        let status = resp.status().as_u16();
        let message = resp
            .text()
            .await
            .unwrap_or_else(|_| "unknown error".to_owned());
        ClientError::Api { status, message }
    }
}
