mod errors;
mod models;
mod ports;
mod service;

use async_trait::async_trait;
use rust_decimal::Decimal;

use errors::WithdrawalError;
use models::{PolicyDecision, PolicyVerdict, SigningMessage, Transaction};
use ports::{PolicyEngine, SigningQueue};
use service::WithdrawalService;

// -----------------------------------------------------------------------
// Stub adapters — replace with real implementations in production
// -----------------------------------------------------------------------

/// Stub policy engine: approves everything.
/// Real implementation would call an internal compliance service over gRPC/HTTP.
struct StubPolicyEngine;

#[async_trait]
impl PolicyEngine for StubPolicyEngine {
    async fn evaluate(&self, tx: &Transaction) -> Result<PolicyDecision, WithdrawalError> {
        tracing::info!(transaction_id = %tx.id, "policy engine: approved (stub)");
        Ok(PolicyDecision {
            decision: PolicyVerdict::Approved,
            reason: None,
            metadata: None,
        })
    }
}

/// Stub signing queue: logs and discards.
/// Real implementation would call SQS SendMessage with FIFO queue parameters.
struct StubSigningQueue;

#[async_trait]
impl SigningQueue for StubSigningQueue {
    async fn send(
        &self,
        message: SigningMessage,
        message_group_id: String,
        message_deduplication_id: String,
    ) -> Result<(), WithdrawalError> {
        tracing::info!(
            transaction_id        = %message.transaction_id,
            message_group_id      = %message_group_id,
            deduplication_id      = %message_deduplication_id,
            "signing queue: message enqueued (stub)"
        );
        Ok(())
    }
}

// -----------------------------------------------------------------------
// Wiring
// -----------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost/custody".into());

    let pool = sqlx::PgPool::connect(&database_url).await?;

    let service = WithdrawalService::new(
        pool,
        Box::new(StubPolicyEngine),
        Box::new(StubSigningQueue),
    );

    // Example invocation
    let req = models::WithdrawalRequest {
        user_id:             uuid::Uuid::parse_str("a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d").expect("valid hardcoded UUID"),
        asset:               "ETH".into(),
        amount:              Decimal::new(25, 0),          // 25 ETH
        destination_address: "0x123456...abcdef".into(),
        idempotency_key:     uuid::Uuid::new_v4().to_string(),
    };

    match service.process_withdrawal(req).await {
        Ok(tx)   => tracing::info!(transaction_id = %tx.id, status = %tx.status, "withdrawal initiated"),
        Err(err) => tracing::error!(error = %err, "withdrawal failed"),
    }

    Ok(())
}
