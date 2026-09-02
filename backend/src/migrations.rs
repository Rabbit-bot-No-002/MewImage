use sqlx::{Row, SqlitePool};
use std::collections::BTreeMap;

pub async fn run_data_integrity_migrations(db: &SqlitePool) -> anyhow::Result<()> {
    migrate_user_session_versions(db).await?;
    migrate_provider_templates(db).await?;
    create_integrity_indexes(db).await?;
    Ok(())
}

async fn migrate_user_session_versions(db: &SqlitePool) -> anyhow::Result<()> {
    let has_session_version = sqlx::query("PRAGMA table_info(users)")
        .fetch_all(db)
        .await?
        .iter()
        .any(|row| row.get::<String, _>("name") == "session_version");
    if !has_session_version {
        sqlx::query("ALTER TABLE users ADD COLUMN session_version INTEGER NOT NULL DEFAULT 0")
            .execute(db)
            .await?;
    }
    Ok(())
}

async fn create_integrity_indexes(db: &SqlitePool) -> anyhow::Result<()> {
    for statement in [
        "CREATE INDEX IF NOT EXISTS assets_user_id ON assets(user_id, id)",
        "CREATE INDEX IF NOT EXISTS assets_user_object_key ON assets(user_id, object_key)",
        "CREATE INDEX IF NOT EXISTS assets_object_key ON assets(object_key)",
        "CREATE INDEX IF NOT EXISTS upload_tokens_user_expiry ON upload_tokens(user_id, expires_at)",
        "CREATE INDEX IF NOT EXISTS upload_tokens_expiry_object_key ON upload_tokens(expires_at, object_key)",
        "CREATE INDEX IF NOT EXISTS upload_tokens_object_key_expiry ON upload_tokens(object_key, expires_at)",
    ] {
        sqlx::query(statement).execute(db).await?;
    }
    Ok(())
}

pub async fn migrate_provider_templates(db: &SqlitePool) -> anyhow::Result<()> {
    let columns = sqlx::query("PRAGMA table_info(provider_templates)")
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("name"),
                (row.get::<i64, _>("notnull") != 0, row.get::<i64, _>("pk")),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let already_scoped = columns.get("user_id") == Some(&(true, 1))
        && columns
            .get("id")
            .is_some_and(|(_, primary_key)| *primary_key == 2);
    if already_scoped {
        return Ok(());
    }

    let mut transaction = db.begin().await?;
    sqlx::query("DROP TABLE IF EXISTS provider_templates_user_scoped")
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        r#"CREATE TABLE provider_templates_user_scoped (
            user_id TEXT NOT NULL,
            id TEXT NOT NULL,
            payload TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (user_id, id)
        )"#,
    )
    .execute(&mut *transaction)
    .await?;
    // 旧表允许 user_id 为空；这类记录没有可靠归属，迁移时不能分配给任意账号。
    sqlx::query(
        "INSERT INTO provider_templates_user_scoped (user_id, id, payload, created_at, updated_at)
         SELECT user_id, id, payload, created_at, updated_at
         FROM provider_templates
         WHERE user_id IS NOT NULL AND TRIM(user_id) != ''",
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DROP TABLE provider_templates")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("ALTER TABLE provider_templates_user_scoped RENAME TO provider_templates")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn provider_template_ids_are_scoped_per_user_after_migration() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE provider_templates (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_templates VALUES ('same-id', 'user-a', '{}', 'now', 'now')",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_templates VALUES ('orphan-id', NULL, '{}', 'now', 'now')",
        )
        .execute(&db)
        .await
        .unwrap();

        migrate_provider_templates(&db).await.unwrap();
        // 重复执行必须保持幂等，避免每次启动都重建表。
        migrate_provider_templates(&db).await.unwrap();
        sqlx::query(
            "INSERT INTO provider_templates VALUES ('user-b', 'same-id', '{}', 'now', 'now')",
        )
        .execute(&db)
        .await
        .unwrap();

        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM provider_templates WHERE id = 'same-id'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(count, 2);

        let duplicate_for_same_user = sqlx::query(
            "INSERT INTO provider_templates VALUES ('user-a', 'same-id', '{}', 'now', 'now')",
        )
        .execute(&db)
        .await;
        assert!(duplicate_for_same_user.is_err());
        let orphan_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM provider_templates WHERE id = 'orphan-id'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(orphan_count, 0);
    }

    #[tokio::test]
    async fn session_version_migration_preserves_existing_users() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE users (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL,
                password_hash TEXT NOT NULL
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO users VALUES ('user-a', 'alice', 'hash')")
            .execute(&db)
            .await
            .unwrap();

        migrate_user_session_versions(&db).await.unwrap();
        let session_version =
            sqlx::query_scalar::<_, i64>("SELECT session_version FROM users WHERE id = 'user-a'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(session_version, 0);
    }
}
