use std::time::Duration;

use sqlx::PgPool;
use tracing::{debug, error, info, warn};

use crate::errors::WithdrawalError;
use crate::models::OutboxEvent;

/// Delivers outbox events to an external message broker
/// (Kafka, SNS, etc.).
#[async_trait::async_trait]
pub trait EventPublisher: Send + Sync {
    async fn publish(
        &self,
        event: &OutboxEvent,
    ) -> Result<(), WithdrawalError>;
}

pub struct OutboxProcessor {
    db: PgPool,
    publisher: Box<dyn EventPublisher>,
    batch_size: i64,
    poll_interval: Duration,
}

impl OutboxProcessor {
    pub fn new(
        db: PgPool,
        publisher: Box<dyn EventPublisher>,
        batch_size: i64,
        poll_interval: Duration,
    ) -> Self {
        Self { db, publisher, batch_size, poll_interval }
    }

    /// Runs the poll loop until the task is cancelled.
    pub async fn run(&self) {
        info!(
            batch_size = self.batch_size,
            poll_interval_ms = self.poll_interval.as_millis() as u64,
            "outbox processor started"
        );
        loop {
            match self.poll_and_publish().await {
                Ok(0) => debug!("no unpublished outbox events"),
                Ok(n) => info!(published = n, "outbox batch published"),
                Err(e) => {
                    error!(error = %e, "outbox poll cycle failed");
                }
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Fetches unpublished events, publishes them in order, and
    /// marks each as published. Stops on first publish failure to
    /// preserve ordering. Returns the count of published events.
    pub async fn poll_and_publish(
        &self,
    ) -> Result<u64, WithdrawalError> {
        let mut tx = self.db.begin().await?;

        let events = sqlx::query_as::<_, OutboxEvent>(
            r#"
            SELECT id, aggregate_id, event_type,
                   payload, created_at, published_at
            FROM outbox_events
            WHERE published_at IS NULL
            ORDER BY created_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(self.batch_size)
        .fetch_all(&mut *tx)
        .await?;

        if events.is_empty() {
            tx.commit().await?;
            return Ok(0);
        }

        let mut published = 0u64;
        for event in &events {
            if let Err(e) = self.publisher.publish(event).await {
                warn!(
                    event_id = %event.id,
                    error = %e,
                    "publish failed — stopping batch"
                );
                break;
            }
            sqlx::query(
                "UPDATE outbox_events \
                 SET published_at = NOW() WHERE id = $1",
            )
            .bind(event.id)
            .execute(&mut *tx)
            .await?;
            published += 1;
        }

        tx.commit().await?;
        Ok(published)
    }
}
