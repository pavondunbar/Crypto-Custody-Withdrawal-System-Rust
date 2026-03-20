mod errors;
mod models;
mod outbox;
mod ports;
mod service;

use std::time::Duration;

use async_trait::async_trait;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::ClientConfig;
use rust_decimal::Decimal;

use errors::WithdrawalError;
use models::{
    OutboxEvent, PolicyDecision, PolicyVerdict, SigningMessage, Transaction,
};
use outbox::EventPublisher;
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

/// Publishes outbox events to Kafka topics.
/// Uses `event_type` as the topic name and `aggregate_id`
/// as the partition key to preserve per-transaction ordering.
struct KafkaEventPublisher {
    producer: FutureProducer,
}

impl KafkaEventPublisher {
    fn new(producer: FutureProducer) -> Self {
        Self { producer }
    }
}

#[async_trait]
impl EventPublisher for KafkaEventPublisher {
    async fn publish(
        &self,
        event: &OutboxEvent,
    ) -> Result<(), WithdrawalError> {
        let payload = event.payload.to_string();
        let record = FutureRecord::to(&event.event_type)
            .key(&event.aggregate_id)
            .payload(&payload);

        self.producer
            .send(record, Duration::from_secs(5))
            .await
            .map_err(|(err, _)| {
                WithdrawalError::OutboxPublish(format!(
                    "kafka produce failed for event {}: {err}",
                    event.id,
                ))
            })?;

        tracing::info!(
            event_id     = %event.id,
            event_type   = %event.event_type,
            aggregate_id = %event.aggregate_id,
            "event published to kafka"
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
        .unwrap_or_else(|_| "postgres://postgres:password@localhost/custodyrust".into());
    let kafka_broker = std::env::var("KAFKA_BROKER_URL")
        .unwrap_or_else(|_| "localhost:9092".into());

    let pool = sqlx::PgPool::connect(&database_url).await?;

    let kafka_producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", &kafka_broker)
        .set("message.timeout.ms", "5000")
        .create()
        .expect("failed to create kafka producer");

    // Spawn outbox processor — polls unpublished events and delivers
    // them to Kafka topics keyed by event_type.
    let outbox_pool = pool.clone();
    let outbox_handle = tokio::spawn(async move {
        let processor = outbox::OutboxProcessor::new(
            outbox_pool,
            Box::new(KafkaEventPublisher::new(kafka_producer)),
            10,
            Duration::from_secs(1),
        );
        processor.run().await;
    });

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

    // Give the outbox processor time to pick up the new event
    tokio::time::sleep(Duration::from_secs(2)).await;
    outbox_handle.abort();

    Ok(())
}
