use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use mew_image_shared::{AdminAuditEntry, AdminAuditResponse, new_id, now_rfc3339};
use serde::Deserialize;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, Transaction};
use tower_sessions::Session;

use crate::{AppError, AppState, require_admin};

const DEFAULT_PAGE_SIZE: usize = 20;

#[derive(Debug, Deserialize)]
struct AuditQuery {
    page: Option<usize>,
    limit: Option<usize>,
    q: Option<String>,
    action: Option<String>,
}

pub struct AuditRecord<'a> {
    pub operation_id: &'a str,
    pub actor_user_id: &'a str,
    pub actor_username: &'a str,
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: &'a str,
    pub target_name: &'a str,
    pub summary: &'a str,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/api/admin/audit", get(list_audit))
}

pub async fn init_db(db: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS admin_audit_logs (
            id TEXT PRIMARY KEY,
            operation_id TEXT NOT NULL,
            actor_user_id TEXT NOT NULL,
            actor_username TEXT NOT NULL,
            action TEXT NOT NULL,
            target_type TEXT NOT NULL,
            target_id TEXT NOT NULL,
            target_name TEXT NOT NULL,
            summary TEXT NOT NULL,
            created_at TEXT NOT NULL
        )"#,
    )
    .execute(db)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS admin_audit_created ON admin_audit_logs(created_at DESC)",
    )
    .execute(db)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS admin_audit_action ON admin_audit_logs(action, created_at DESC)")
        .execute(db)
        .await?;
    Ok(())
}

pub async fn record(
    transaction: &mut Transaction<'_, Sqlite>,
    entry: AuditRecord<'_>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO admin_audit_logs (id, operation_id, actor_user_id, actor_username, action, target_type, target_id, target_name, summary, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(new_id())
    .bind(entry.operation_id)
    .bind(entry.actor_user_id)
    .bind(entry.actor_username)
    .bind(entry.action)
    .bind(entry.target_type)
    .bind(entry.target_id)
    .bind(entry.target_name)
    .bind(entry.summary)
    .bind(now_rfc3339())
    .execute(&mut **transaction)
    .await
    .map_err(AppError::internal)?;
    Ok(())
}

async fn list_audit(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(query): Query<AuditQuery>,
) -> Result<Json<AdminAuditResponse>, AppError> {
    require_admin(&state, &session).await?;
    let page = query.page.unwrap_or(1).max(1);
    let limit = match query.limit.unwrap_or(DEFAULT_PAGE_SIZE) {
        20 | 50 | 100 => query.limit.unwrap_or(DEFAULT_PAGE_SIZE),
        _ => return Err(AppError::bad_request("审计分页大小仅支持 20、50 或 100。")),
    };
    let keyword = query.q.unwrap_or_default().trim().to_string();
    if keyword.chars().count() > 100 {
        return Err(AppError::bad_request("审计搜索内容不能超过 100 个字符。"));
    }
    let action = query.action.unwrap_or_default().trim().to_string();
    let mut count = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM admin_audit_logs WHERE 1=1");
    append_filters(&mut count, &keyword, &action);
    let total = count
        .build_query_scalar::<i64>()
        .fetch_one(&state.db)
        .await
        .map_err(AppError::internal)?
        .max(0) as usize;

    let mut rows = QueryBuilder::<Sqlite>::new(
        "SELECT id, operation_id, actor_user_id, actor_username, action, target_type, target_id, target_name, summary, created_at FROM admin_audit_logs WHERE 1=1",
    );
    append_filters(&mut rows, &keyword, &action);
    rows.push(" ORDER BY created_at DESC, id DESC LIMIT ")
        .push_bind(limit as i64)
        .push(" OFFSET ")
        .push_bind(page.saturating_sub(1).saturating_mul(limit) as i64);
    let entries = rows
        .build()
        .fetch_all(&state.db)
        .await
        .map_err(AppError::internal)?
        .into_iter()
        .map(|row| AdminAuditEntry {
            id: row.get("id"),
            operation_id: row.get("operation_id"),
            actor_user_id: row.get("actor_user_id"),
            actor_username: row.get("actor_username"),
            action: row.get("action"),
            target_type: row.get("target_type"),
            target_id: row.get("target_id"),
            target_name: row.get("target_name"),
            summary: row.get("summary"),
            created_at: row.get("created_at"),
        })
        .collect();
    Ok(Json(AdminAuditResponse {
        entries,
        total,
        page,
        limit,
    }))
}

fn append_filters(builder: &mut QueryBuilder<'_, Sqlite>, keyword: &str, action: &str) {
    if !keyword.is_empty() {
        let pattern = format!("%{}%", escape_like(keyword));
        builder
            .push(" AND (actor_username LIKE ")
            .push_bind(pattern.clone())
            .push(" ESCAPE '\\' OR target_name LIKE ")
            .push_bind(pattern)
            .push(" ESCAPE '\\')");
    }
    if !action.is_empty() && action != "all" {
        builder.push(" AND action = ").push_bind(action.to_string());
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_search_escapes_sql_like_wildcards() {
        assert_eq!(escape_like(r"a%_\b"), r"a\%\_\\b");
    }
}
