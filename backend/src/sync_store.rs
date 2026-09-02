use mew_image_shared::{
    SyncEnvelope, merge_envelopes, now_rfc3339, strip_successful_task_payloads,
};
use sqlx::{Row, SqliteConnection, SqlitePool};

pub struct StoredSyncMerge {
    pub envelope: SyncEnvelope,
    pub updated_at: String,
}

pub async fn merge_snapshot_transactionally(
    db: &SqlitePool,
    user_id: &str,
    incoming: &SyncEnvelope,
) -> anyhow::Result<StoredSyncMerge> {
    let mut connection = db.acquire().await?;
    // IMMEDIATE 在读取旧快照前获得写保留锁，避免两台设备同时读到同一个旧版本后互相覆盖。
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;

    let result = merge_snapshot_on_connection(&mut connection, user_id, incoming).await;
    match result {
        Ok(merged) => {
            match sqlx::query("COMMIT").execute(&mut *connection).await {
                Ok(_) => Ok(merged),
                Err(error) => {
                    // 手写 BEGIN IMMEDIATE 没有 RAII 回滚保护，提交失败时必须主动清理连接状态。
                    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                    Err(error.into())
                }
            }
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn merge_snapshot_on_connection(
    connection: &mut SqliteConnection,
    user_id: &str,
    incoming: &SyncEnvelope,
) -> anyhow::Result<StoredSyncMerge> {
    let row = sqlx::query("SELECT payload FROM sync_snapshots WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(&mut *connection)
        .await?;
    let mut existing = row
        .map(|row| serde_json::from_str::<SyncEnvelope>(&row.get::<String, _>("payload")))
        .transpose()?
        .unwrap_or_default();
    strip_successful_task_payloads(&mut existing.tasks);

    let merged = merge_envelopes(&existing, incoming);
    let updated_at = now_rfc3339();
    sqlx::query(
        "INSERT INTO sync_snapshots (user_id, payload, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET
             payload = excluded.payload,
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(serde_json::to_string(&merged)?)
    .bind(&updated_at)
    .execute(&mut *connection)
    .await?;

    Ok(StoredSyncMerge {
        envelope: merged,
        updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mew_image_shared::{SyncEntityKind, SyncTombstone};
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    #[tokio::test]
    async fn concurrent_pushes_do_not_overwrite_each_other() {
        let database_url = format!(
            "sqlite:file:sync-{}?mode=memory&cache=shared",
            uuid::Uuid::new_v4()
        );
        let options = SqliteConnectOptions::from_str(&database_url)
            .unwrap()
            .create_if_missing(true);
        let db = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE sync_snapshots (
                user_id TEXT PRIMARY KEY,
                payload TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&db)
        .await
        .unwrap();

        let envelope = |entity_id: &str| SyncEnvelope {
            tombstones: vec![SyncTombstone {
                entity_kind: SyncEntityKind::Asset,
                entity_id: entity_id.to_string(),
                deleted_at: "2026-09-01T00:00:00Z".into(),
            }],
            updated_at: "2026-09-01T00:00:00Z".into(),
            ..SyncEnvelope::default()
        };
        let first = envelope("asset-a");
        let second = envelope("asset-b");
        let (first_result, second_result) = tokio::join!(
            merge_snapshot_transactionally(&db, "user-a", &first),
            merge_snapshot_transactionally(&db, "user-a", &second),
        );
        first_result.unwrap();
        second_result.unwrap();

        let payload = sqlx::query_scalar::<_, String>(
            "SELECT payload FROM sync_snapshots WHERE user_id = 'user-a'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        let stored: SyncEnvelope = serde_json::from_str(&payload).unwrap();
        let ids = stored
            .tombstones
            .iter()
            .map(|item| item.entity_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            ids,
            std::collections::BTreeSet::from(["asset-a", "asset-b"])
        );
    }
}
