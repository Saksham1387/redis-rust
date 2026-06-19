use thiserror::Error;

#[derive(Debug, PartialEq)]
pub enum RESPResponse {
    SimpleString(String),
    ErrorString(String),
    Int64(i64),
    BulkString(Option<Vec<u8>>),
    Array(Option<Vec<RESPResponse>>),
}

#[derive(Debug, Error)]
pub enum RespError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Incomplete data")]
    Incomplete,
}
