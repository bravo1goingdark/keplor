//! Background task that periodically refreshes today's daily rollups.
//!
//! Runs on a configurable interval (default 60s). Also backfills yesterday
//! on startup in case the server was down at midnight.

use std::sync::Arc;
use std::time::Duration;

use keplor_store::Store;

/// Spawn a background task that re-rolls today's `daily_rollups` on a
/// fixed interval.
///
/// The returned handle can be used to abort the task on shutdown.
pub fn spawn_rollup_task(store: Arc<Store>, interval: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Backfill yesterday on startup (covers restarts across midnight).
        let yesterday = today_epoch() - 86400;
        if let Err(e) = rollup_blocking(&store, yesterday).await {
            tracing::warn!(day = yesterday, error = %e, "failed to backfill yesterday's rollup");
        }

        loop {
            let today = today_epoch();
            if let Err(e) = rollup_blocking(&store, today).await {
                tracing::warn!(day = today, error = %e, "rollup tick failed");
            }
            tokio::time::sleep(interval).await;
        }
    })
}

/// Run `store.rollup_day` on a blocking thread.
async fn rollup_blocking(
    store: &Arc<Store>,
    day_epoch: i64,
) -> Result<(), keplor_store::StoreError> {
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || store.rollup_day(day_epoch)).await.map_err(|e| {
        keplor_store::StoreError::Migration {
            version: 0,
            reason: format!("rollup task panicked: {e}"),
        }
    })?
}

/// Current UTC day boundary as epoch seconds.
fn today_epoch() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    now - (now % 86400)
}
