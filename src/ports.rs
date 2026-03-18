use async_trait::async_trait;

use crate::{
    errors::WithdrawalError,
    models::{PolicyDecision, SigningMessage, Transaction},
};

/// Evaluates a transaction against compliance and risk policies.
/// Called OUTSIDE database transactions — never hold a row lock during this.
#[async_trait]
pub trait PolicyEngine: Send + Sync {
    async fn evaluate(&self, tx: &Transaction) -> Result<PolicyDecision, WithdrawalError>;
}

/// Publishes transactions to the HSM signing queue (e.g. SQS FIFO).
/// Per-account group ID guarantees FIFO ordering within an account.
#[async_trait]
pub trait SigningQueue: Send + Sync {
    // message_group_id         → SQS MessageGroupId (one queue per account)
    // message_deduplication_id → SQS MessageDeduplicationId (idempotent sends)
    async fn send(
        &self,
        message: SigningMessage,
        message_group_id: String,
        message_deduplication_id: String,
    ) -> Result<(), WithdrawalError>;
}
