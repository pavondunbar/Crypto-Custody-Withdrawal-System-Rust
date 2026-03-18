use rust_decimal::Decimal;
use serde_json::json;
use sqlx::{PgPool, Row};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::{
    errors::WithdrawalError,
    models::{
        PolicyVerdict, SigningMessage, Transaction, TransactionStatus, WithdrawalRequest,
    },
    ports::{PolicyEngine, SigningQueue},
};

pub struct WithdrawalService {
    db: PgPool,
    policy_engine: Box<dyn PolicyEngine>,
    signing_queue: Box<dyn SigningQueue>,
}

impl WithdrawalService {
    pub fn new(
        db: PgPool,
        policy_engine: Box<dyn PolicyEngine>,
        signing_queue: Box<dyn SigningQueue>,
    ) -> Self {
        Self { db, policy_engine, signing_queue }
    }

    /// Entry point. Returns the transaction record, whether new or a replay.
    #[instrument(skip(self), fields(
        user_id = %req.user_id,
        asset   = %req.asset,
        amount  = %req.amount,
    ))]
    pub async fn process_withdrawal(
        &self,
        req: WithdrawalRequest,
    ) -> Result<Transaction, WithdrawalError> {

        // ---- STEP 1: Idempotency check ----------------------------------------
        // Must be FIRST — before any writes.
        if let Some(existing) = self.find_by_idempotency_key(&req.idempotency_key).await? {
            info!(
                idempotency_key = %req.idempotency_key,
                transaction_id  = %existing.id,
                "duplicate request — returning existing transaction"
            );
            return Ok(existing);
        }

        // ---- STEP 2: Address validation ----------------------------------------
        self.validate_address(&req.asset, &req.destination_address)?;

        // ---- STEP 3: Atomic fund lock + ledger writes -------------------------
        let (account_id, tx) = self
            .lock_funds_and_create_transaction(&req)
            .await?;

        // ---- STEP 4: Policy evaluation ----------------------------------------
        // OUTSIDE the DB transaction — never hold a row lock during external calls.
        let decision = self.policy_engine.evaluate(&tx).await?;

        if decision.decision == PolicyVerdict::Rejected {
            let reason = decision.reason.unwrap_or_else(|| "no reason given".into());
            warn!(
                transaction_id = %tx.id,
                %reason,
                "transaction rejected by policy engine — refunding lock"
            );
            self.refund_and_reject(&tx, account_id, req.amount).await?;
            return Err(WithdrawalError::PolicyRejected { reason });
        }

        // ---- STEP 5: Enqueue for signing --------------------------------------
        // account_id as group ID → per-account FIFO ordering.
        // tx.id as deduplication ID → safe to retry on transient errors.
        self.signing_queue
            .send(
                SigningMessage {
                    transaction_id: tx.id.to_string(),
                    asset: req.asset.clone(),
                    amount: req.amount.to_string(),
                    destination: req.destination_address.clone(),
                },
                account_id.to_string(),
                tx.id.to_string(),
            )
            .await?;

        info!(transaction_id = %tx.id, "withdrawal enqueued for signing");
        Ok(tx)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    async fn find_by_idempotency_key(
        &self,
        key: &str,
    ) -> Result<Option<Transaction>, WithdrawalError> {
        let row = sqlx::query(
            r#"
            SELECT id, account_id, type, amount, status,
                   destination_address, tx_hash, block_number,
                   idempotency_key, policy_check_result,
                   created_at, confirmed_at
            FROM current_transactions
            WHERE idempotency_key = $1
            "#,
        )
        .bind(key)
        .fetch_optional(&self.db)
        .await?;

        Ok(row.map(|r| map_transaction_row(&r)))
    }

    fn validate_address(
        &self,
        asset: &str,
        address: &str,
    ) -> Result<(), WithdrawalError> {
        if address.is_empty() {
            return Err(WithdrawalError::InvalidAddress {
                asset: asset.to_string(),
                address: address.to_string(),
            });
        }
        // TODO: plug in per-asset validation (bech32, EIP-55, etc.)
        Ok(())
    }

    /// Acquires a pessimistic row lock on the account, derives the available
    /// balance from the append-only ledger, inserts a lock entry, the
    /// transaction, and an outbox event — all in a single DB transaction.
    async fn lock_funds_and_create_transaction(
        &self,
        req: &WithdrawalRequest,
    ) -> Result<(Uuid, Transaction), WithdrawalError> {
        let mut db_tx = self.db.begin().await?;

        // Pessimistic lock on the identity row — serialises concurrent
        // withdrawals. No balance stored here; it is derived below.
        let acct_row = sqlx::query(
            r#"
            SELECT id
            FROM accounts
            WHERE user_id = $1 AND asset = $2
            FOR UPDATE
            "#,
        )
        .bind(req.user_id)
        .bind(&req.asset)
        .fetch_optional(&mut *db_tx)
        .await?
        .ok_or_else(|| WithdrawalError::AccountNotFound {
            user_id: req.user_id,
            asset: req.asset.clone(),
        })?;

        let account_id: Uuid = acct_row.get("id");

        // Derive balance from the append-only ledger
        let bal_row = sqlx::query(
            r#"
            SELECT COALESCE(SUM(amount), 0)       AS balance,
                   COALESCE(SUM(locked_delta), 0)  AS locked_balance
            FROM ledger_entries
            WHERE account_id = $1
            "#,
        )
        .bind(account_id)
        .fetch_one(&mut *db_tx)
        .await?;

        let balance: Decimal        = bal_row.get("balance");
        let locked_balance: Decimal = bal_row.get("locked_balance");

        let available = balance - locked_balance;
        if available < req.amount {
            return Err(WithdrawalError::InsufficientBalance {
                available,
                requested: req.amount,
            });
        }

        // All inserts are append-only — no row is ever mutated.
        let tx_id = Uuid::new_v4();

        // 1. Immutable transaction record
        sqlx::query(
            r#"
            INSERT INTO transactions
                (id, account_id, type, amount, destination_address, idempotency_key)
            VALUES ($1, $2, 'withdrawal', $3, $4, $5)
            "#,
        )
        .bind(tx_id)
        .bind(account_id)
        .bind(req.amount)
        .bind(&req.destination_address)
        .bind(&req.idempotency_key)
        .execute(&mut *db_tx)
        .await?;

        // 2. Initial status event
        sqlx::query(
            r#"
            INSERT INTO transaction_events (transaction_id, status)
            VALUES ($1, $2)
            "#,
        )
        .bind(tx_id)
        .bind(TransactionStatus::PendingPolicy.as_str())
        .execute(&mut *db_tx)
        .await?;

        // 3. Lock funds via ledger entry
        sqlx::query(
            r#"
            INSERT INTO ledger_entries
                (account_id, entry_type, locked_delta, transaction_id)
            VALUES ($1, 'withdrawal_lock', $2, $3)
            "#,
        )
        .bind(account_id)
        .bind(req.amount)
        .bind(tx_id)
        .execute(&mut *db_tx)
        .await?;

        // 4. Transactional outbox
        let payload = json!({
            "transaction_id": tx_id.to_string(),
            "asset":          req.asset,
            "amount":         req.amount.to_string(),
            "destination":    req.destination_address,
        });

        sqlx::query(
            r#"
            INSERT INTO outbox_events (id, aggregate_id, event_type, payload)
            VALUES ($1, $2, 'withdrawal.pending_policy', $3)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(tx_id.to_string())
        .bind(payload)
        .execute(&mut *db_tx)
        .await?;

        // 5. Read derived state from the view
        let tx_row = sqlx::query(
            r#"
            SELECT id, account_id, type, amount, status,
                   destination_address, tx_hash, block_number,
                   idempotency_key, policy_check_result,
                   created_at, confirmed_at
            FROM current_transactions
            WHERE id = $1
            "#,
        )
        .bind(tx_id)
        .fetch_one(&mut *db_tx)
        .await?;

        let tx = map_transaction_row(&tx_row);

        db_tx.commit().await?;

        info!(
            transaction_id = %tx_id,
            %account_id,
            amount = %req.amount,
            "funds locked via ledger entry"
        );

        Ok((account_id, tx))
    }

    /// Releases locked funds via an append-only ledger entry and records
    /// a rejection event.
    async fn refund_and_reject(
        &self,
        tx: &Transaction,
        account_id: Uuid,
        amount: Decimal,
    ) -> Result<(), WithdrawalError> {
        let mut db_tx = self.db.begin().await?;

        // Append-only: unlock funds via a new ledger row
        sqlx::query(
            r#"
            INSERT INTO ledger_entries
                (account_id, entry_type, locked_delta, transaction_id)
            VALUES ($1, 'withdrawal_unlock', $2, $3)
            "#,
        )
        .bind(account_id)
        .bind(-amount)
        .bind(tx.id)
        .execute(&mut *db_tx)
        .await?;

        // Append-only: record rejection as a new event (no UPDATE)
        sqlx::query(
            r#"
            INSERT INTO transaction_events (transaction_id, status)
            VALUES ($1, $2)
            "#,
        )
        .bind(tx.id)
        .bind(TransactionStatus::Rejected.as_str())
        .execute(&mut *db_tx)
        .await?;

        db_tx.commit().await?;

        info!(transaction_id = %tx.id, "funds unlocked via ledger entry, rejection event recorded");
        Ok(())
    }
}

// -----------------------------------------------------------------------
// Row mapping helper — converts a PgRow into a Transaction struct
// -----------------------------------------------------------------------

fn map_transaction_row(row: &sqlx::postgres::PgRow) -> Transaction {
    Transaction {
        id:                  row.get("id"),
        account_id:          row.get("account_id"),
        r#type:              row.get("type"),
        amount:              row.get("amount"),
        status:              row.get("status"),
        destination_address: row.get("destination_address"),
        tx_hash:             row.get("tx_hash"),
        block_number:        row.get("block_number"),
        idempotency_key:     row.get("idempotency_key"),
        policy_check_result: row.get("policy_check_result"),
        created_at:          row.get("created_at"),
        confirmed_at:        row.get("confirmed_at"),
    }
}
