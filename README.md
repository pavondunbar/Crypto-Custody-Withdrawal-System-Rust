# Crypto Custody Withdrawal Service

<img width="1498" height="696" alt="Screenshot 2026-03-17 at 11 17 29 AM" src="https://github.com/user-attachments/assets/28aecd29-6e7c-4651-9324-4458b5fea85e" />

> **DEMO ONLY — NOT FOR PRODUCTION USE**
> This codebase is a simplified demonstration of withdrawal system architecture concepts.
> It is not hardened, audited, or suitable for handling real funds under any circumstances.
> See [Production Considerations](#production-considerations) for what would need to change.

---

## Overview

A Rust implementation of a crypto custody withdrawal service backed by PostgreSQL and Kafka. It demonstrates how to safely move funds from a custodied account to an on-chain destination address using several battle-tested backend patterns.

The service handles the full withdrawal lifecycle:

```
User Request → Idempotency Check → Address Validation → Fund Lock
    → Policy Evaluation → Signing Queue → Blockchain → Confirmation
```

---

## Key Patterns Demonstrated

### Append-Only Event-Sourced Architecture
No row in the database is ever updated or deleted. Balances are derived from an append-only `ledger_entries` table, and transaction state is derived from an append-only `transaction_events` table. PostgreSQL triggers enforce immutability — any `UPDATE` or `DELETE` raises an exception. Two derived views (`account_balances`, `current_transactions`) materialise the current state on read.

### Pessimistic Locking (`FOR UPDATE`)
Before any balance check or deduction, the service acquires a row-level lock on the account identity row. This serialises concurrent withdrawal attempts for the same account and eliminates double-spend races without requiring application-level mutexes.

### Transactional Outbox
The fund lock, ledger entry, transaction event, and Kafka outbox event are written in a **single atomic database transaction**. A separate `OutboxProcessor` polls unpublished events and delivers them to Kafka via `KafkaEventPublisher`. If the broker is down or the process crashes mid-flight, the event is already durable in Postgres — no lost events, no dual-write inconsistency.

### Idempotency Keys
Every withdrawal request carries a caller-supplied `idempotency_key`. The service checks this before any writes, making it safe for clients to retry on network failures without risk of duplicate withdrawals.

### Hexagonal Architecture (Ports & Adapters)
The core service (`service.rs`) depends only on traits:

- `PolicyEngine` — compliance and risk evaluation
- `SigningQueue` — HSM signing queue (modelled after SQS FIFO)
- `EventPublisher` — outbox event delivery (Kafka)

`PolicyEngine` and `SigningQueue` are swappable without touching the service logic. The included `StubPolicyEngine` and `StubSigningQueue` in `main.rs` approve everything and log to stdout, making the service runnable in a sandbox with no compliance or signing dependencies. `KafkaEventPublisher` implements real Kafka delivery using `rdkafka`.

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
    ├── main.rs         # Binary entrypoint + stub adapters + KafkaEventPublisher
    ├── service.rs      # Core withdrawal logic
    ├── models.rs       # DB row types, enums, request/response types
    ├── outbox.rs       # EventPublisher trait + OutboxProcessor poll loop
    ├── ports.rs        # PolicyEngine + SigningQueue trait definitions
    └── errors.rs       # Typed error enum
```

---

## Database Schema

The schema uses an append-only, event-sourced design. Run `schema.sql` to create all tables, triggers, views, and indexes.

### Tables

- **`accounts`** — Immutable identity records. Balances are derived from `ledger_entries`, never stored directly.
- **`transactions`** — Immutable facts recorded at withdrawal initiation. Current status is derived from `transaction_events`.
- **`transaction_events`** — Append-only event log for transaction lifecycle (`pending_policy` → `approved` → `signed` → `broadcast` → `confirmed`).
- **`ledger_entries`** — Append-only ledger. Every balance change is an immutable entry. Balance = `SUM(amount)`, locked = `SUM(locked_delta)`.
- **`outbox_events`** — Transactional outbox for reliable event delivery to Kafka.

### Immutability Enforcement

A `prevent_append_only_mutation()` trigger function is attached to `accounts`, `transactions`, `transaction_events`, and `ledger_entries`. Any `UPDATE` or `DELETE` raises an exception.

### Derived Views

- **`account_balances`** — Joins `accounts` with `ledger_entries` to derive current balance and locked balance.
- **`current_transactions`** — Joins `transactions` with the latest `transaction_events` row to derive current status, `tx_hash`, `block_number`, and `confirmed_at`.

---

## Running Locally

### Prerequisites

- Rust (latest stable)
- PostgreSQL 16+
- Apache Kafka (for outbox event delivery)
- CMake (required by `rdkafka` to build bundled librdkafka)

### 1. Start Postgres and Kafka

```bash
# Postgres
docker run --rm -e POSTGRES_PASSWORD=password -e POSTGRES_DB=custodyrust -p 5432:5432 postgres:16

# Kafka (KRaft mode, no Zookeeper)
docker run --rm --name kafka -p 9092:9092 apache/kafka:latest
```

### 2. Apply the schema and seed data

```bash
psql postgres://postgres:password@localhost/custodyrust -f schema.sql
psql postgres://postgres:password@localhost/custodyrust -f seed_test.sql
```

### 3. Run the service

```bash
DATABASE_URL=postgres://postgres:password@localhost/custodyrust \
KAFKA_BROKER_URL=localhost:9092 \
cargo run
```

The service will:

1. Connect to Postgres and Kafka
2. Spawn the outbox processor (polls every 1 second)
3. Attempt a sample 25 ETH withdrawal
4. Log each step (idempotency check, fund lock, policy eval, signing queue)
5. Publish the outbox event to the `withdrawal.pending_policy` Kafka topic
6. Print the resulting transaction ID and status

### 4. Verify Kafka events

```bash
# List topics (should include withdrawal.pending_policy)
docker exec kafka /opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server localhost:9092 --list

# Read messages
docker exec kafka /opt/kafka/bin/kafka-console-consumer.sh \
  --bootstrap-server localhost:9092 \
  --topic withdrawal.pending_policy --from-beginning
```

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `postgres://postgres:password@localhost/custodyrust` | PostgreSQL connection string |
| `KAFKA_BROKER_URL` | `localhost:9092` | Kafka bootstrap server address |

---

## Dependencies

| Crate | Purpose |
|---|---|
| `sqlx` | Async Postgres driver with runtime query binding |
| `tokio` | Async runtime |
| `rdkafka` | Kafka producer (bundled librdkafka via cmake-build) |
| `uuid` | UUID v4 generation |
| `rust_decimal` | Precise decimal arithmetic for financial amounts |
| `serde` / `serde_json` | Serialization for outbox event payloads |
| `chrono` | Timestamp types |
| `thiserror` | Typed error derivation |
| `anyhow` | Application-level error handling |
| `async-trait` | Async methods in trait definitions |
| `tracing` | Structured, levelled logging |

---

## Production Considerations

This demo omits a significant number of concerns that a real custody system would require:

- **No real policy engine** — the stub approves all transactions unconditionally
- **No real signing queue** — the stub discards messages without sending to an HSM
- **No confirmation tracker** — the blockchain listener that updates `tx_hash`, `block_number`, and `confirmed_at` is not implemented
- **No per-asset address validation** — the address check is a non-empty string guard only
- **No authentication or authorisation** — there is no API layer or identity verification
- **No secrets management** — `DATABASE_URL` and `KAFKA_BROKER_URL` are passed as plain environment variables
- **No migrations** — schema is applied manually; a production system would use `sqlx migrate` or similar
- **No observability** — no metrics, distributed tracing, or alerting
- **No rate limiting or velocity checks** — a user can submit unlimited withdrawal requests
- **No dead-letter handling** — outbox events that repeatedly fail to publish are retried indefinitely

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).

---

*Built with ♥️ by [Pavon Dunbar](https://linktr.ee/pavondunbar)*
