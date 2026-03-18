-- -----------------------------------------------
-- SETUP
-- -----------------------------------------------

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- -----------------------------------------------
-- STEP 1: Insert account + seed balance via ledger
-- -----------------------------------------------

INSERT INTO accounts (id, user_id, asset)
VALUES (
    gen_random_uuid(),
    'a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d',
    'ETH'
);

-- Initial deposit: 100 ETH credited through the append-only ledger
INSERT INTO ledger_entries (account_id, entry_type, amount)
SELECT id, 'deposit', 100.000000000000000000
FROM accounts
WHERE asset = 'ETH';

SELECT * FROM account_balances;

-- -----------------------------------------------
-- STEP 2: Atomic withdrawal initiation
-- All inserts, no mutations.
-- -----------------------------------------------

BEGIN;

WITH locked_account AS (
    SELECT id
    FROM accounts
    WHERE asset = 'ETH'
    FOR UPDATE
),

inserted_transaction AS (
    INSERT INTO transactions (
        id, account_id, type, amount,
        destination_address, idempotency_key
    )
    SELECT
        gen_random_uuid(),
        id,
        'withdrawal',
        25.000000000000000000,
        '0x123456abcdef...fedcba654321',
        gen_random_uuid()::VARCHAR
    FROM locked_account
    RETURNING id, amount, account_id
),

-- Record initial status event
inserted_event AS (
    INSERT INTO transaction_events (transaction_id, status)
    SELECT id, 'pending_policy'
    FROM inserted_transaction
    RETURNING transaction_id
),

-- Lock 25 ETH via ledger entry (no balance mutation)
inserted_lock AS (
    INSERT INTO ledger_entries (account_id, entry_type, locked_delta, transaction_id)
    SELECT account_id, 'withdrawal_lock', amount, id
    FROM inserted_transaction
    RETURNING id
),

inserted_outbox AS (
    INSERT INTO outbox_events (
        id, aggregate_id, event_type, payload
    )
    SELECT
        gen_random_uuid(),
        id::VARCHAR,
        'withdrawal.pending_policy',
        jsonb_build_object(
            'transaction_id', id::VARCHAR,
            'amount', amount::VARCHAR,
            'destination', '0x123456abcdef...fedcba654321'
        )
    FROM inserted_transaction
    RETURNING id
)

SELECT 'Transaction, status event, ledger lock, and outbox event created' AS result;

COMMIT;

-- Verify initiation state (everything derived from views)
SELECT * FROM account_balances;
SELECT * FROM current_transactions;
SELECT * FROM ledger_entries ORDER BY created_at;

-- -----------------------------------------------
-- STEP 3: Blockchain confirmation
-- No UPDATEs — only new event and ledger rows.
-- -----------------------------------------------

BEGIN;

-- Record confirmation as a new event (no UPDATE on transactions)
INSERT INTO transaction_events (transaction_id, status, tx_hash, block_number)
SELECT id, 'confirmed', '0x321...cba', 12345678
FROM transactions;

-- Settle the withdrawal via ledger entry:
-- debit balance (-25) and release lock (-25)
INSERT INTO ledger_entries (account_id, entry_type, amount, locked_delta, transaction_id)
SELECT
    t.account_id,
    'withdrawal_settle',
    -t.amount,
    -t.amount,
    t.id
FROM transactions t;

-- Write confirmation event to outbox
INSERT INTO outbox_events (id, aggregate_id, event_type, payload)
SELECT
    gen_random_uuid(),
    t.id::VARCHAR,
    'withdrawal.confirmed',
    jsonb_build_object(
        'transaction_id', t.id::VARCHAR,
        'tx_hash', '0x321...cba',
        'block_number', '12345678',
        'amount', t.amount::VARCHAR
    )
FROM transactions t;

COMMIT;

-- -----------------------------------------------
-- STEP 4: Verify final state
-- Everything derived from append-only tables.
-- -----------------------------------------------

SELECT
    ab.balance,
    ab.locked_balance,
    ab.balance - ab.locked_balance AS available,
    ct.status,
    ct.tx_hash,
    ct.block_number,
    ct.confirmed_at
FROM account_balances ab
JOIN current_transactions ct ON ct.account_id = ab.account_id;

SELECT * FROM ledger_entries ORDER BY created_at;
SELECT * FROM transaction_events ORDER BY created_at;
SELECT * FROM outbox_events ORDER BY created_at;
