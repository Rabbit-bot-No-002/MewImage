use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File as StdFile,
    io::{Cursor, Read, Write},
    net::SocketAddr,
    path::{Component, Path as FsPath, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use futures_util::{StreamExt, stream};
use image::{ImageFormat, ImageReader};
use mew_image_shared::{
    GalleryAsset, GalleryAssetRole, GalleryExportFilter, GalleryExportPreview, GalleryExportScope,
    GalleryImportConflict, GalleryImportMode, GalleryImportResponse, GalleryLikeResponse,
    GalleryTagSummary, GalleryTemplate, GalleryTemplateListResponse, GalleryTemplateStatus,
    GalleryTemplateUpsertRequest, new_id, now_rfc3339,
};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower_cookies::{
    Cookie, Cookies,
    cookie::{SameSite, time::Duration as CookieDuration},
};
use tower_sessions::Session;
use tracing::warn;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::state::AppState;
use crate::{
    AppError, current_user, delete_object, detect_image_mime, enforce_auth_rate_limit,
    get_object_bytes, hash_auth_identifier, hex_sha256, object_exists, put_object, require_admin,
    resolve_client_ip,
};

const GALLERY_VISITOR_COOKIE: &str = "mew_gallery_visitor";
const MAX_PAGE_SIZE: u32 = 48;
const DEFAULT_PAGE_SIZE: u32 = 24;
const MAX_TITLE_CHARS: usize = 120;
const MAX_PROMPT_CHARS: usize = 20_000;
const MAX_DESCRIPTION_CHARS: usize = 4_000;
const MAX_MODEL_CHARS: usize = 200;
const MAX_TAGS: usize = 12;
const MAX_TAG_CHARS: usize = 32;
const MAX_PREVIEWS: usize = 6;
const MAX_REFERENCES: usize = 16;
const MAX_PREVIEW_EDGE: u32 = 2_048;
const MAX_REFERENCE_EDGE: u32 = 4_096;
const MAX_THUMBNAIL_EDGE: u32 = 480;
const MAX_THUMBNAIL_BYTES: u64 = 4 * 1024 * 1024;
const LIKE_RATE_LIMIT: u32 = 120;
const LIKE_RATE_WINDOW_SECONDS: u64 = 600;
pub fn routes(max_asset_bytes: usize, max_archive_bytes: usize) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/gallery/templates", get(list_templates))
        .route("/api/gallery/templates/{template_id}", get(get_template))
        .route(
            "/api/gallery/templates/{template_id}/like",
            post(like_template).delete(unlike_template),
        )
        .route("/api/gallery/tags", get(list_tags))
        .route("/api/admin/gallery/tags", get(list_admin_tags))
        .route("/api/gallery/assets/{asset_id}", get(get_gallery_asset))
        .route(
            "/api/gallery/assets/{asset_id}/thumbnail",
            get(get_gallery_asset_thumbnail),
        )
        .route("/api/admin/gallery/templates", post(create_template))
        .route(
            "/api/admin/gallery/templates/{template_id}",
            put(update_template).delete(delete_template),
        )
        .route(
            "/api/admin/gallery/assets",
            post(upload_gallery_asset).layer(DefaultBodyLimit::max(max_asset_bytes)),
        )
        .route(
            "/api/admin/gallery/assets/{asset_id}",
            delete(delete_gallery_asset),
        )
        .route(
            "/api/admin/gallery/export",
            get(export_gallery_archive).post(export_filtered_archive),
        )
        .route(
            "/api/admin/gallery/export-preview",
            post(preview_gallery_export),
        )
        .route(
            "/api/admin/gallery/import",
            post(import_gallery_archive).layer(DefaultBodyLimit::max(max_archive_bytes)),
        )
}

pub async fn init_db(db: &SqlitePool) -> anyhow::Result<()> {
    for statement in [
        r#"CREATE TABLE IF NOT EXISTS gallery_templates (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            prompt TEXT NOT NULL,
            description TEXT NOT NULL,
            generation_settings TEXT NOT NULL,
            recommended_provider_kind TEXT NOT NULL,
            recommended_model TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS gallery_tags (
            name TEXT PRIMARY KEY,
            created_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS gallery_template_tags (
            template_id TEXT NOT NULL,
            tag_name TEXT NOT NULL,
            position INTEGER NOT NULL,
            PRIMARY KEY (template_id, tag_name),
            FOREIGN KEY (template_id) REFERENCES gallery_templates(id) ON DELETE CASCADE,
            FOREIGN KEY (tag_name) REFERENCES gallery_tags(name) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS gallery_assets (
            id TEXT PRIMARY KEY,
            object_key TEXT NOT NULL UNIQUE,
            thumbnail_object_key TEXT,
            thumbnail_byte_len INTEGER NOT NULL DEFAULT 0,
            mime_type TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            byte_len INTEGER NOT NULL,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            created_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS gallery_template_assets (
            template_id TEXT NOT NULL,
            asset_id TEXT NOT NULL,
            role TEXT NOT NULL,
            position INTEGER NOT NULL,
            PRIMARY KEY (template_id, role, position),
            UNIQUE (template_id, asset_id, role),
            FOREIGN KEY (template_id) REFERENCES gallery_templates(id) ON DELETE CASCADE,
            FOREIGN KEY (asset_id) REFERENCES gallery_assets(id) ON DELETE RESTRICT
        )"#,
        r#"CREATE TABLE IF NOT EXISTS gallery_likes (
            template_id TEXT NOT NULL,
            actor_key TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (template_id, actor_key),
            FOREIGN KEY (template_id) REFERENCES gallery_templates(id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS gallery_staged_objects (
            object_key TEXT PRIMARY KEY,
            created_at TEXT NOT NULL
        )"#,
    ] {
        sqlx::query(statement).execute(db).await?;
    }
    let asset_columns = sqlx::query("PRAGMA table_info(gallery_assets)")
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<BTreeSet<_>>();
    if !asset_columns.contains("thumbnail_object_key") {
        sqlx::query("ALTER TABLE gallery_assets ADD COLUMN thumbnail_object_key TEXT")
            .execute(db)
            .await?;
    }
    if !asset_columns.contains("thumbnail_byte_len") {
        sqlx::query(
            "ALTER TABLE gallery_assets ADD COLUMN thumbnail_byte_len INTEGER NOT NULL DEFAULT 0",
        )
        .execute(db)
        .await?;
    }
    for index in [
        "CREATE INDEX IF NOT EXISTS gallery_templates_status_updated ON gallery_templates(status, updated_at DESC)",
        "CREATE INDEX IF NOT EXISTS gallery_template_tags_tag ON gallery_template_tags(tag_name, template_id)",
        "CREATE INDEX IF NOT EXISTS gallery_template_assets_asset ON gallery_template_assets(asset_id)",
        "CREATE INDEX IF NOT EXISTS gallery_likes_template ON gallery_likes(template_id)",
        "CREATE INDEX IF NOT EXISTS gallery_staged_objects_created ON gallery_staged_objects(created_at)",
    ] {
        sqlx::query(index).execute(db).await?;
    }
    Ok(())
}

#[derive(Debug, Deserialize, Default)]
struct TemplateListQuery {
    q: Option<String>,
    tags: Option<String>,
    sort: Option<String>,
    page: Option<u32>,
    page_size: Option<u32>,
    admin: Option<bool>,
}

async fn list_templates(
    State(state): State<Arc<AppState>>,
    session: Session,
    cookies: Cookies,
    Query(query): Query<TemplateListQuery>,
) -> Result<Json<GalleryTemplateListResponse>, AppError> {
    let is_admin = query.admin.unwrap_or(false) && viewer_is_admin(&state, &session).await?;
    let actor_key = viewer_actor_key(&state, &session, &cookies).await?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query
        .page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let tags = normalize_tags(query.tags.as_deref().unwrap_or("").split(','))?;
    let search = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let rows = fetch_template_list_rows(
        &state.db,
        TemplateListFilter {
            actor_key: &actor_key,
            is_admin,
            search,
            tags: &tags,
            popular: query.sort.as_deref() == Some("popular"),
            page,
            page_size,
        },
    )
    .await?;
    let total = rows
        .first()
        .map(|row| row.get::<i64, _>("total").max(0) as u64)
        .unwrap_or(0);
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        items.push(template_from_row(&state.db, row).await?);
    }
    Ok(Json(GalleryTemplateListResponse {
        items,
        total,
        page,
        page_size,
    }))
}

struct TemplateListFilter<'a> {
    actor_key: &'a str,
    is_admin: bool,
    search: Option<&'a str>,
    tags: &'a [String],
    popular: bool,
    page: u32,
    page_size: u32,
}

async fn fetch_template_list_rows(
    db: &SqlitePool,
    filter: TemplateListFilter<'_>,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, AppError> {
    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT gt.*, COUNT(*) OVER() AS total, \
         (SELECT COUNT(*) FROM gallery_likes gl WHERE gl.template_id = gt.id) AS like_count, \
         EXISTS(SELECT 1 FROM gallery_likes gl WHERE gl.template_id = gt.id AND gl.actor_key = ",
    );
    builder
        .push_bind(filter.actor_key)
        .push(") AS liked_by_viewer FROM gallery_templates gt WHERE ");
    builder.push(if filter.is_admin {
        "1 = 1"
    } else {
        "gt.status = 'published'"
    });
    if let Some(search) = filter.search {
        let pattern = format!("%{}%", escape_like(&search.to_lowercase()));
        builder
            .push(" AND (LOWER(gt.title) LIKE ")
            .push_bind(pattern.clone());
        builder
            .push(" ESCAPE '\\' OR LOWER(gt.prompt) LIKE ")
            .push_bind(pattern.clone());
        builder.push(" ESCAPE '\\' OR EXISTS (SELECT 1 FROM gallery_template_tags gst WHERE gst.template_id = gt.id AND LOWER(gst.tag_name) LIKE ");
        builder.push_bind(pattern).push(" ESCAPE '\\'))");
    }
    for tag in filter.tags {
        builder.push(" AND EXISTS (SELECT 1 FROM gallery_template_tags gtt WHERE gtt.template_id = gt.id AND gtt.tag_name = ");
        builder.push_bind(tag).push(")");
    }
    if filter.popular {
        builder.push(" ORDER BY like_count DESC, gt.updated_at DESC");
    } else {
        builder.push(" ORDER BY gt.updated_at DESC");
    }
    builder
        .push(" LIMIT ")
        .push_bind(i64::from(filter.page_size));
    builder
        .push(" OFFSET ")
        .push_bind(i64::from(filter.page.saturating_sub(1)) * i64::from(filter.page_size));

    builder
        .build()
        .fetch_all(db)
        .await
        .map_err(AppError::internal)
}

async fn get_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    cookies: Cookies,
    Path(template_id): Path<String>,
) -> Result<Json<GalleryTemplate>, AppError> {
    let is_admin = viewer_is_admin(&state, &session).await?;
    let actor_key = viewer_actor_key(&state, &session, &cookies).await?;
    let row = sqlx::query(
        "SELECT gt.*, (SELECT COUNT(*) FROM gallery_likes gl WHERE gl.template_id = gt.id) AS like_count,
         EXISTS(SELECT 1 FROM gallery_likes gl WHERE gl.template_id = gt.id AND gl.actor_key = ?) AS liked_by_viewer
         FROM gallery_templates gt WHERE gt.id = ?",
    )
    .bind(actor_key)
    .bind(&template_id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::internal)?
    .ok_or_else(|| AppError::not_found("模板不存在。"))?;
    if row.get::<String, _>("status") != "published" && !is_admin {
        return Err(AppError::not_found("模板不存在。"));
    }
    Ok(Json(template_from_row(&state.db, row).await?))
}

async fn list_tags(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<GalleryTagSummary>>, AppError> {
    Ok(Json(load_tag_summaries(&state.db, true).await?))
}

async fn list_admin_tags(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> Result<Json<Vec<GalleryTagSummary>>, AppError> {
    require_admin(&state, &session).await?;
    Ok(Json(load_tag_summaries(&state.db, false).await?))
}

async fn load_tag_summaries(
    db: &SqlitePool,
    published_only: bool,
) -> Result<Vec<GalleryTagSummary>, AppError> {
    let query = if published_only {
        "SELECT gtt.tag_name, COUNT(DISTINCT gtt.template_id) AS template_count
         FROM gallery_template_tags gtt JOIN gallery_templates gt ON gt.id = gtt.template_id
         WHERE gt.status = 'published'
         GROUP BY gtt.tag_name ORDER BY template_count DESC, gtt.tag_name ASC"
    } else {
        "SELECT gtt.tag_name, COUNT(DISTINCT gtt.template_id) AS template_count
         FROM gallery_template_tags gtt
         GROUP BY gtt.tag_name ORDER BY template_count DESC, gtt.tag_name ASC"
    };
    let rows = sqlx::query(query)
        .fetch_all(db)
        .await
        .map_err(AppError::internal)?;
    Ok(rows
        .into_iter()
        .map(|row| GalleryTagSummary {
            name: row.get("tag_name"),
            template_count: row.get::<i64, _>("template_count").max(0) as u64,
        })
        .collect())
}

async fn get_gallery_asset(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(asset_id): Path<String>,
) -> Result<Response, AppError> {
    get_gallery_asset_response(&state, &session, &asset_id, false).await
}

async fn get_gallery_asset_thumbnail(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(asset_id): Path<String>,
) -> Result<Response, AppError> {
    get_gallery_asset_response(&state, &session, &asset_id, true).await
}

async fn get_gallery_asset_response(
    state: &AppState,
    session: &Session,
    asset_id: &str,
    thumbnail: bool,
) -> Result<Response, AppError> {
    let is_admin = viewer_is_admin(state, session).await?;
    let row = sqlx::query(
        "SELECT ga.object_key, ga.thumbnail_object_key, ga.thumbnail_byte_len,
         ga.mime_type, ga.byte_len,
         EXISTS(SELECT 1 FROM gallery_template_assets gta JOIN gallery_templates gt ON gt.id = gta.template_id
                WHERE gta.asset_id = ga.id AND gt.status = 'published') AS is_public
         FROM gallery_assets ga WHERE ga.id = ?",
    )
    .bind(asset_id).fetch_optional(&state.db).await.map_err(AppError::internal)?
    .ok_or_else(|| AppError::not_found("模板图片不存在。"))?;
    if row.get::<i64, _>("is_public") == 0 && !is_admin {
        return Err(AppError::not_found("模板图片不存在。"));
    }
    let original_object_key = row.get::<String, _>("object_key");
    let thumbnail_object_key = row.get::<Option<String>, _>("thumbnail_object_key");
    let thumbnail_byte_len = row.get::<i64, _>("thumbnail_byte_len").max(0) as u64;
    let (mut object_key, mut byte_len) = if thumbnail && thumbnail_byte_len > 0 {
        thumbnail_object_key
            .map(|object_key| (object_key, thumbnail_byte_len))
            .unwrap_or_else(|| {
                (
                    original_object_key.clone(),
                    row.get::<i64, _>("byte_len").max(0) as u64,
                )
            })
    } else {
        (
            original_object_key.clone(),
            row.get::<i64, _>("byte_len").max(0) as u64,
        )
    };
    if !object_exists(state, &object_key).await? {
        if thumbnail && object_key != original_object_key {
            object_key = original_object_key;
            byte_len = row.get::<i64, _>("byte_len").max(0) as u64;
        } else {
            return Err(AppError::not_found("模板图片原文件不存在。"));
        }
    }
    let bytes = get_object_bytes(state, &object_key, byte_len.max(1)).await?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/webp"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok((StatusCode::OK, headers, bytes).into_response())
}

async fn create_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(payload): Json<GalleryTemplateUpsertRequest>,
) -> Result<Json<GalleryTemplate>, AppError> {
    require_admin(&state, &session).await?;
    let _gallery_guard = state.gallery_write_lock.lock().await;
    let template_id = payload.id.clone().unwrap_or_else(new_id);
    if sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM gallery_templates WHERE id = ?")
        .bind(&template_id)
        .fetch_one(&state.db)
        .await
        .map_err(AppError::internal)?
        > 0
    {
        return Err(AppError::bad_request("模板 ID 已存在。"));
    }
    upsert_template(&state, &template_id, payload, true).await
}

async fn update_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(template_id): Path<String>,
    Json(payload): Json<GalleryTemplateUpsertRequest>,
) -> Result<Json<GalleryTemplate>, AppError> {
    require_admin(&state, &session).await?;
    let _gallery_guard = state.gallery_write_lock.lock().await;
    upsert_template(&state, &template_id, payload, false).await
}

async fn upsert_template(
    state: &AppState,
    template_id: &str,
    payload: GalleryTemplateUpsertRequest,
    create: bool,
) -> Result<Json<GalleryTemplate>, AppError> {
    if uuid::Uuid::parse_str(template_id).is_err() {
        return Err(AppError::bad_request("模板 ID 格式无效。"));
    }
    validate_template_input(&payload)?;
    validate_template_asset_ids(state, &payload).await?;
    let previous_asset_ids =
        sqlx::query("SELECT asset_id FROM gallery_template_assets WHERE template_id = ?")
            .bind(template_id)
            .fetch_all(&state.db)
            .await
            .map_err(AppError::internal)?
            .into_iter()
            .map(|row| row.get::<String, _>("asset_id"))
            .collect::<Vec<_>>();
    let tags = normalize_tags(payload.tags.iter().map(String::as_str))?;
    let now = now_rfc3339();
    let status = status_value(payload.status);
    let provider =
        serde_json::to_string(&payload.recommended_provider_kind).map_err(AppError::internal)?;
    let settings =
        serde_json::to_string(&payload.generation_settings).map_err(AppError::internal)?;
    let mut transaction = state.db.begin().await.map_err(AppError::internal)?;
    if !create {
        let exists =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM gallery_templates WHERE id = ?")
                .bind(template_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(AppError::internal)?;
        if exists == 0 {
            return Err(AppError::not_found("模板不存在。"));
        }
    }
    sqlx::query(
        "INSERT INTO gallery_templates
         (id, title, prompt, description, generation_settings, recommended_provider_kind, recommended_model, status, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, prompt=excluded.prompt,
         description=excluded.description, generation_settings=excluded.generation_settings,
         recommended_provider_kind=excluded.recommended_provider_kind, recommended_model=excluded.recommended_model,
         status=excluded.status, updated_at=excluded.updated_at",
    )
    .bind(template_id).bind(payload.title.trim()).bind(payload.prompt.trim())
    .bind(payload.description.trim()).bind(settings).bind(provider)
    .bind(payload.recommended_model.trim()).bind(status).bind(&now).bind(&now)
    .execute(&mut *transaction).await.map_err(AppError::internal)?;
    sqlx::query("DELETE FROM gallery_template_tags WHERE template_id = ?")
        .bind(template_id)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
    for (position, tag) in tags.iter().enumerate() {
        sqlx::query("INSERT OR IGNORE INTO gallery_tags (name, created_at) VALUES (?, ?)")
            .bind(tag)
            .bind(&now)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        sqlx::query(
            "INSERT INTO gallery_template_tags (template_id, tag_name, position) VALUES (?, ?, ?)",
        )
        .bind(template_id)
        .bind(tag)
        .bind(position as i64)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
    }
    sqlx::query("DELETE FROM gallery_template_assets WHERE template_id = ?")
        .bind(template_id)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
    insert_asset_relations(
        &mut transaction,
        template_id,
        &payload.preview_asset_ids,
        "preview",
    )
    .await?;
    insert_asset_relations(
        &mut transaction,
        template_id,
        &payload.reference_asset_ids,
        "reference",
    )
    .await?;
    transaction.commit().await.map_err(AppError::internal)?;
    if let Err(error) = cleanup_unused_tags(&state.db).await {
        warn!(
            "gallery tag cleanup after template update failed: {}",
            error.message
        );
    }
    if let Err(error) = cleanup_unreferenced_assets(state, &previous_asset_ids).await {
        warn!(
            "gallery asset cleanup after template update failed: {}",
            error.message
        );
    }
    get_template_for_admin(state, template_id).await.map(Json)
}

async fn delete_template(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(template_id): Path<String>,
) -> Result<StatusCode, AppError> {
    require_admin(&state, &session).await?;
    let _gallery_guard = state.gallery_write_lock.lock().await;
    let asset_ids =
        sqlx::query("SELECT asset_id FROM gallery_template_assets WHERE template_id = ?")
            .bind(&template_id)
            .fetch_all(&state.db)
            .await
            .map_err(AppError::internal)?
            .into_iter()
            .map(|row| row.get::<String, _>("asset_id"))
            .collect::<Vec<_>>();
    let result = sqlx::query("DELETE FROM gallery_templates WHERE id = ?")
        .bind(&template_id)
        .execute(&state.db)
        .await
        .map_err(AppError::internal)?;
    if result.rows_affected() == 0 {
        return Err(AppError::not_found("模板不存在。"));
    }
    if let Err(error) = cleanup_unused_tags(&state.db).await {
        warn!(
            "gallery tag cleanup after template deletion failed: {}",
            error.message
        );
    }
    if let Err(error) = cleanup_unreferenced_assets(&state, &asset_ids).await {
        warn!(
            "gallery asset cleanup after template deletion failed: {}",
            error.message
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn upload_gallery_asset(
    State(state): State<Arc<AppState>>,
    session: Session,
    request: Request,
) -> Result<Json<GalleryAsset>, AppError> {
    require_admin(&state, &session).await?;
    let _gallery_guard = state.gallery_write_lock.lock().await;
    cleanup_expired_orphan_assets(&state).await?;
    let declared_len = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if declared_len.is_some_and(|value| value > state.config.max_upload_bytes) {
        return Err(AppError::bad_request("模板图片超过单文件大小限制。"));
    }
    let bytes = to_bytes(request.into_body(), state.config.max_upload_bytes as usize)
        .await
        .map_err(|error| AppError::bad_request(format!("模板图片读取失败：{error}")))?;
    if bytes.is_empty() || detect_image_mime(&bytes) != Some("image/webp") {
        return Err(AppError::bad_request(
            "模板图片必须是浏览器处理后的静态 WebP。",
        ));
    }
    let (width, height) = webp_dimensions(&bytes)?;
    if width.max(height) > MAX_REFERENCE_EDGE {
        return Err(AppError::bad_request("模板图片最长边不能超过 4096px。"));
    }
    let id = new_id();
    let sha256 = hex_sha256(&bytes);
    let byte_len = bytes.len() as u64;
    let object_key = format!("gallery/assets/{id}.webp");
    let thumbnail = create_gallery_thumbnail(&bytes)?;
    let thumbnail_object_key = format!("gallery/thumbnails/{id}.webp");
    let thumbnail_byte_len = thumbnail.len() as u64;
    ensure_gallery_quota(&state, byte_len.saturating_add(thumbnail_byte_len), 2).await?;
    let staged_keys = vec![object_key.clone(), thumbnail_object_key.clone()];
    record_staged_objects(&state.db, &staged_keys).await?;
    if let Err(error) = put_object(&state, &object_key, "image/webp", bytes).await {
        cleanup_staged_objects(&state, &staged_keys).await;
        return Err(error);
    }
    if let Err(error) = put_object(&state, &thumbnail_object_key, "image/webp", thumbnail).await {
        cleanup_staged_objects(&state, &staged_keys).await;
        return Err(error);
    }
    let created_at = now_rfc3339();
    let mut transaction = match state.db.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            cleanup_staged_objects(&state, &staged_keys).await;
            return Err(AppError::internal(error));
        }
    };
    let insert_result = sqlx::query(
        "INSERT INTO gallery_assets
         (id, object_key, thumbnail_object_key, thumbnail_byte_len, mime_type, sha256, byte_len, width, height, created_at)
         VALUES (?, ?, ?, ?, 'image/webp', ?, ?, ?, ?, ?)",
    ).bind(&id).bind(&object_key).bind(&thumbnail_object_key).bind(thumbnail_byte_len as i64)
        .bind(&sha256).bind(byte_len as i64)
        .bind(i64::from(width)).bind(i64::from(height)).bind(&created_at)
        .execute(&mut *transaction).await;
    if let Err(error) = insert_result {
        drop(transaction);
        cleanup_staged_objects(&state, &staged_keys).await;
        return Err(AppError::internal(error));
    }
    for staged_key in &staged_keys {
        if let Err(error) = sqlx::query("DELETE FROM gallery_staged_objects WHERE object_key = ?")
            .bind(staged_key)
            .execute(&mut *transaction)
            .await
        {
            drop(transaction);
            cleanup_staged_objects(&state, &staged_keys).await;
            return Err(AppError::internal(error));
        }
    }
    if let Err(error) = transaction.commit().await {
        cleanup_staged_objects(&state, &staged_keys).await;
        return Err(AppError::internal(error));
    }
    Ok(Json(GalleryAsset {
        id,
        role: GalleryAssetRole::Preview,
        sha256,
        mime_type: "image/webp".into(),
        byte_len,
        width,
        height,
        created_at,
    }))
}

async fn delete_gallery_asset(
    State(state): State<Arc<AppState>>,
    session: Session,
    Path(asset_id): Path<String>,
) -> Result<StatusCode, AppError> {
    require_admin(&state, &session).await?;
    let _gallery_guard = state.gallery_write_lock.lock().await;
    let row = sqlx::query(
        "SELECT ga.object_key, ga.thumbnail_object_key,
         EXISTS(SELECT 1 FROM gallery_template_assets gta WHERE gta.asset_id = ga.id) AS in_use
         FROM gallery_assets ga WHERE ga.id = ?",
    )
    .bind(&asset_id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::internal)?
    .ok_or_else(|| AppError::not_found("模板图片不存在。"))?;
    if row.get::<i64, _>("in_use") != 0 {
        return Err(AppError::bad_request("模板图片仍被模板使用，不能删除。"));
    }
    let object_key = row.get::<String, _>("object_key");
    let thumbnail_object_key = row.get::<Option<String>, _>("thumbnail_object_key");
    if let Some(thumbnail_key) = thumbnail_object_key.as_deref() {
        delete_object(&state, thumbnail_key).await?;
    }
    delete_object(&state, &object_key).await?;
    sqlx::query("DELETE FROM gallery_assets WHERE id = ?")
        .bind(asset_id)
        .execute(&state.db)
        .await
        .map_err(AppError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn like_template(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    session: Session,
    cookies: Cookies,
    Path(template_id): Path<String>,
) -> Result<Json<GalleryLikeResponse>, AppError> {
    change_like(
        &state,
        peer,
        &headers,
        &session,
        &cookies,
        &template_id,
        true,
    )
    .await
}

async fn unlike_template(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    session: Session,
    cookies: Cookies,
    Path(template_id): Path<String>,
) -> Result<Json<GalleryLikeResponse>, AppError> {
    change_like(
        &state,
        peer,
        &headers,
        &session,
        &cookies,
        &template_id,
        false,
    )
    .await
}

async fn change_like(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
    session: &Session,
    cookies: &Cookies,
    template_id: &str,
    liked: bool,
) -> Result<Json<GalleryLikeResponse>, AppError> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM gallery_templates WHERE id = ? AND status = 'published'",
    )
    .bind(template_id)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::internal)?;
    if exists == 0 {
        return Err(AppError::not_found("模板不存在。"));
    }
    let ip = resolve_client_ip(&state.config, headers, peer);
    let ip_hash = hash_auth_identifier(
        &state.config.auth_secret,
        "gallery_like_ip",
        &ip.to_string(),
    );
    enforce_auth_rate_limit(
        &state.db,
        "gallery_like_ip",
        &ip_hash,
        LIKE_RATE_LIMIT,
        LIKE_RATE_WINDOW_SECONDS,
        "点赞操作过于频繁，请稍后再试。",
    )
    .await?;
    let actor_key = viewer_actor_key(state, session, cookies).await?;
    if liked {
        sqlx::query("INSERT OR IGNORE INTO gallery_likes (template_id, actor_key, created_at) VALUES (?, ?, ?)")
            .bind(template_id).bind(&actor_key).bind(now_rfc3339())
            .execute(&state.db).await.map_err(AppError::internal)?;
    } else {
        sqlx::query("DELETE FROM gallery_likes WHERE template_id = ? AND actor_key = ?")
            .bind(template_id)
            .bind(&actor_key)
            .execute(&state.db)
            .await
            .map_err(AppError::internal)?;
    }
    let like_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM gallery_likes WHERE template_id = ?")
            .bind(template_id)
            .fetch_one(&state.db)
            .await
            .map_err(AppError::internal)?
            .max(0) as u64;
    Ok(Json(GalleryLikeResponse {
        template_id: template_id.into(),
        like_count,
        liked,
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GalleryArchiveManifest {
    archive_kind: String,
    schema_version: u32,
    exported_at: String,
    templates: Vec<GalleryTemplate>,
    assets: Vec<GalleryArchiveAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GalleryArchiveAsset {
    id: String,
    sha256: String,
    mime_type: String,
    byte_len: u64,
    width: u32,
    height: u32,
    created_at: String,
    path: String,
}

#[derive(Debug, Deserialize, Default)]
struct ImportModeQuery {
    mode: Option<GalleryImportMode>,
    conflict: Option<GalleryImportConflict>,
}

struct TemporaryPath(PathBuf);

impl Drop for TemporaryPath {
    fn drop(&mut self) {
        if self.0.is_dir() {
            let _ = std::fs::remove_dir_all(&self.0);
        } else {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

struct TemporaryDownload {
    file: tokio::fs::File,
    _path: TemporaryPath,
}

async fn export_gallery_archive(
    State(state): State<Arc<AppState>>,
    session: Session,
) -> Result<Response, AppError> {
    require_admin(&state, &session).await?;
    export_gallery_selection(
        &state,
        &GalleryExportFilter {
            scope: GalleryExportScope::Backup,
            ..Default::default()
        },
    )
    .await
}

async fn export_filtered_archive(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(filter): Json<GalleryExportFilter>,
) -> Result<Response, AppError> {
    require_admin(&state, &session).await?;
    export_gallery_selection(&state, &normalize_export_filter(filter)?).await
}

async fn preview_gallery_export(
    State(state): State<Arc<AppState>>,
    session: Session,
    Json(filter): Json<GalleryExportFilter>,
) -> Result<Json<GalleryExportPreview>, AppError> {
    require_admin(&state, &session).await?;
    let filter = normalize_export_filter(filter)?;
    let _guard = state.gallery_write_lock.lock().await;
    let (templates, assets) = load_export_selection(&state.db, &filter).await?;
    Ok(Json(GalleryExportPreview {
        template_count: templates.len(),
        asset_count: assets.len(),
        asset_byte_len: assets
            .iter()
            .map(|row| row.get::<i64, _>("byte_len").max(0) as u64)
            .sum(),
    }))
}

fn normalize_export_filter(
    mut filter: GalleryExportFilter,
) -> Result<GalleryExportFilter, AppError> {
    let mut statuses = Vec::with_capacity(3);
    for status in filter.statuses.drain(..) {
        if !statuses.contains(&status) {
            statuses.push(status);
        }
    }
    filter.statuses = statuses;
    for values in [&mut filter.categories, &mut filter.tags] {
        for value in values.iter_mut() {
            *value = value.trim().to_lowercase();
            if value.is_empty() || value.chars().count() > MAX_TAG_CHARS {
                return Err(AppError::bad_request("分类和标签必须为 1–32 个字符。"));
            }
        }
        values.sort();
        values.dedup();
    }
    if filter.categories.len() + filter.tags.len() + usize::from(filter.uncategorized) > 256 {
        return Err(AppError::bad_request("最多选择 256 个分类或标签。"));
    }
    if filter.scope == GalleryExportScope::Share
        && (filter.statuses.is_empty()
            || (filter.categories.is_empty() && filter.tags.is_empty() && !filter.uncategorized))
    {
        return Err(AppError::bad_request(
            "请选择至少一个分类或标签，以及一种模板状态。",
        ));
    }
    Ok(filter)
}

async fn load_export_selection(
    db: &SqlitePool,
    filter: &GalleryExportFilter,
) -> Result<(Vec<GalleryTemplate>, Vec<sqlx::sqlite::SqliteRow>), AppError> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT gt.*, 0 AS like_count, 0 AS liked_by_viewer FROM gallery_templates gt WHERE ",
    );
    if filter.scope == GalleryExportScope::Backup {
        query.push("1=1");
    } else {
        query.push("gt.status IN (");
        let mut statuses = query.separated(",");
        for status in &filter.statuses {
            statuses.push_bind(status_value(*status));
        }
        statuses.push_unseparated(") AND (");
        query.push("0=1");
        // 与前端一致：仅第一条斜杠两侧均非空时识别为分类，避免前缀误匹配。
        const CATEGORY: &str = "CASE WHEN instr(gtt.tag_name, '/') > 0 AND trim(substr(gtt.tag_name,1,instr(gtt.tag_name,'/')-1)) != '' AND trim(substr(gtt.tag_name,instr(gtt.tag_name,'/')+1)) != '' THEN trim(substr(gtt.tag_name,1,instr(gtt.tag_name,'/')-1)) ELSE '未分类' END";
        for category in &filter.categories {
            query.push(" OR EXISTS(SELECT 1 FROM gallery_template_tags gtt WHERE gtt.template_id=gt.id AND (").push(CATEGORY).push(")=").push_bind(category).push(")");
        }
        for tag in &filter.tags {
            query.push(" OR EXISTS(SELECT 1 FROM gallery_template_tags gtt WHERE gtt.template_id=gt.id AND gtt.tag_name=").push_bind(tag).push(")");
        }
        if filter.uncategorized {
            query.push(" OR NOT EXISTS(SELECT 1 FROM gallery_template_tags gtt WHERE gtt.template_id=gt.id) OR EXISTS(SELECT 1 FROM gallery_template_tags gtt WHERE gtt.template_id=gt.id AND (").push(CATEGORY).push(")='未分类')");
        }
        query.push(")");
    }
    query.push(" ORDER BY gt.created_at, gt.id");
    let rows = query
        .build()
        .fetch_all(db)
        .await
        .map_err(AppError::internal)?;
    let mut templates = Vec::with_capacity(rows.len());
    let mut ids = BTreeSet::new();
    for row in rows {
        let template = template_from_row(db, row).await?;
        ids.extend(
            template
                .preview_assets
                .iter()
                .chain(&template.reference_assets)
                .map(|asset| asset.id.clone()),
        );
        templates.push(template);
    }
    let rows = sqlx::query("SELECT * FROM gallery_assets ORDER BY id")
        .fetch_all(db)
        .await
        .map_err(AppError::internal)?;
    let assets = rows
        .into_iter()
        .filter(|row| ids.contains(&row.get::<String, _>("id")))
        .collect();
    Ok((templates, assets))
}

async fn export_gallery_selection(
    state: &AppState,
    filter: &GalleryExportFilter,
) -> Result<Response, AppError> {
    let _gallery_guard = state.gallery_write_lock.lock().await;
    let (templates, asset_rows) = load_export_selection(&state.db, filter).await?;
    if filter.scope == GalleryExportScope::Share && templates.is_empty() {
        return Err(AppError::bad_request("没有匹配的模板，请刷新导出范围。"));
    }
    let export_dir = unique_temp_path("mew-gallery-export", true).await?;
    let export_guard = TemporaryPath(export_dir.clone());
    let staged_assets_dir = export_dir.join("assets");
    tokio::fs::create_dir_all(&staged_assets_dir)
        .await
        .map_err(AppError::internal)?;
    let mut assets = Vec::with_capacity(asset_rows.len());
    for row in asset_rows {
        let id = row.get::<String, _>("id");
        let object_key = row.get::<String, _>("object_key");
        let byte_len = row.get::<i64, _>("byte_len").max(0) as u64;
        let bytes = get_object_bytes(state, &object_key, byte_len.max(1)).await?;
        let sha256 = row.get::<String, _>("sha256");
        if bytes.len() as u64 != byte_len || hex_sha256(&bytes) != sha256 {
            return Err(AppError::bad_request(format!(
                "模板图片 `{id}` 的原文件缺失或哈希不一致，已终止导出。"
            )));
        }
        let file_name = format!("{id}.webp");
        tokio::fs::write(staged_assets_dir.join(&file_name), bytes)
            .await
            .map_err(AppError::internal)?;
        assets.push(GalleryArchiveAsset {
            id,
            sha256,
            mime_type: row.get("mime_type"),
            byte_len,
            width: row.get::<i64, _>("width").max(0) as u32,
            height: row.get::<i64, _>("height").max(0) as u32,
            created_at: row.get("created_at"),
            path: format!("assets/{file_name}"),
        });
    }
    let manifest = GalleryArchiveManifest {
        archive_kind: "gallery_templates".into(),
        schema_version: 1,
        exported_at: now_rfc3339(),
        templates,
        assets,
    };
    let zip_path = export_dir.join("mew-gallery.zip");
    let zip_path_for_build = zip_path.clone();
    let assets_dir_for_build = staged_assets_dir.clone();
    tokio::task::spawn_blocking(move || {
        write_gallery_zip(&zip_path_for_build, &assets_dir_for_build, &manifest)
    })
    .await
    .map_err(AppError::internal)?
    .map_err(AppError::internal_message)?;
    let file = tokio::fs::File::open(&zip_path)
        .await
        .map_err(AppError::internal)?;
    let purpose = if filter.scope == GalleryExportScope::Share {
        "share"
    } else {
        "backup"
    };
    let file_name = format!(
        "mew-gallery-{purpose}-{}.zip",
        chrono::Utc::now().format("%Y%m%d")
    );
    let stream = stream::try_unfold(
        TemporaryDownload {
            file,
            _path: export_guard,
        },
        |mut download| async move {
            let mut chunk = vec![0u8; 64 * 1024];
            let read = download.file.read(&mut chunk).await?;
            if read == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            chunk.truncate(read);
            Ok(Some((Bytes::from(chunk), download)))
        },
    );
    let mut response = Body::from_stream(stream).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{file_name}\""))
            .map_err(AppError::internal)?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn import_gallery_archive(
    State(state): State<Arc<AppState>>,
    session: Session,
    Query(query): Query<ImportModeQuery>,
    request: Request,
) -> Result<Json<GalleryImportResponse>, AppError> {
    require_admin(&state, &session).await?;
    let _gallery_guard = state.gallery_write_lock.lock().await;
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > gallery_archive_byte_limit(&state))
    {
        return Err(AppError::bad_request("模板包超过当前广场资源配额。"));
    }
    let mode = query.mode.unwrap_or_default();
    let import_dir = unique_temp_path("mew-gallery-import", true).await?;
    let import_guard = TemporaryPath(import_dir.clone());
    let zip_path = import_dir.join("upload.zip");
    let mut output = tokio::fs::File::create(&zip_path)
        .await
        .map_err(AppError::internal)?;
    let mut total = 0usize;
    let mut body = request.into_body().into_data_stream();
    while let Some(chunk) = body.next().await {
        let chunk =
            chunk.map_err(|error| AppError::bad_request(format!("模板包读取失败：{error}")))?;
        total = total
            .checked_add(chunk.len())
            .ok_or_else(|| AppError::bad_request("模板包大小溢出。"))?;
        if total > gallery_archive_byte_limit(&state) {
            return Err(AppError::bad_request("模板包超过当前广场资源配额。"));
        }
        output.write_all(&chunk).await.map_err(AppError::internal)?;
    }
    output.flush().await.map_err(AppError::internal)?;
    drop(output);
    if total == 0 {
        return Err(AppError::bad_request("模板包为空。"));
    }
    let extracted_dir = import_dir.join("extracted");
    let zip_for_extract = zip_path.clone();
    let extracted_for_task = extracted_dir.clone();
    let limits = (
        state.config.gallery_asset_quota_bytes,
        state.config.gallery_asset_quota_count,
    );
    let mut manifest = tokio::task::spawn_blocking(move || {
        extract_and_validate_gallery_zip(&zip_for_extract, &extracted_for_task, limits)
    })
    .await
    .map_err(AppError::internal)?
    .map_err(AppError::bad_request)?;
    validate_archive_manifest(&manifest)?;
    // 即使选择跳过同 ID 模板，也先校验原包的所有图片，不能借冲突策略绕过校验。
    for asset in &manifest.assets {
        if asset.byte_len > state.config.max_upload_bytes {
            return Err(AppError::bad_request("模板图片超过本站单文件限制。"));
        }
        let file_len = tokio::fs::metadata(extracted_dir.join(&asset.path))
            .await
            .map_err(AppError::internal)?
            .len();
        if file_len != asset.byte_len {
            return Err(AppError::bad_request("模板图片实际大小与清单不一致。"));
        }
        let bytes = tokio::fs::read(extracted_dir.join(&asset.path))
            .await
            .map_err(AppError::internal)?;
        if detect_image_mime(&bytes) != Some("image/webp")
            || bytes.len() as u64 != asset.byte_len
            || hex_sha256(&bytes) != asset.sha256
            || webp_dimensions(&bytes)? != (asset.width, asset.height)
        {
            return Err(AppError::bad_request(format!(
                "模板图片 `{}` 校验失败。",
                asset.id
            )));
        }
    }
    let existing_ids = sqlx::query_scalar::<_, String>("SELECT id FROM gallery_templates")
        .fetch_all(&state.db)
        .await
        .map_err(AppError::internal)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut skipped_template_count = 0;
    if mode == GalleryImportMode::Merge
        && query.conflict.unwrap_or_default() == GalleryImportConflict::KeepLocal
    {
        manifest.templates.retain(|template| {
            let keep = !existing_ids.contains(&template.id);
            skipped_template_count += usize::from(!keep);
            keep
        });
    }
    let overwritten_template_count = if mode == GalleryImportMode::Merge {
        manifest
            .templates
            .iter()
            .filter(|template| existing_ids.contains(&template.id))
            .count()
    } else {
        0
    };
    let added_template_count = manifest.templates.len() - overwritten_template_count;
    if mode == GalleryImportMode::Merge && manifest.templates.is_empty() {
        return Ok(Json(GalleryImportResponse {
            imported_template_count: 0,
            imported_asset_count: 0,
            mode,
            added_template_count: 0,
            overwritten_template_count: 0,
            skipped_template_count,
        }));
    }
    let used_ids = manifest
        .templates
        .iter()
        .flat_map(|template| {
            template
                .preview_assets
                .iter()
                .chain(&template.reference_assets)
        })
        .map(|asset| asset.id.clone())
        .collect::<BTreeSet<_>>();
    manifest.assets.retain(|asset| used_ids.contains(&asset.id));
    let mut reused_asset_ids = BTreeSet::new();
    if mode == GalleryImportMode::Merge {
        for asset in &mut manifest.assets {
            let Some(row) = sqlx::query("SELECT * FROM gallery_assets WHERE id=?")
                .bind(&asset.id)
                .fetch_optional(&state.db)
                .await
                .map_err(AppError::internal)?
            else {
                continue;
            };
            let same = row.get::<String, _>("sha256") == asset.sha256
                && row.get::<String, _>("mime_type") == asset.mime_type
                && row.get::<i64, _>("byte_len") == asset.byte_len as i64
                && row.get::<i64, _>("width") == i64::from(asset.width)
                && row.get::<i64, _>("height") == i64::from(asset.height);
            if same {
                let bytes = get_object_bytes(
                    &state,
                    &row.get::<String, _>("object_key"),
                    asset.byte_len.max(1),
                )
                .await?;
                if bytes.len() as u64 == asset.byte_len && hex_sha256(&bytes) == asset.sha256 {
                    reused_asset_ids.insert(asset.id.clone());
                    continue;
                }
            }
            // 图片 ID 冲突只能改变本次导入的引用，不能覆盖包外或保留模板的原图。
            let old_id = std::mem::replace(&mut asset.id, new_id());
            for template in &mut manifest.templates {
                for reference in template
                    .preview_assets
                    .iter_mut()
                    .chain(&mut template.reference_assets)
                {
                    if reference.id == old_id {
                        reference.id.clone_from(&asset.id);
                    }
                }
            }
        }
    }
    let mut staged_objects = Vec::with_capacity(manifest.assets.len().saturating_mul(2));
    let mut thumbnail_objects = BTreeMap::<String, (String, u64)>::new();
    let batch_id = new_id();
    for asset in &manifest.assets {
        if reused_asset_ids.contains(&asset.id) {
            continue;
        }
        let bytes = tokio::fs::read(extracted_dir.join(&asset.path))
            .await
            .map_err(AppError::internal)?;
        if detect_image_mime(&bytes) != Some("image/webp")
            || bytes.len() as u64 != asset.byte_len
            || hex_sha256(&bytes) != asset.sha256
            || webp_dimensions(&bytes)? != (asset.width, asset.height)
        {
            cleanup_staged_objects(&state, &staged_objects).await;
            return Err(AppError::bad_request(format!(
                "模板图片 `{}` 校验失败。",
                asset.id
            )));
        }
        let thumbnail = match create_gallery_thumbnail(&bytes) {
            Ok(thumbnail) => thumbnail,
            Err(error) => {
                cleanup_staged_objects(&state, &staged_objects).await;
                return Err(error);
            }
        };
        let object_key = format!("gallery/imports/{batch_id}/{}.webp", asset.id);
        if let Err(error) =
            record_staged_objects(&state.db, std::slice::from_ref(&object_key)).await
        {
            cleanup_staged_objects(&state, &staged_objects).await;
            return Err(error);
        }
        staged_objects.push(object_key.clone());
        if let Err(error) = put_object(&state, &object_key, "image/webp", bytes).await {
            cleanup_staged_objects(&state, &staged_objects).await;
            return Err(error);
        }
        let thumbnail_key = format!("gallery/imports/{batch_id}/thumbnails/{}.webp", asset.id);
        if let Err(error) =
            record_staged_objects(&state.db, std::slice::from_ref(&thumbnail_key)).await
        {
            cleanup_staged_objects(&state, &staged_objects).await;
            return Err(error);
        }
        staged_objects.push(thumbnail_key.clone());
        let thumbnail_byte_len = thumbnail.len() as u64;
        if let Err(error) = put_object(&state, &thumbnail_key, "image/webp", thumbnail).await {
            cleanup_staged_objects(&state, &staged_objects).await;
            return Err(error);
        }
        thumbnail_objects.insert(asset.id.clone(), (thumbnail_key, thumbnail_byte_len));
    }
    let old_keys = match commit_gallery_import(
        &state.db,
        GalleryImportCommit {
            manifest: &manifest,
            mode,
            batch_id: &batch_id,
            thumbnail_objects: &thumbnail_objects,
            staged_object_keys: &staged_objects,
            reused_asset_ids: &reused_asset_ids,
            quota_bytes: state.config.gallery_asset_quota_bytes,
            quota_count: state.config.gallery_asset_quota_count,
        },
    )
    .await
    {
        Ok(keys) => keys,
        Err(error) => {
            cleanup_staged_objects(&state, &staged_objects).await;
            return Err(error);
        }
    };
    let obsolete_keys = old_keys
        .into_iter()
        .filter(|key| !staged_objects.contains(key))
        .collect::<Vec<_>>();
    cleanup_staged_objects(&state, &obsolete_keys).await;
    drop(import_guard);
    Ok(Json(GalleryImportResponse {
        imported_template_count: manifest.templates.len(),
        imported_asset_count: manifest.assets.len(),
        mode,
        added_template_count,
        overwritten_template_count,
        skipped_template_count,
    }))
}

fn write_gallery_zip(
    zip_path: &FsPath,
    assets_dir: &FsPath,
    manifest: &GalleryArchiveManifest,
) -> Result<(), String> {
    let file = StdFile::create(zip_path).map_err(|error| error.to_string())?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    writer
        .start_file("manifest.json", options)
        .map_err(|error| error.to_string())?;
    let json = serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?;
    if json.len() > 8 * 1024 * 1024 {
        return Err("模板清单超过 8 MiB，请缩小导出范围。".into());
    }
    writer.write_all(&json).map_err(|error| error.to_string())?;
    for asset in &manifest.assets {
        writer
            .start_file(&asset.path, options)
            .map_err(|error| error.to_string())?;
        let mut input = StdFile::open(assets_dir.join(format!("{}.webp", asset.id)))
            .map_err(|error| error.to_string())?;
        std::io::copy(&mut input, &mut writer).map_err(|error| error.to_string())?;
    }
    writer.finish().map_err(|error| error.to_string())?;
    Ok(())
}

fn extract_and_validate_gallery_zip(
    zip_path: &FsPath,
    extracted_dir: &FsPath,
    limits: (u64, u64),
) -> Result<GalleryArchiveManifest, String> {
    std::fs::create_dir_all(extracted_dir).map_err(|error| error.to_string())?;
    let file = StdFile::open(zip_path).map_err(|error| error.to_string())?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("模板包不是有效 ZIP：{error}"))?;
    let max_entries = if limits.1 == 0 {
        20_001
    } else {
        limits.1.saturating_add(1).min(20_001) as usize
    };
    if archive.is_empty() || archive.len() > max_entries {
        return Err("模板包文件数量超过限制。".into());
    }
    let mut seen = BTreeSet::new();
    let mut total_uncompressed = 0u64;
    let mut manifest_bytes = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| "模板包包含危险路径。".to_string())?
            .to_path_buf();
        if enclosed
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err("模板包包含危险路径。".into());
        }
        let name = enclosed.to_string_lossy().replace('\\', "/");
        if !seen.insert(name.clone()) {
            return Err("模板包包含重复文件。".into());
        }
        if entry.is_dir() {
            return Err("模板包不允许目录条目。".into());
        }
        total_uncompressed = total_uncompressed
            .checked_add(entry.size())
            .ok_or_else(|| "模板包解压大小溢出。".to_string())?;
        if limits.0 > 0 && total_uncompressed > limits.0.saturating_add(8 * 1024 * 1024) {
            return Err("模板包解压后超过广场资源配额。".into());
        }
        if entry.size() > 4 * 1024 * 1024
            && entry.compressed_size() > 0
            && entry.size() / entry.compressed_size().max(1) > 200
        {
            return Err("模板包包含异常压缩比文件。".into());
        }
        if name == "manifest.json" {
            if entry.size() > 8 * 1024 * 1024 {
                return Err("模板包清单过大。".into());
            }
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            manifest_bytes = Some(bytes);
            continue;
        }
        if !name.starts_with("assets/") || !name.ends_with(".webp") {
            return Err(format!("模板包包含未知文件 `{name}`。"));
        }
        let destination = extracted_dir.join(&enclosed);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut output = StdFile::create(destination).map_err(|error| error.to_string())?;
        let expected_size = entry.size();
        let actual_size = std::io::copy(
            &mut entry.take(expected_size.saturating_add(1)),
            &mut output,
        )
        .map_err(|error| error.to_string())?;
        if actual_size != expected_size {
            return Err("模板包文件实际解压大小与声明不一致。".into());
        }
    }
    let bytes = manifest_bytes.ok_or_else(|| "模板包缺少 manifest.json。".to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| format!("模板包清单无效：{error}"))
}

fn validate_archive_manifest(manifest: &GalleryArchiveManifest) -> Result<(), AppError> {
    if manifest.archive_kind != "gallery_templates" || manifest.schema_version != 1 {
        return Err(AppError::bad_request("不支持的模板包类型或版本。"));
    }
    let asset_map = manifest
        .assets
        .iter()
        .map(|asset| (asset.id.as_str(), asset))
        .collect::<BTreeMap<_, _>>();
    if asset_map.len() != manifest.assets.len() {
        return Err(AppError::bad_request("模板包包含重复资源 ID。"));
    }
    let mut template_ids = BTreeSet::new();
    let mut referenced_asset_ids = BTreeSet::new();
    for template in &manifest.templates {
        if uuid::Uuid::parse_str(&template.id).is_err() || !template_ids.insert(&template.id) {
            return Err(AppError::bad_request("模板包包含无效或重复模板 ID。"));
        }
        let request = archive_template_request(template);
        validate_template_input(&request)?;
        let normalized = normalize_tags(template.tags.iter().map(String::as_str))?;
        if normalized != template.tags {
            return Err(AppError::bad_request("模板包标签没有规范化。"));
        }
        if template.preview_assets.len() > MAX_PREVIEWS
            || template.reference_assets.len() > MAX_REFERENCES
        {
            return Err(AppError::bad_request("模板包中的图片数量超过限制。"));
        }
        let mut relation_ids = BTreeSet::new();
        for (assets, role, max_edge) in [
            (
                &template.preview_assets,
                GalleryAssetRole::Preview,
                MAX_PREVIEW_EDGE,
            ),
            (
                &template.reference_assets,
                GalleryAssetRole::Reference,
                MAX_REFERENCE_EDGE,
            ),
        ] {
            for asset in assets {
                let Some(archive_asset) = asset_map.get(asset.id.as_str()) else {
                    return Err(AppError::bad_request("模板包存在悬空图片引用。"));
                };
                if asset.role != role
                    || !relation_ids.insert(&asset.id)
                    || asset.sha256 != archive_asset.sha256
                    || asset.byte_len != archive_asset.byte_len
                    || asset.width != archive_asset.width
                    || asset.height != archive_asset.height
                    || asset.width.max(asset.height) > max_edge
                {
                    return Err(AppError::bad_request("模板包图片关系或元数据不一致。"));
                }
                referenced_asset_ids.insert(asset.id.as_str());
            }
        }
    }
    if referenced_asset_ids.len() != asset_map.len() {
        return Err(AppError::bad_request("模板包包含未被模板引用的图片。"));
    }
    for asset in &manifest.assets {
        if uuid::Uuid::parse_str(&asset.id).is_err()
            || asset.mime_type != "image/webp"
            || asset.path != format!("assets/{}.webp", asset.id)
            || asset.width == 0
            || asset.height == 0
            || asset.width.max(asset.height) > MAX_REFERENCE_EDGE
        {
            return Err(AppError::bad_request("模板包包含无效图片元数据。"));
        }
    }
    Ok(())
}

fn archive_template_request(template: &GalleryTemplate) -> GalleryTemplateUpsertRequest {
    GalleryTemplateUpsertRequest {
        id: Some(template.id.clone()),
        title: template.title.clone(),
        prompt: template.prompt.clone(),
        description: template.description.clone(),
        tags: template.tags.clone(),
        generation_settings: template.generation_settings.clone(),
        recommended_provider_kind: template.recommended_provider_kind,
        recommended_model: template.recommended_model.clone(),
        preview_asset_ids: template
            .preview_assets
            .iter()
            .map(|asset| asset.id.clone())
            .collect(),
        reference_asset_ids: template
            .reference_assets
            .iter()
            .map(|asset| asset.id.clone())
            .collect(),
        status: template.status,
    }
}

struct GalleryImportCommit<'a> {
    manifest: &'a GalleryArchiveManifest,
    mode: GalleryImportMode,
    batch_id: &'a str,
    thumbnail_objects: &'a BTreeMap<String, (String, u64)>,
    staged_object_keys: &'a [String],
    reused_asset_ids: &'a BTreeSet<String>,
    quota_bytes: u64,
    quota_count: u64,
}

async fn commit_gallery_import(
    db: &SqlitePool,
    commit: GalleryImportCommit<'_>,
) -> Result<Vec<String>, AppError> {
    let GalleryImportCommit {
        manifest,
        mode,
        batch_id,
        thumbnail_objects,
        staged_object_keys,
        reused_asset_ids,
        quota_bytes,
        quota_count,
    } = commit;
    let rows = sqlx::query("SELECT id, object_key, thumbnail_object_key FROM gallery_assets")
        .fetch_all(db)
        .await
        .map_err(AppError::internal)?;
    let imported_ids = manifest
        .assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut old_keys = Vec::new();
    for row in rows {
        let id = row.get::<String, _>("id");
        if mode == GalleryImportMode::Replace
            || (imported_ids.contains(id.as_str()) && !reused_asset_ids.contains(&id))
        {
            old_keys.push(row.get::<String, _>("object_key"));
            if let Some(key) = row.get::<Option<String>, _>("thumbnail_object_key") {
                old_keys.push(key);
            }
        }
    }
    let previously_referenced =
        sqlx::query_scalar::<_, String>("SELECT DISTINCT asset_id FROM gallery_template_assets")
            .fetch_all(db)
            .await
            .map_err(AppError::internal)?
            .into_iter()
            .collect::<BTreeSet<_>>();

    let mut transaction = db.begin().await.map_err(AppError::internal)?;
    if mode == GalleryImportMode::Replace {
        // 全量替换在下方统一处理。
        sqlx::query("DELETE FROM gallery_likes")
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        sqlx::query("DELETE FROM gallery_template_assets")
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        sqlx::query("DELETE FROM gallery_template_tags")
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        sqlx::query("DELETE FROM gallery_templates")
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        sqlx::query("DELETE FROM gallery_assets")
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        sqlx::query("DELETE FROM gallery_tags")
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
    }
    for asset in &manifest.assets {
        if reused_asset_ids.contains(&asset.id) {
            continue;
        }
        let object_key = format!("gallery/imports/{batch_id}/{}.webp", asset.id);
        let (thumbnail_object_key, thumbnail_byte_len) = thumbnail_objects
            .get(&asset.id)
            .map(|(key, byte_len)| (Some(key.as_str()), *byte_len))
            .unwrap_or((None, 0));
        sqlx::query(
            "INSERT INTO gallery_assets
             (id, object_key, thumbnail_object_key, thumbnail_byte_len, mime_type, sha256, byte_len, width, height, created_at)
             VALUES (?, ?, ?, ?, 'image/webp', ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET object_key=excluded.object_key, mime_type=excluded.mime_type,
             thumbnail_object_key=excluded.thumbnail_object_key, thumbnail_byte_len=excluded.thumbnail_byte_len,
             sha256=excluded.sha256, byte_len=excluded.byte_len, width=excluded.width, height=excluded.height",
        ).bind(&asset.id).bind(object_key).bind(thumbnail_object_key).bind(thumbnail_byte_len as i64)
            .bind(&asset.sha256).bind(asset.byte_len as i64)
            .bind(i64::from(asset.width)).bind(i64::from(asset.height)).bind(&asset.created_at)
            .execute(&mut *transaction).await.map_err(AppError::internal)?;
    }
    for template in &manifest.templates {
        let settings =
            serde_json::to_string(&template.generation_settings).map_err(AppError::internal)?;
        let provider = serde_json::to_string(&template.recommended_provider_kind)
            .map_err(AppError::internal)?;
        sqlx::query(
            "INSERT INTO gallery_templates
             (id, title, prompt, description, generation_settings, recommended_provider_kind, recommended_model, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET title=excluded.title, prompt=excluded.prompt,
             description=excluded.description, generation_settings=excluded.generation_settings,
             recommended_provider_kind=excluded.recommended_provider_kind, recommended_model=excluded.recommended_model,
             status=excluded.status, created_at=excluded.created_at, updated_at=excluded.updated_at",
        ).bind(&template.id).bind(&template.title).bind(&template.prompt).bind(&template.description)
            .bind(settings).bind(provider).bind(&template.recommended_model).bind(status_value(template.status))
            .bind(&template.created_at).bind(&template.updated_at)
            .execute(&mut *transaction).await.map_err(AppError::internal)?;
        sqlx::query("DELETE FROM gallery_template_tags WHERE template_id = ?")
            .bind(&template.id)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        sqlx::query("DELETE FROM gallery_template_assets WHERE template_id = ?")
            .bind(&template.id)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
        for (position, tag) in template.tags.iter().enumerate() {
            sqlx::query("INSERT OR IGNORE INTO gallery_tags (name, created_at) VALUES (?, ?)")
                .bind(tag)
                .bind(&template.created_at)
                .execute(&mut *transaction)
                .await
                .map_err(AppError::internal)?;
            sqlx::query("INSERT INTO gallery_template_tags (template_id, tag_name, position) VALUES (?, ?, ?)")
                .bind(&template.id).bind(tag).bind(position as i64)
                .execute(&mut *transaction).await.map_err(AppError::internal)?;
        }
        insert_asset_relations(
            &mut transaction,
            &template.id,
            &template
                .preview_assets
                .iter()
                .map(|asset| asset.id.clone())
                .collect::<Vec<_>>(),
            "preview",
        )
        .await?;
        insert_asset_relations(
            &mut transaction,
            &template.id,
            &template
                .reference_assets
                .iter()
                .map(|asset| asset.id.clone())
                .collect::<Vec<_>>(),
            "reference",
        )
        .await?;
    }
    let orphan_rows = sqlx::query("SELECT id, object_key, thumbnail_object_key FROM gallery_assets WHERE NOT EXISTS (SELECT 1 FROM gallery_template_assets gta WHERE gta.asset_id=gallery_assets.id)")
        .fetch_all(&mut *transaction).await.map_err(AppError::internal)?;
    // 未关联的编辑器上传不能误删，只回收此次导入前已有模板实际引用过的旧资源。
    for row in orphan_rows {
        let id = row.get::<String, _>("id");
        if !previously_referenced.contains(&id) {
            continue;
        }
        old_keys.push(row.get::<String, _>("object_key"));
        if let Some(key) = row.get::<Option<String>, _>("thumbnail_object_key") {
            old_keys.push(key);
        }
        sqlx::query("DELETE FROM gallery_assets WHERE id=?")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
    }
    let usage = sqlx::query("SELECT COALESCE(SUM(byte_len + thumbnail_byte_len),0) AS bytes, COUNT(*) + COUNT(thumbnail_object_key) AS files FROM gallery_assets")
        .fetch_one(&mut *transaction).await.map_err(AppError::internal)?;
    if (quota_bytes > 0 && usage.get::<i64, _>("bytes").max(0) as u64 > quota_bytes)
        || (quota_count > 0 && usage.get::<i64, _>("files").max(0) as u64 > quota_count)
    {
        return Err(AppError::bad_request(
            "合并后的模板广场资源超过容量或文件数量配额。",
        ));
    }
    sqlx::query("DELETE FROM gallery_tags WHERE NOT EXISTS (SELECT 1 FROM gallery_template_tags gtt WHERE gtt.tag_name = gallery_tags.name)")
        .execute(&mut *transaction).await.map_err(AppError::internal)?;
    for object_key in staged_object_keys {
        sqlx::query("DELETE FROM gallery_staged_objects WHERE object_key = ?")
            .bind(object_key)
            .execute(&mut *transaction)
            .await
            .map_err(AppError::internal)?;
    }
    let cleanup_created_at = now_rfc3339();
    for object_key in &old_keys {
        sqlx::query(
            "INSERT INTO gallery_staged_objects (object_key, created_at) VALUES (?, ?)
             ON CONFLICT(object_key) DO UPDATE SET created_at = excluded.created_at",
        )
        .bind(object_key)
        .bind(&cleanup_created_at)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
    }
    transaction.commit().await.map_err(AppError::internal)?;
    Ok(old_keys)
}

async fn record_staged_objects(db: &SqlitePool, keys: &[String]) -> Result<(), AppError> {
    let mut transaction = db.begin().await.map_err(AppError::internal)?;
    let created_at = now_rfc3339();
    for key in keys {
        sqlx::query(
            "INSERT INTO gallery_staged_objects (object_key, created_at) VALUES (?, ?)
             ON CONFLICT(object_key) DO UPDATE SET created_at = excluded.created_at",
        )
        .bind(key)
        .bind(&created_at)
        .execute(&mut *transaction)
        .await
        .map_err(AppError::internal)?;
    }
    transaction.commit().await.map_err(AppError::internal)
}

async fn cleanup_staged_objects(state: &AppState, keys: &[String]) {
    for key in keys {
        if delete_object(state, key).await.is_err() {
            continue;
        }
        let _ = sqlx::query("DELETE FROM gallery_staged_objects WHERE object_key = ?")
            .bind(key)
            .execute(&state.db)
            .await;
    }
}

async fn unique_temp_path(prefix: &str, directory: bool) -> Result<PathBuf, AppError> {
    let path = std::env::temp_dir().join(format!("{prefix}-{}", new_id()));
    if directory {
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(AppError::internal)?;
    }
    Ok(path)
}

fn gallery_archive_byte_limit(state: &AppState) -> usize {
    let bytes = state
        .config
        .gallery_asset_quota_bytes
        .saturating_add(8 * 1024 * 1024);
    usize::try_from(bytes).unwrap_or(usize::MAX)
}

async fn template_from_row(
    db: &SqlitePool,
    row: sqlx::sqlite::SqliteRow,
) -> Result<GalleryTemplate, AppError> {
    let id = row.get::<String, _>("id");
    Ok(GalleryTemplate {
        tags: load_tags(db, &id).await?,
        preview_assets: load_assets(db, &id, "preview").await?,
        reference_assets: load_assets(db, &id, "reference").await?,
        id,
        title: row.get("title"),
        prompt: row.get("prompt"),
        description: row.get("description"),
        generation_settings: serde_json::from_str(&row.get::<String, _>("generation_settings"))
            .map_err(AppError::internal)?,
        recommended_provider_kind: serde_json::from_str(
            &row.get::<String, _>("recommended_provider_kind"),
        )
        .map_err(AppError::internal)?,
        recommended_model: row.get("recommended_model"),
        status: parse_status(&row.get::<String, _>("status"))?,
        like_count: row.try_get::<i64, _>("like_count").unwrap_or(0).max(0) as u64,
        liked_by_viewer: row.try_get::<i64, _>("liked_by_viewer").unwrap_or(0) != 0,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn get_template_for_admin(state: &AppState, id: &str) -> Result<GalleryTemplate, AppError> {
    let row = sqlx::query(
        "SELECT gt.*, (SELECT COUNT(*) FROM gallery_likes gl WHERE gl.template_id = gt.id) AS like_count,
         0 AS liked_by_viewer FROM gallery_templates gt WHERE gt.id = ?",
    ).bind(id).fetch_optional(&state.db).await.map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("模板不存在。"))?;
    template_from_row(&state.db, row).await
}

async fn load_tags(db: &SqlitePool, template_id: &str) -> Result<Vec<String>, AppError> {
    Ok(sqlx::query(
        "SELECT tag_name FROM gallery_template_tags WHERE template_id = ? ORDER BY position",
    )
    .bind(template_id)
    .fetch_all(db)
    .await
    .map_err(AppError::internal)?
    .into_iter()
    .map(|row| row.get("tag_name"))
    .collect())
}

async fn load_assets(
    db: &SqlitePool,
    template_id: &str,
    role: &str,
) -> Result<Vec<GalleryAsset>, AppError> {
    let rows = sqlx::query(
        "SELECT ga.*, gta.role FROM gallery_template_assets gta JOIN gallery_assets ga ON ga.id = gta.asset_id
         WHERE gta.template_id = ? AND gta.role = ? ORDER BY gta.position",
    ).bind(template_id).bind(role).fetch_all(db).await.map_err(AppError::internal)?;
    rows.into_iter()
        .map(|row| asset_from_row(row, role))
        .collect()
}

fn asset_from_row(row: sqlx::sqlite::SqliteRow, role: &str) -> Result<GalleryAsset, AppError> {
    Ok(GalleryAsset {
        id: row.get("id"),
        role: if role == "reference" {
            GalleryAssetRole::Reference
        } else {
            GalleryAssetRole::Preview
        },
        sha256: row.get("sha256"),
        mime_type: row.get("mime_type"),
        byte_len: row.get::<i64, _>("byte_len").max(0) as u64,
        width: u32::try_from(row.get::<i64, _>("width"))
            .map_err(|_| AppError::internal_message("模板图片宽度记录无效。"))?,
        height: u32::try_from(row.get::<i64, _>("height"))
            .map_err(|_| AppError::internal_message("模板图片高度记录无效。"))?,
        created_at: row.get("created_at"),
    })
}

async fn insert_asset_relations(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    template_id: &str,
    asset_ids: &[String],
    role: &str,
) -> Result<(), AppError> {
    for (position, asset_id) in asset_ids.iter().enumerate() {
        sqlx::query("INSERT INTO gallery_template_assets (template_id, asset_id, role, position) VALUES (?, ?, ?, ?)")
            .bind(template_id).bind(asset_id).bind(role).bind(position as i64)
            .execute(&mut **transaction).await.map_err(AppError::internal)?;
    }
    Ok(())
}

async fn validate_template_asset_ids(
    state: &AppState,
    payload: &GalleryTemplateUpsertRequest,
) -> Result<(), AppError> {
    if payload.preview_asset_ids.len() > MAX_PREVIEWS {
        return Err(AppError::bad_request("每个模板最多使用 6 张预览图。"));
    }
    if payload.reference_asset_ids.len() > mew_image_shared::MAX_GENERATION_REFERENCE_IMAGES {
        return Err(AppError::bad_request(
            "每个模板最多使用 10 张参考图；旧模板请精简后再保存。",
        ));
    }
    let mut unique = BTreeSet::new();
    for (ids, max_edge, label) in [
        (&payload.preview_asset_ids, MAX_PREVIEW_EDGE, "预览图"),
        (&payload.reference_asset_ids, MAX_REFERENCE_EDGE, "参考图"),
    ] {
        for id in ids {
            if !unique.insert(id) {
                return Err(AppError::bad_request("同一张图片不能在模板中重复使用。"));
            }
            let row = sqlx::query("SELECT width, height FROM gallery_assets WHERE id = ?")
                .bind(id)
                .fetch_optional(&state.db)
                .await
                .map_err(AppError::internal)?
                .ok_or_else(|| AppError::bad_request(format!("{label} `{id}` 不存在。")))?;
            let edge = row.get::<i64, _>("width").max(row.get::<i64, _>("height"));
            if edge > i64::from(max_edge) {
                return Err(AppError::bad_request(format!("{label}尺寸超过限制。")));
            }
        }
    }
    Ok(())
}

fn validate_template_input(payload: &GalleryTemplateUpsertRequest) -> Result<(), AppError> {
    validate_text("标题", &payload.title, 1, MAX_TITLE_CHARS)?;
    validate_text("提示词", &payload.prompt, 1, MAX_PROMPT_CHARS)?;
    validate_text("说明", &payload.description, 0, MAX_DESCRIPTION_CHARS)?;
    validate_text("推荐模型", &payload.recommended_model, 1, MAX_MODEL_CHARS)?;
    let normalized_size = mew_image_shared::clamp_size(
        payload.generation_settings.width,
        payload.generation_settings.height,
    );
    if normalized_size.adjusted || !(1..=4).contains(&payload.generation_settings.count) {
        return Err(AppError::bad_request("模板生成参数无效。"));
    }
    if payload.status == GalleryTemplateStatus::Published && payload.preview_asset_ids.is_empty() {
        return Err(AppError::bad_request("发布模板前至少需要一张预览图。"));
    }
    Ok(())
}

fn validate_text(label: &str, value: &str, min: usize, max: usize) -> Result<(), AppError> {
    let chars = value.trim().chars().count();
    if chars < min || chars > max {
        return Err(AppError::bad_request(format!(
            "{label}长度必须在 {min}–{max} 个字符之间。"
        )));
    }
    Ok(())
}

fn normalize_tags<'a>(values: impl IntoIterator<Item = &'a str>) -> Result<Vec<String>, AppError> {
    let mut tags = Vec::new();
    for value in values {
        let tag = value.trim().to_lowercase();
        if tag.is_empty() || tags.contains(&tag) {
            continue;
        }
        if tag.chars().count() > MAX_TAG_CHARS {
            return Err(AppError::bad_request("单个标签不能超过 32 个字符。"));
        }
        tags.push(tag);
    }
    if tags.len() > MAX_TAGS {
        return Err(AppError::bad_request("每个模板最多使用 12 个标签。"));
    }
    Ok(tags)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn status_value(status: GalleryTemplateStatus) -> &'static str {
    match status {
        GalleryTemplateStatus::Draft => "draft",
        GalleryTemplateStatus::Published => "published",
        GalleryTemplateStatus::Archived => "archived",
    }
}

fn parse_status(value: &str) -> Result<GalleryTemplateStatus, AppError> {
    match value {
        "draft" => Ok(GalleryTemplateStatus::Draft),
        "published" => Ok(GalleryTemplateStatus::Published),
        "archived" => Ok(GalleryTemplateStatus::Archived),
        _ => Err(AppError::internal_message("模板状态记录无效。")),
    }
}

async fn viewer_is_admin(state: &AppState, session: &Session) -> Result<bool, AppError> {
    Ok(current_user(state, session)
        .await?
        .is_some_and(|user| user.status == "approved" && user.role == "admin"))
}

async fn viewer_actor_key(
    state: &AppState,
    session: &Session,
    cookies: &Cookies,
) -> Result<String, AppError> {
    if let Some(user) = current_user(state, session)
        .await?
        .filter(|user| user.status == "approved")
    {
        return Ok(format!("user:{}", user.id));
    }
    let visitor_id = if let Some(cookie) = cookies
        .get(GALLERY_VISITOR_COOKIE)
        .filter(|cookie| uuid::Uuid::parse_str(cookie.value()).is_ok())
    {
        cookie.value().to_string()
    } else {
        let visitor_id = new_id();
        let mut cookie = Cookie::new(GALLERY_VISITOR_COOKIE, visitor_id.clone());
        cookie.set_path("/");
        cookie.set_http_only(true);
        cookie.set_same_site(SameSite::Lax);
        cookie.set_secure(state.config.session_secure);
        cookie.set_max_age(CookieDuration::days(3650));
        cookies.add(cookie);
        visitor_id
    };
    Ok(format!(
        "guest:{}",
        hash_auth_identifier(&state.config.auth_secret, "gallery_visitor", &visitor_id)
    ))
}

fn webp_dimensions(bytes: &[u8]) -> Result<(u32, u32), AppError> {
    let reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::WebP);
    reader
        .into_dimensions()
        .map_err(|_| AppError::bad_request("无法读取 WebP 图片尺寸。"))
}

fn create_gallery_thumbnail(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    let image = image::load_from_memory_with_format(bytes, ImageFormat::WebP)
        .map_err(|_| AppError::bad_request("无法解码模板预览图。"))?;
    let thumbnail = image.thumbnail(MAX_THUMBNAIL_EDGE, MAX_THUMBNAIL_EDGE);
    let mut output = Cursor::new(Vec::new());
    thumbnail
        .write_to(&mut output, ImageFormat::WebP)
        .map_err(AppError::internal)?;
    let bytes = output.into_inner();
    if bytes.is_empty() || bytes.len() as u64 > MAX_THUMBNAIL_BYTES {
        return Err(AppError::bad_request("模板预览缩略图编码结果异常。"));
    }
    Ok(bytes)
}

async fn ensure_gallery_quota(
    state: &AppState,
    additional_bytes: u64,
    additional_count: u64,
) -> Result<(), AppError> {
    let row = sqlx::query(
        "SELECT COALESCE(SUM(byte_len + thumbnail_byte_len), 0) AS bytes,
         COUNT(*) + COALESCE(SUM(CASE WHEN thumbnail_object_key IS NULL THEN 0 ELSE 1 END), 0) AS count
         FROM gallery_assets",
    )
    .fetch_one(&state.db)
    .await
    .map_err(AppError::internal)?;
    let bytes = row.get::<i64, _>("bytes").max(0) as u64;
    let count = row.get::<i64, _>("count").max(0) as u64;
    if state.config.gallery_asset_quota_bytes > 0
        && bytes.saturating_add(additional_bytes) > state.config.gallery_asset_quota_bytes
    {
        return Err(AppError::bad_request("模板广场资源容量已满。"));
    }
    if state.config.gallery_asset_quota_count > 0
        && count.saturating_add(additional_count) > state.config.gallery_asset_quota_count
    {
        return Err(AppError::bad_request("模板广场资源数量已达上限。"));
    }
    Ok(())
}

async fn cleanup_unused_tags(db: &SqlitePool) -> Result<(), AppError> {
    sqlx::query("DELETE FROM gallery_tags WHERE NOT EXISTS (SELECT 1 FROM gallery_template_tags gtt WHERE gtt.tag_name = gallery_tags.name)")
        .execute(db).await.map_err(AppError::internal)?;
    Ok(())
}

async fn cleanup_unreferenced_assets(
    state: &AppState,
    candidate_ids: &[String],
) -> Result<(), AppError> {
    for asset_id in candidate_ids {
        let row = sqlx::query(
            "SELECT ga.object_key, ga.thumbnail_object_key,
             EXISTS(SELECT 1 FROM gallery_template_assets gta WHERE gta.asset_id = ga.id) AS in_use
             FROM gallery_assets ga WHERE ga.id = ?",
        )
        .bind(asset_id)
        .fetch_optional(&state.db)
        .await
        .map_err(AppError::internal)?;
        let Some(row) = row else {
            continue;
        };
        if row.get::<i64, _>("in_use") != 0 {
            continue;
        }
        let object_key = row.get::<String, _>("object_key");
        let thumbnail_object_key = row.get::<Option<String>, _>("thumbnail_object_key");
        if let Some(thumbnail_key) = thumbnail_object_key.as_deref() {
            delete_object(state, thumbnail_key).await?;
        }
        delete_object(state, &object_key).await?;
        sqlx::query("DELETE FROM gallery_assets WHERE id = ?")
            .bind(asset_id)
            .execute(&state.db)
            .await
            .map_err(AppError::internal)?;
    }
    Ok(())
}

async fn cleanup_expired_orphan_assets(state: &AppState) -> Result<(), AppError> {
    cleanup_expired_staged_objects(state).await?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let rows = sqlx::query(
        "SELECT ga.id, ga.object_key, ga.thumbnail_object_key FROM gallery_assets ga
         WHERE ga.created_at < ? AND NOT EXISTS (
             SELECT 1 FROM gallery_template_assets gta WHERE gta.asset_id = ga.id
         )",
    )
    .bind(cutoff)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::internal)?;
    for row in rows {
        let id = row.get::<String, _>("id");
        let object_key = row.get::<String, _>("object_key");
        let thumbnail_object_key = row.get::<Option<String>, _>("thumbnail_object_key");
        if let Some(thumbnail_key) = thumbnail_object_key.as_deref()
            && delete_object(state, thumbnail_key).await.is_err()
        {
            continue;
        }
        if delete_object(state, &object_key).await.is_err() {
            continue;
        }
        sqlx::query("DELETE FROM gallery_assets WHERE id = ?")
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(AppError::internal)?;
    }
    Ok(())
}

pub(crate) async fn cleanup_expired_staged_objects(state: &AppState) -> Result<(), AppError> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let keys = sqlx::query(
        "SELECT object_key FROM gallery_staged_objects WHERE created_at < ? ORDER BY created_at",
    )
    .bind(cutoff)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::internal)?
    .into_iter()
    .map(|row| row.get::<String, _>("object_key"))
    .collect::<Vec<_>>();
    cleanup_staged_objects(state, &keys).await;
    Ok(())
}

#[cfg(test)]
#[path = "gallery_transfer_tests.rs"]
mod transfer_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba, RgbaImage};
    use sqlx::sqlite::SqlitePoolOptions;

    pub(super) async fn test_db() -> SqlitePool {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&db)
            .await
            .unwrap();
        init_db(&db).await.unwrap();
        db
    }

    fn test_settings() -> mew_image_shared::GenerationSettingsSnapshot {
        mew_image_shared::GenerationSettingsSnapshot {
            automatic_size: false,
            width: 1024,
            height: 1024,
            quality: Some("high".into()),
            count: 1,
            endpoint_mode: mew_image_shared::ProviderEndpointMode::ImagesApi,
            output_format: Some("webp".into()),
            output_compression: Some(90),
            background: None,
            moderation: None,
            responses_model: None,
        }
    }

    pub(super) fn test_template(
        id: String,
        title: &str,
        status: GalleryTemplateStatus,
    ) -> GalleryTemplate {
        GalleryTemplate {
            id,
            title: title.into(),
            prompt: format!("{title} prompt"),
            description: String::new(),
            tags: Vec::new(),
            generation_settings: test_settings(),
            recommended_provider_kind: mew_image_shared::ProviderKind::OpenAiImage,
            recommended_model: "gpt-image-2".into(),
            preview_assets: Vec::new(),
            reference_assets: Vec::new(),
            status,
            like_count: 0,
            liked_by_viewer: false,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        }
    }

    pub(super) async fn insert_test_template(
        db: &SqlitePool,
        template: &GalleryTemplate,
        tags: &[&str],
    ) {
        sqlx::query(
            "INSERT INTO gallery_templates
             (id, title, prompt, description, generation_settings, recommended_provider_kind,
              recommended_model, status, created_at, updated_at)
             VALUES (?, ?, ?, '', ?, ?, ?, ?, ?, ?)",
        )
        .bind(&template.id)
        .bind(&template.title)
        .bind(&template.prompt)
        .bind(serde_json::to_string(&template.generation_settings).unwrap())
        .bind(serde_json::to_string(&template.recommended_provider_kind).unwrap())
        .bind(&template.recommended_model)
        .bind(status_value(template.status))
        .bind(&template.created_at)
        .bind(&template.updated_at)
        .execute(db)
        .await
        .unwrap();
        for (position, tag) in tags.iter().enumerate() {
            sqlx::query("INSERT OR IGNORE INTO gallery_tags (name, created_at) VALUES (?, ?)")
                .bind(tag)
                .bind(&template.created_at)
                .execute(db)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO gallery_template_tags (template_id, tag_name, position) VALUES (?, ?, ?)",
            )
            .bind(&template.id)
            .bind(tag)
            .bind(position as i64)
            .execute(db)
            .await
            .unwrap();
        }
    }

    #[test]
    fn tags_are_normalized_deduplicated_and_bounded() {
        assert_eq!(
            normalize_tags([" 人像 ", "人像", "光影"]).unwrap(),
            ["人像", "光影"]
        );
        assert!(normalize_tags((0..13).map(|_| "标签")).is_ok());
        let values = (0..13)
            .map(|index| format!("标签{index}"))
            .collect::<Vec<_>>();
        assert!(normalize_tags(values.iter().map(String::as_str)).is_err());
    }

    #[test]
    fn like_search_escaping_treats_wildcards_as_text() {
        assert_eq!(escape_like("50%_x\\y"), "50\\%\\_x\\\\y");
    }

    #[test]
    fn preview_thumbnail_is_bounded_webp() {
        let source =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(1_200, 600, Rgba([20, 40, 80, 180])));
        let mut encoded = Cursor::new(Vec::new());
        source.write_to(&mut encoded, ImageFormat::WebP).unwrap();
        let thumbnail = create_gallery_thumbnail(&encoded.into_inner()).unwrap();
        assert_eq!(detect_image_mime(&thumbnail), Some("image/webp"));
        assert_eq!(webp_dimensions(&thumbnail).unwrap(), (480, 240));
    }

    #[tokio::test]
    async fn list_filters_use_published_visibility_and_tag_and_semantics() {
        let db = test_db().await;
        let published_match = test_template(
            new_id(),
            "published match",
            GalleryTemplateStatus::Published,
        );
        let published_partial = test_template(
            new_id(),
            "published partial",
            GalleryTemplateStatus::Published,
        );
        let draft_match = test_template(new_id(), "draft match", GalleryTemplateStatus::Draft);
        insert_test_template(&db, &published_match, &["portrait", "night"]).await;
        insert_test_template(&db, &published_partial, &["portrait"]).await;
        insert_test_template(&db, &draft_match, &["portrait", "night", "状态/草稿专用"]).await;
        let tags = vec!["portrait".to_string(), "night".to_string()];

        let public_tags = load_tag_summaries(&db, true).await.unwrap();
        let admin_tags = load_tag_summaries(&db, false).await.unwrap();
        assert_eq!(
            public_tags
                .iter()
                .find(|tag| tag.name == "portrait")
                .map(|tag| tag.template_count),
            Some(2)
        );
        assert_eq!(
            admin_tags
                .iter()
                .find(|tag| tag.name == "portrait")
                .map(|tag| tag.template_count),
            Some(3)
        );
        assert!(public_tags.iter().all(|tag| tag.name != "状态/草稿专用"));
        assert_eq!(
            admin_tags
                .iter()
                .find(|tag| tag.name == "状态/草稿专用")
                .map(|tag| tag.template_count),
            Some(1)
        );

        let public_rows = fetch_template_list_rows(
            &db,
            TemplateListFilter {
                actor_key: "guest:test",
                is_admin: false,
                search: None,
                tags: &tags,
                popular: false,
                page: 1,
                page_size: 24,
            },
        )
        .await
        .unwrap();
        assert_eq!(public_rows.len(), 1);
        assert_eq!(public_rows[0].get::<String, _>("id"), published_match.id);

        let admin_rows = fetch_template_list_rows(
            &db,
            TemplateListFilter {
                actor_key: "user:admin",
                is_admin: true,
                search: Some("match"),
                tags: &tags,
                popular: false,
                page: 1,
                page_size: 24,
            },
        )
        .await
        .unwrap();
        assert_eq!(admin_rows.len(), 2);
    }

    #[tokio::test]
    async fn existing_gallery_asset_table_gains_thumbnail_columns() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE gallery_assets (
                id TEXT PRIMARY KEY, object_key TEXT NOT NULL UNIQUE, mime_type TEXT NOT NULL,
                sha256 TEXT NOT NULL, byte_len INTEGER NOT NULL, width INTEGER NOT NULL,
                height INTEGER NOT NULL, created_at TEXT NOT NULL
            )",
        )
        .execute(&db)
        .await
        .unwrap();

        init_db(&db).await.unwrap();
        let columns = sqlx::query("PRAGMA table_info(gallery_assets)")
            .fetch_all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<BTreeSet<_>>();
        assert!(columns.contains("thumbnail_object_key"));
        assert!(columns.contains("thumbnail_byte_len"));
    }

    #[tokio::test]
    async fn replace_import_rolls_back_all_rows_when_a_relation_fails() {
        let db = test_db().await;
        let existing = test_template(new_id(), "existing", GalleryTemplateStatus::Published);
        insert_test_template(&db, &existing, &[]).await;
        let mut invalid = test_template(new_id(), "invalid", GalleryTemplateStatus::Draft);
        invalid.tags = vec!["duplicate".into(), "duplicate".into()];
        let manifest = GalleryArchiveManifest {
            archive_kind: "gallery_templates".into(),
            schema_version: 1,
            exported_at: now_rfc3339(),
            templates: vec![invalid],
            assets: Vec::new(),
        };

        assert!(
            commit_gallery_import(
                &db,
                GalleryImportCommit {
                    manifest: &manifest,
                    mode: GalleryImportMode::Replace,
                    batch_id: "batch",
                    thumbnail_objects: &BTreeMap::new(),
                    staged_object_keys: &[],
                    reused_asset_ids: &BTreeSet::new(),
                    quota_bytes: 1024,
                    quota_count: 10,
                },
            )
            .await
            .is_err()
        );
        let remaining =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM gallery_templates WHERE id = ?")
                .bind(existing.id)
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn gallery_zip_rejects_parent_directory_entries() {
        let root = std::env::temp_dir().join(format!("mew-gallery-test-{}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let zip_path = root.join("dangerous.zip");
        let file = StdFile::create(&zip_path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file("../escape.webp", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"not an image").unwrap();
        writer.finish().unwrap();
        let result = extract_and_validate_gallery_zip(
            &zip_path,
            &root.join("output"),
            (5 * 1024 * 1024, 10),
        );
        assert!(result.unwrap_err().contains("危险路径"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn archive_manifest_rejects_dangling_asset_references() {
        let asset = GalleryAsset {
            id: new_id(),
            role: GalleryAssetRole::Preview,
            sha256: "00".repeat(32),
            mime_type: "image/webp".into(),
            byte_len: 12,
            width: 1,
            height: 1,
            created_at: now_rfc3339(),
        };
        let template = GalleryTemplate {
            id: new_id(),
            title: "测试模板".into(),
            prompt: "测试提示词".into(),
            description: String::new(),
            tags: vec!["测试".into()],
            generation_settings: mew_image_shared::GenerationSettingsSnapshot {
                automatic_size: false,
                width: 1024,
                height: 1024,
                quality: Some("high".into()),
                count: 1,
                endpoint_mode: mew_image_shared::ProviderEndpointMode::ImagesApi,
                output_format: Some("webp".into()),
                output_compression: Some(90),
                background: None,
                moderation: None,
                responses_model: None,
            },
            recommended_provider_kind: mew_image_shared::ProviderKind::OpenAiImage,
            recommended_model: "gpt-image-2".into(),
            preview_assets: vec![asset],
            reference_assets: Vec::new(),
            status: GalleryTemplateStatus::Published,
            like_count: 0,
            liked_by_viewer: false,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        };
        let manifest = GalleryArchiveManifest {
            archive_kind: "gallery_templates".into(),
            schema_version: 1,
            exported_at: now_rfc3339(),
            templates: vec![template],
            assets: Vec::new(),
        };
        assert!(validate_archive_manifest(&manifest).is_err());
    }
}
