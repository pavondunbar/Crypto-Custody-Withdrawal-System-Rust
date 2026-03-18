-- Accounts: immutable identity records.
-- Balances are derived from ledger_entries, never stored directly.
CREATE TABLE accounts(
  id UUID PRIMARY KEY,
  user_id UUID NOT NULL,
  asset VARCHAR(10) NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE(user_id, asset)
);

-- Transactions: immutable facts recorded at withdrawal initiation.
-- Current status is derived from transaction_events.
CREATE TABLE transactions(
  id UUID PRIMARY KEY,
  account_id UUID NOT NULL REFERENCES accounts(id),
  type VARCHAR(20) NOT NULL,
  amount DECIMAL(38,18) NOT NULL,
  destination_address VARCHAR(256),
  idempotency_key VARCHAR(256) UNIQUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Append-only event log for transaction lifecycle.
-- Current state = latest event per transaction.
-- Events:
--   pending_policy → created, awaiting policy check
--   approved       → policy approved
--   rejected       → policy rejected (terminal)
--   signed         → signing service processed
--   broadcast      → submitted to blockchain
--   confirmed      → on-chain confirmation (terminal)
--   failed         → unrecoverable failure (terminal)
CREATE TABLE transaction_events(
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  transaction_id UUID NOT NULL REFERENCES transactions(id),
  status VARCHAR(20) NOT NULL,
  tx_hash VARCHAR(256),
  block_number BIGINT,
  policy_check_result JSONB,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Append-only ledger: every balance change is an immutable entry.
-- balance = SUM(amount), locked = SUM(locked_delta).
-- Entry types:
--   deposit           → amount > 0, locked_delta = 0
--   withdrawal_lock   → amount = 0, locked_delta > 0
--   withdrawal_unlock → amount = 0, locked_delta < 0
--   withdrawal_settle → amount < 0, locked_delta < 0
CREATE TABLE ledger_entries(
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  account_id UUID NOT NULL REFERENCES accounts(id),
  entry_type VARCHAR(30) NOT NULL,
  amount DECIMAL(38,18) NOT NULL DEFAULT 0,
  locked_delta DECIMAL(38,18) NOT NULL DEFAULT 0,
  transaction_id UUID REFERENCES transactions(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE outbox_events(
  id UUID PRIMARY KEY,
  aggregate_id VARCHAR(256) NOT NULL,
  event_type VARCHAR(64) NOT NULL,
  payload JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  published_at TIMESTAMPTZ
);

-- -------------------------------------------------------
-- Append-only enforcement
-- -------------------------------------------------------

CREATE OR REPLACE FUNCTION prevent_append_only_mutation()
RETURNS TRIGGER AS $$
BEGIN
  RAISE EXCEPTION
    '% is append-only: % not allowed', TG_TABLE_NAME, TG_OP;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER enforce_accounts_append_only
BEFORE UPDATE OR DELETE ON accounts
FOR EACH ROW EXECUTE FUNCTION prevent_append_only_mutation();

CREATE TRIGGER enforce_transactions_append_only
BEFORE UPDATE OR DELETE ON transactions
FOR EACH ROW EXECUTE FUNCTION prevent_append_only_mutation();

CREATE TRIGGER enforce_transaction_events_append_only
BEFORE UPDATE OR DELETE ON transaction_events
FOR EACH ROW EXECUTE FUNCTION prevent_append_only_mutation();

CREATE TRIGGER enforce_ledger_append_only
BEFORE UPDATE OR DELETE ON ledger_entries
FOR EACH ROW EXECUTE FUNCTION prevent_append_only_mutation();

-- -------------------------------------------------------
-- Derived views
-- -------------------------------------------------------

-- Derived balances: always consistent with the ledger.
CREATE VIEW account_balances AS
SELECT
    a.id AS account_id,
    a.user_id,
    a.asset,
    COALESCE(SUM(le.amount), 0) AS balance,
    COALESCE(SUM(le.locked_delta), 0) AS locked_balance
FROM accounts a
LEFT JOIN ledger_entries le ON le.account_id = a.id
GROUP BY a.id, a.user_id, a.asset;

-- Derived transaction state: latest event per transaction.
CREATE VIEW current_transactions AS
SELECT
    t.id,
    t.account_id,
    t.type,
    t.amount,
    t.destination_address,
    t.idempotency_key,
    t.created_at,
    latest.status,
    latest.tx_hash,
    latest.block_number,
    latest.policy_check_result,
    (SELECT MIN(te2.created_at)
     FROM transaction_events te2
     WHERE te2.transaction_id = t.id
       AND te2.status = 'confirmed') AS confirmed_at
FROM transactions t
JOIN LATERAL (
    SELECT status, tx_hash, block_number, policy_check_result
    FROM transaction_events
    WHERE transaction_id = t.id
    ORDER BY created_at DESC
    LIMIT 1
) latest ON true;

-- -------------------------------------------------------
-- Indexes
-- -------------------------------------------------------

CREATE INDEX idx_transaction_events_lookup
ON transaction_events(transaction_id, created_at DESC);

CREATE INDEX idx_ledger_account
ON ledger_entries(account_id, created_at);

CREATE INDEX idx_outbox_unpublished
ON outbox_events(created_at)
WHERE published_at IS NULL;
