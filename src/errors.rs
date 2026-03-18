use thiserror::Error;

#[derive(Debug, Error)]
pub enum WithdrawalError {
    #[error("insufficient balance: available {available}, requested {requested}")]
    InsufficientBalance {
        available: rust_decimal::Decimal,
        requested: rust_decimal::Decimal,
    },

    #[error("invalid destination address for asset {asset}: {address}")]
    InvalidAddress { asset: String, address: String },

    #[error("account not found for user {user_id} and asset {asset}")]
    AccountNotFound { user_id: uuid::Uuid, asset: String },

    #[error("transaction rejected by policy engine: {reason}")]
    PolicyRejected { reason: String },

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("signing queue error: {0}")]
    SigningQueue(String),
}
