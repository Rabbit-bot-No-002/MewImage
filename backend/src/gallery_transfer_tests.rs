use super::tests::{insert_test_template, test_db, test_template};
use super::*;
use crate::tests::{authenticated_test_session, insert_test_user, test_app_state};
use image::{DynamicImage, Rgba, RgbaImage};
use tower_sessions::MemoryStore;

#[tokio::test]
async fn selection_handles_or_statuses_boundaries_and_more_than_one_page() {
    let db = test_db().await;
    for index in 0..30 {
        insert_test_template(
            &db,
            &test_template(
                new_id(),
                &format!("海报{index}"),
                GalleryTemplateStatus::Published,
            ),
            &["海报/商业", "用途/海报"],
        )
        .await;
    }
    for (title, status, tags) in [
        (
            "前缀",
            GalleryTemplateStatus::Published,
            vec!["海报设计/商业"],
        ),
        ("草稿", GalleryTemplateStatus::Draft, vec!["海报/商业"]),
        ("归档", GalleryTemplateStatus::Archived, vec!["海报/商业"]),
        (
            "具体标签",
            GalleryTemplateStatus::Published,
            vec!["用途/动漫"],
        ),
        ("无分类", GalleryTemplateStatus::Published, vec!["普通标签"]),
        ("无标签", GalleryTemplateStatus::Published, vec![]),
        ("特殊", GalleryTemplateStatus::Published, vec!["a%_/b"]),
    ] {
        insert_test_template(&db, &test_template(new_id(), title, status), &tags).await;
    }
    let mut filter = GalleryExportFilter {
        categories: vec!["海报".into()],
        tags: vec!["用途/动漫".into()],
        ..Default::default()
    };
    let (selected, _) = load_export_selection(&db, &filter).await.unwrap();
    assert_eq!(selected.len(), 31);
    assert_eq!(
        selected
            .iter()
            .map(|value| &value.id)
            .collect::<BTreeSet<_>>()
            .len(),
        31
    );
    filter.statuses.push(GalleryTemplateStatus::Archived);
    assert_eq!(
        load_export_selection(&db, &filter).await.unwrap().0.len(),
        32
    );
    filter.categories.clear();
    filter.tags.clear();
    filter.uncategorized = true;
    assert_eq!(
        load_export_selection(&db, &filter).await.unwrap().0.len(),
        2
    );
    filter.uncategorized = false;
    filter.categories = vec!["a%_".into()];
    assert_eq!(
        load_export_selection(&db, &filter).await.unwrap().0.len(),
        1
    );
    filter.scope = GalleryExportScope::Backup;
    assert_eq!(
        load_export_selection(&db, &filter).await.unwrap().0.len(),
        37
    );
}

#[test]
fn filter_validation_and_legacy_defaults_are_explicit() {
    assert!(normalize_export_filter(GalleryExportFilter::default()).is_err());
    assert!(
        normalize_export_filter(GalleryExportFilter {
            categories: vec!["字".repeat(33)],
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        normalize_export_filter(GalleryExportFilter {
            tags: (0..257).map(|n| n.to_string()).collect(),
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(
        normalize_export_filter(GalleryExportFilter {
            tags: vec![" ABC ".into(), "abc".into()],
            ..Default::default()
        })
        .unwrap()
        .tags,
        ["abc"]
    );
    assert_eq!(
        serde_json::from_str::<ImportModeQuery>("{}")
            .unwrap()
            .conflict
            .unwrap_or_default(),
        GalleryImportConflict::Overwrite
    );
    assert!(serde_json::from_str::<GalleryExportFilter>(r#"{"scope":"unknown"}"#).is_err());
}

async fn site() -> (Arc<AppState>, Session, TemporaryPath) {
    let root = unique_temp_path("mew-transfer-test", true).await.unwrap();
    let state = test_app_state(root.to_string_lossy().into_owned()).await;
    init_db(&state.db).await.unwrap();
    insert_test_user(
        &state.db, "admin", "admin", "unused", "admin", "approved", 0,
    )
    .await;
    let session = authenticated_test_session(Arc::new(MemoryStore::default()), "admin", 0).await;
    (Arc::new(state), session, TemporaryPath(root))
}

async fn add_image(state: &AppState, id: &str, color: u8) -> GalleryAsset {
    let image = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([color, 50, 80, 200])));
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, ImageFormat::WebP).unwrap();
    let bytes = output.into_inner();
    let key = format!("gallery/test/{id}.webp");
    let asset = GalleryAsset {
        id: id.into(),
        role: GalleryAssetRole::Preview,
        sha256: hex_sha256(&bytes),
        mime_type: "image/webp".into(),
        byte_len: bytes.len() as u64,
        width: 8,
        height: 8,
        created_at: now_rfc3339(),
    };
    put_object(state, &key, "image/webp", bytes).await.unwrap();
    sqlx::query("INSERT INTO gallery_assets (id,object_key,mime_type,sha256,byte_len,width,height,created_at) VALUES (?,?,'image/webp',?,?,8,8,?)")
        .bind(id).bind(key).bind(&asset.sha256).bind(asset.byte_len as i64).bind(&asset.created_at).execute(&state.db).await.unwrap();
    asset
}

async fn add_template(state: &AppState, id: &str, title: &str, image: &GalleryAsset) {
    insert_test_template(
        &state.db,
        &test_template(id.into(), title, GalleryTemplateStatus::Published),
        &["海报/商业", "用途/分享"],
    )
    .await;
    sqlx::query("INSERT INTO gallery_template_assets (template_id,asset_id,role,position) VALUES (?,?,'preview',0)").bind(id).bind(&image.id).execute(&state.db).await.unwrap();
}

async fn import_bytes(
    state: Arc<AppState>,
    session: Session,
    bytes: &[u8],
    conflict: GalleryImportConflict,
) -> Result<GalleryImportResponse, AppError> {
    import_gallery_archive(
        State(state),
        session,
        Query(ImportModeQuery {
            mode: Some(GalleryImportMode::Merge),
            conflict: Some(conflict),
        }),
        Request::builder().body(Body::from(bytes.to_vec())).unwrap(),
    )
    .await
    .map(|value| value.0)
}

#[tokio::test]
async fn independent_sites_share_complete_assets_and_keep_or_overwrite_conflicts() {
    let (source, source_session, _source_root) = site().await;
    let (destination, destination_session, _destination_root) = site().await;
    let image_id = new_id();
    let source_image = add_image(&source, &image_id, 10).await;
    let template_id = new_id();
    let new_template_id = new_id();
    add_template(&source, &template_id, "包内版本", &source_image).await;
    add_template(&source, &new_template_id, "新增模板", &source_image).await;
    let local_image = add_image(&destination, &image_id, 220).await;
    add_template(&destination, &template_id, "本地版本", &local_image).await;
    let outside_id = new_id();
    add_template(&destination, &outside_id, "包外模板", &local_image).await;
    sqlx::query(
        "INSERT INTO gallery_likes (template_id,actor_key,created_at) VALUES (?,'local',?)",
    )
    .bind(&template_id)
    .bind(now_rfc3339())
    .execute(&destination.db)
    .await
    .unwrap();
    let filter = GalleryExportFilter {
        categories: vec!["海报".into()],
        ..Default::default()
    };
    let preview = preview_gallery_export(
        State(source.clone()),
        source_session.clone(),
        Json(filter.clone()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!((preview.template_count, preview.asset_count), (2, 1));
    let response = export_filtered_archive(State(source), source_session, Json(filter))
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let kept = import_bytes(
        destination.clone(),
        destination_session.clone(),
        &bytes,
        GalleryImportConflict::KeepLocal,
    )
    .await
    .unwrap();
    assert_eq!(
        (kept.added_template_count, kept.skipped_template_count),
        (1, 1)
    );
    let local = get_template_for_admin(&destination, &template_id)
        .await
        .unwrap();
    assert_eq!(local.title, "本地版本");
    assert_eq!(local.preview_assets[0].sha256, local_image.sha256);
    let imported = get_template_for_admin(&destination, &new_template_id)
        .await
        .unwrap();
    assert_ne!(imported.preview_assets[0].id, image_id);
    assert_eq!(imported.preview_assets[0].sha256, source_image.sha256);
    assert_eq!(imported.tags, ["海报/商业", "用途/分享"]);
    let repeated = import_bytes(
        destination.clone(),
        destination_session.clone(),
        &bytes,
        GalleryImportConflict::KeepLocal,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            repeated.imported_template_count,
            repeated.imported_asset_count,
            repeated.skipped_template_count
        ),
        (0, 0, 2)
    );
    let overwritten = import_bytes(
        destination.clone(),
        destination_session,
        &bytes,
        GalleryImportConflict::Overwrite,
    )
    .await
    .unwrap();
    assert_eq!(overwritten.overwritten_template_count, 2);
    let changed = get_template_for_admin(&destination, &template_id)
        .await
        .unwrap();
    assert_eq!(changed.title, "包内版本");
    assert_eq!(changed.like_count, 1);
    assert_eq!(changed.preview_assets[0].sha256, source_image.sha256);
    let outside = get_template_for_admin(&destination, &outside_id)
        .await
        .unwrap();
    assert_eq!(outside.preview_assets[0].sha256, local_image.sha256);
    for template in [changed, outside] {
        let asset = &template.preview_assets[0];
        let key =
            sqlx::query_scalar::<_, String>("SELECT object_key FROM gallery_assets WHERE id=?")
                .bind(&asset.id)
                .fetch_one(&destination.db)
                .await
                .unwrap();
        assert_eq!(
            hex_sha256(
                &get_object_bytes(&destination, &key, asset.byte_len)
                    .await
                    .unwrap()
            ),
            asset.sha256
        );
    }
}

#[tokio::test]
async fn transfer_endpoints_reject_non_approved_admins() {
    let (state, _, _root) = site().await;
    for (role, status) in [
        ("user", "approved"),
        ("admin", "pending"),
        ("admin", "disabled"),
    ] {
        sqlx::query("UPDATE users SET role='user'")
            .execute(&state.db)
            .await
            .unwrap();
        let id = new_id();
        insert_test_user(&state.db, &id, &id, "unused", role, status, 0).await;
        let session = authenticated_test_session(Arc::new(MemoryStore::default()), &id, 0).await;
        assert!(
            preview_gallery_export(
                State(state.clone()),
                session.clone(),
                Json(GalleryExportFilter::default())
            )
            .await
            .is_err()
        );
        assert!(
            export_filtered_archive(
                State(state.clone()),
                session.clone(),
                Json(GalleryExportFilter::default())
            )
            .await
            .is_err()
        );
        assert!(
            import_bytes(
                state.clone(),
                session,
                b"invalid",
                GalleryImportConflict::KeepLocal
            )
            .await
            .is_err()
        );
    }
    let guest = Session::new(None, Arc::new(MemoryStore::default()), None);
    assert!(
        preview_gallery_export(State(state), guest, Json(GalleryExportFilter::default()))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn resource_reuse_quota_rollback_and_skipped_bad_hash_are_safe() {
    let (source, session, _source_root) = site().await;
    let (mut destination, destination_session, _destination_root) = site().await;
    let image = add_image(&source, &new_id(), 35).await;
    let id = new_id();
    add_template(&source, &id, "共享图片", &image).await;
    let response = export_gallery_archive(State(source), session)
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();

    Arc::get_mut(&mut destination)
        .unwrap()
        .config
        .gallery_asset_quota_bytes = 1;
    assert!(
        import_bytes(
            destination.clone(),
            destination_session.clone(),
            &bytes,
            GalleryImportConflict::Overwrite
        )
        .await
        .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM gallery_templates")
            .fetch_one(&destination.db)
            .await
            .unwrap(),
        0
    );
    Arc::get_mut(&mut destination)
        .unwrap()
        .config
        .gallery_asset_quota_bytes = 1_000_000;
    import_bytes(
        destination.clone(),
        destination_session.clone(),
        &bytes,
        GalleryImportConflict::Overwrite,
    )
    .await
    .unwrap();
    let key_before =
        sqlx::query_scalar::<_, String>("SELECT object_key FROM gallery_assets WHERE id=?")
            .bind(&image.id)
            .fetch_one(&destination.db)
            .await
            .unwrap();
    import_bytes(
        destination.clone(),
        destination_session.clone(),
        &bytes,
        GalleryImportConflict::Overwrite,
    )
    .await
    .unwrap();
    let key_after =
        sqlx::query_scalar::<_, String>("SELECT object_key FROM gallery_assets WHERE id=?")
            .bind(&image.id)
            .fetch_one(&destination.db)
            .await
            .unwrap();
    assert_eq!(key_before, key_after);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM gallery_assets")
            .fetch_one(&destination.db)
            .await
            .unwrap(),
        1
    );

    // 已存在的模板也必须验证原包，不能因为 keep_local 而接受损坏图片。
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents).unwrap();
        if entry.name().ends_with(".webp") {
            contents[0] ^= 1;
        }
        writer
            .start_file(entry.name(), SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, &contents).unwrap();
    }
    let damaged = writer.finish().unwrap().into_inner();
    assert!(
        import_bytes(
            destination.clone(),
            destination_session,
            &damaged,
            GalleryImportConflict::KeepLocal
        )
        .await
        .is_err()
    );
    assert_eq!(
        get_template_for_admin(&destination, &id)
            .await
            .unwrap()
            .title,
        "共享图片"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM gallery_staged_objects")
            .fetch_one(&destination.db)
            .await
            .unwrap(),
        0
    );
}
