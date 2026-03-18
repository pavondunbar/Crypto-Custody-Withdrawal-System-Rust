use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

// -----------------------------------------------
// Enums
// -----------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "VARCHAR", rename_all = "snake_case")]
pub enum TransactionStatus {
    #[sqlx(rename = "pending_policy")]
    PendingPolicy,
    #[sqlx(rename = "approved")]
    Approved,
    #[sqlx(rename = "rejected")]
    Rejected,
    #[sqlx(rename = "signed")]
    Signed,
    #[sqlx(rename = "broadcast")]
    Broadcast,
    #[sqlx(rename = "confirmed")]
    Confirmed,
    #[sqlx(rename = "failed")]
    Failed,
}

impl TransactionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PendingPolicy => "pending_policy",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Signed => "signed",
            Self::Broadcast => "broadcast",
            Self::Confirmed => "confirmed",
            Self::Failed => "failed",
        }
    }
}

// -----------------------------------------------
// Database row types
// -----------------------------------------------

#[derive(Debug, Clone, FromRow)]
#[allow(dead_code)] // maps to accounts table; used by typed queries
pub struct Account {
    pub id: Uuid,
    pub user_id: Uuid,
    pub asset: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
#[allow(dead_code)] // maps to ledger_entries table
pub struct LedgerEntry {
    pub id: Uuid,
    pub account_id: Uuid,
    pub entry_type: String,
    pub amount: Decimal,
    pub locked_delta: Decimal,
    pub transaction_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
#[allow(dead_code)] // maps to account_balances view
pub struct AccountBalance {
    pub account_id: Uuid,
    pub user_id: Uuid,
    pub asset: String,
    pub balance: Decimal,
    pub locked_balance: Decimal,
}

impl AccountBalance {
    #[allow(dead_code)]
    pub fn available(&self) -> Decimal {
        self.balance - self.locked_balance
    }
}

#[derive(Debug, Clone, FromRow)]
#[allow(dead_code)] // maps to transaction_events table
pub struct TransactionEvent {
    pub id: Uuid,
    pub transaction_id: Uuid,
    pub status: String,
    pub tx_hash: Option<String>,
    pub block_number: Option<i64>,
    pub policy_check_result: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
#[allow(dead_code)] // maps to current_transactions view
pub struct Transaction {
    pub id: Uuid,
    pub account_id: Uuid,
    pub r#type: String,
    pub amount: Decimal,
    pub status: String,
    pub destination_address: Option<String>,
    pub tx_hash: Option<String>,
    pub block_number: Option<i64>,
    pub idempotency_key: Option<String>,
    pub policy_check_result: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub confirmed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
#[allow(dead_code)] // maps to outbox_events table; used by typed queries
pub struct OutboxEvent {
    pub id: Uuid,
    pub aggregate_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
}

// -----------------------------------------------
// Service I/O types
// -----------------------------------------------

#[derive(Debug, Clone)]
pub struct WithdrawalRequest {
    pub user_id: Uuid,
    pub asset: String,
    pub amount: Decimal,
    pub destination_address: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub decision: PolicyVerdict,
    pub reason: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyVerdict {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigningMessage {
    pub transaction_id: String,
    pub asset: String,
    pub amount: String,
    pub destination: String,
}
