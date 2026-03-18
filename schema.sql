CREATE TABLE accounts(
  id UUID PRIMARY KEY,
  user_id UUID NOT NULL,
  asset VARCHAR(10) NOT NULL,
  balance DECIMAL(38,18) NOT NULL DEFAULT 0,
  locked_balance DECIMAL(38,18) NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE transactions(
  id UUID PRIMARY KEY,
  account_id UUID REFERENCES accounts(id),
  type VARCHAR(20) NOT NULL,
  amount DECIMAL(38,18) NOT NULL,
  status VARCHAR(20) NOT NULL,
  destination_address VARCHAR(256),
  tx_hash VARCHAR(256),
  block_number BIGINT,
  idempotency_key VARCHAR(256) UNIQUE,
  policy_check_result JSONB,
  created_at TIMESTAMPTZ NOT NULL,
  confirmed_at TIMESTAMPTZ      -- NULL means in flight, NOT NULL means confirmed
);

CREATE TABLE outbox_events(
  id UUID PRIMARY KEY,
  aggregate_id VARCHAR(256) NOT NULL,
  event_type VARCHAR(64) NOT NULL,
  payload JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  published_at TIMESTAMPTZ      -- NULL means not yet delivered to Kafka
);

CREATE INDEX idx_transactions_status
ON transactions(status, created_at);

CREATE INDEX idx_outbox_unpublished
ON outbox_events(created_at)
WHERE published_at IS NULL;
