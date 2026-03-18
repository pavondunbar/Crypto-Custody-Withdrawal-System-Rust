# Crypto Custody Withdrawal Service

<img width="1498" height="696" alt="Screenshot 2026-03-17 at 11 17 29 AM" src="https://github.com/user-attachments/assets/28aecd29-6e7c-4651-9324-4458b5fea85e" />

> ⚠️ **DEMO ONLY — NOT FOR PRODUCTION USE**
> This codebase is a simplified demonstration of withdrawal system architecture concepts.
> It is not hardened, audited, or suitable for handling real funds under any circumstances.
> See [Production Considerations](#production-considerations) for what would need to change.

---

## Overview

A Rust implementation of a crypto custody withdrawal service backed by PostgreSQL. It demonstrates how to safely move funds from a custodied account to an on-chain destination address using several battle-tested backend patterns.

The service handles the full withdrawal lifecycle:

```
User Request → Idempotency Check → Address Validation → Fund Lock
    → Policy Evaluation → Signing Queue → Blockchain → Confirmation
```

---

## Key Patterns Demonstrated

### Pessimistic Locking (`FOR UPDATE`)
Before any balance check or deduction, the service acquires a row-level lock on the account. This serialises concurrent withdrawal attempts for the same account and eliminates double-spend races without requiring application-level mutexes.

### Transactional Outbox
The fund lock, transaction record, and Kafka outbox event are written in a **single atomic database transaction**. If the message broker is down or the process crashes mid-flight, the event is already durable in Postgres. A separate outbox poller delivers it to Kafka once the broker recovers — no lost events, no dual-write inconsistency.

### Idempotency Keys
Every withdrawal request carries a caller-supplied `idempotency_key`. The service checks this before any writes, making it safe for clients to retry on network failures without risk of duplicate withdrawals.

### Hexagonal Architecture (Ports & Adapters)
The core service (`service.rs`) depends only on two traits:

- `PolicyEngine` — compliance and risk evaluation
- `SigningQueue` — HSM signing queue (modelled after SQS FIFO)

Both are swappable without touching the service logic. The included `StubPolicyEngine` and `StubSigningQueue` in `main.rs` approve everything and log to stdout, making the service runnable in a sandbox with no external dependencies.

### Typed Errors
Every failure mode is a distinct variant in `WithdrawalError` (thiserror), carrying structured context — available balance, requested amount, rejected asset — rather than stringly-typed messages.

### Structured Logging
`tracing` + `#[instrument]` attach `user_id`, `asset`, and `amount` as structured fields to every log line emitted inside a request, without manual threading.

---

## Project Structure

```
withdrawal/
├── Cargo.toml
├── README.md
├── schema.sql          # DDL — run this first in your sandbox DB
├── seed_test.sql       # Optional: seeds a test account + simulates full lifecycle
└── src/
    ├── main.rs         # Binary entrypoint + stub adapter implementations
    ├── service.rs      # Core withdrawal logic
    ├── models.rs       # DB row types, enums, request/response types
    ├── ports.rs        # PolicyEngine + SigningQueue trait definitions
    └── errors.rs       # Typed error enum
```

---

## Running in CoderPad

### 1. Set up the database schema

Open the **PostgreSQL panel** in CoderPad and run `schema.sql`:

```sql
CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE accounts (
  id             UUID PRIMARY KEY,
  user_id        UUID NOT NULL,
  asset          VARCHAR(10) NOT NULL,
  balance        DECIMAL(38,18) NOT NULL DEFAULT 0,
  locked_balance DECIMAL(38,18) NOT NULL DEFAULT 0,
  created_at     TIMESTAMP NOT NULL,
  updated_at     TIMESTAMP NOT NULL
);

CREATE TABLE transactions (
  id                  UUID PRIMARY KEY,
  account_id          UUID REFERENCES accounts(id),
  type                VARCHAR(20) NOT NULL,
  amount              DECIMAL(38,18) NOT NULL,
  status              VARCHAR(20) NOT NULL,
  destination_address VARCHAR(256),
  tx_hash             VARCHAR(256),
  block_number        BIGINT,
  idempotency_key     VARCHAR(256) UNIQUE,
  policy_check_result JSONB,
  created_at          TIMESTAMP NOT NULL,
  confirmed_at        TIMESTAMP
);

CREATE TABLE outbox_events (
  id           UUID PRIMARY KEY,
  aggregate_id VARCHAR(256) NOT NULL,
  event_type   VARCHAR(64) NOT NULL,
  payload      JSONB NOT NULL,
  created_at   TIMESTAMP NOT NULL DEFAULT NOW(),
  published_at TIMESTAMP
);

CREATE INDEX idx_transactions_status ON transactions(status, created_at);
CREATE INDEX idx_outbox_unpublished ON outbox_events(created_at) WHERE published_at IS NULL;
```

### 2. Seed a test account (optional)

Run `seed_test.sql` in the PostgreSQL panel to insert a test ETH account with a pre-set balance and walk through the full withdrawal lifecycle in raw SQL. Useful for verifying state before and after running the Rust service.

### 3. Run the Rust service

CoderPad automatically sets `DATABASE_URL` to its managed Postgres instance. Hit **Run** — the service will:

1. Connect to Postgres
2. Attempt a sample 25 ETH withdrawal
3. Log each step (idempotency check, fund lock, policy eval, signing queue)
4. Print the resulting transaction ID and status

To test a different scenario, edit the `WithdrawalRequest` in `main.rs`:

```rust
let req = WithdrawalRequest {
    user_id:             uuid::Uuid::new_v4(),
    asset:               "ETH".into(),
    amount:              Decimal::new(25, 0),
    destination_address: "bc1qxy2kgdygjrsqtzq2n0yrf".into(),
    idempotency_key:     uuid::Uuid::new_v4().to_string(),
};
```

---

## Running Locally

```bash
# Start a local Postgres instance
docker run --rm -e POSTGRES_PASSWORD=password -e POSTGRES_DB=custody -p 5432:5432 postgres:16

# Apply the schema
psql postgres://postgres:password@localhost/custody -f schema.sql

# Run the service
DATABASE_URL=postgres://postgres:password@localhost/custody cargo run
```

---

## Dependencies

| Crate | Purpose |
|---|---|
| `sqlx` | Async Postgres driver with runtime query binding |
| `tokio` | Async runtime |
| `uuid` | UUID v4 generation |
| `rust_decimal` | Precise decimal arithmetic for financial amounts |
| `serde` / `serde_json` | Serialization for outbox event payloads |
| `chrono` | Timestamp types |
| `thiserror` | Typed error derivation |
| `async-trait` | Async methods in trait definitions |
| `tracing` | Structured, levelled logging |

---

## Production Considerations

This demo omits a significant number of concerns that a real custody system would require:

- **No real policy engine** — the stub approves all transactions unconditionally
- **No real signing queue** — the stub discards messages without sending to an HSM
- **No confirmation tracker** — the blockchain listener that updates `tx_hash`, `block_number`, and `confirmed_at` is not implemented
- **No outbox poller** — events written to `outbox_events` are never published to Kafka
- **No per-asset address validation** — the address check is a non-empty string guard only
- **No authentication or authorisation** — there is no API layer or identity verification
- **No secrets management** — `DATABASE_URL` is passed as a plain environment variable
- **No migrations** — schema is applied manually; a production system would use `sqlx migrate` or similar
- **No observability** — no metrics, distributed tracing, or alerting
- **No rate limiting or velocity checks** — a user can submit unlimited withdrawal requests

---

## 📄 License

This project is provided as-is for educational and reference purposes. Please review the repository's license file before use.

---

*Built with ♥️ by [Pavon Dunbar](https://linktr.ee/pavondunbar)*
