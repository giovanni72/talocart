use super::*;
use talocart_platform::media::{MediaStore, library::MediaLibrary};

async fn setup(
    pool: &PgPool,
) -> (
    Identity,
    talocart_platform::identity::Session,
    MediaStore,
    i64,
    String,
) {
    let identity = installed(pool).await;
    let token = authenticated(&identity).await;
    let session = identity.session(&token).await.unwrap().unwrap();
    let media = test_media(pool).await;
    let identity = identity.with_media(media.clone());
    let library = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
    let mut raw = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(600, 300)
        .write_to(&mut raw, image::ImageFormat::Png)
        .unwrap();
    let id = library
        .upload(
            &session,
            &session.csrf,
            None,
            raw.into_inner(),
            "audit.png",
            false,
        )
        .await
        .unwrap();
    let key = library.asset(&session, id, false).await.unwrap().key;
    assert!(media.process_cache_job(pool).await.unwrap());
    (identity, session, media, id, key)
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn truncated_cache_falls_back_and_repairs() {
    isolated(|pool, _| async move {
        let (_, _, media, id, source) = setup(&pool).await;
        let key: String = sqlx::query_scalar(
            "SELECT object_key FROM media_renditions WHERE source_key=$1 AND profile='card'",
        )
        .bind(&source)
        .fetch_one(&pool)
        .await
        .unwrap();
        tokio::fs::write(
            media.registered_path(&pool, &key).await.unwrap(),
            b"truncated",
        )
        .await
        .unwrap();
        assert!(
            media
                .rendition_url(&pool, id, "card")
                .await
                .unwrap()
                .is_some()
        );
        assert!(media.process_cache_job(&pool).await.unwrap());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn public_profile_serves_without_redirect_and_revalidates_current_representation() {
    isolated(|pool, _| async move {
        let (_, _, media, id, _) = setup(&pool).await;
        let app = router(
            test_app_config(
                pool.clone(),
                "Shop".into(),
                talocart_web::AppStateConfig {
                    media: media.clone(),
                    ..Default::default()
                },
            )
            .await,
        );
        let url = format!("/media/image/{id}/thumbnail");
        let response = image_request(&app, &url, "", "GET", "").await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "A storefront image needs only one HTTP request"
        );
        assert!(!response.headers().contains_key(header::LOCATION));
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=0, must-revalidate"
        );
        let etag = response.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .to_owned();
        let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024)
            .await
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (128, 64));
        let head = image_request(&app, &url, "", "HEAD", "").await;
        assert_eq!(
            head.headers()[header::CONTENT_LENGTH],
            bytes.len().to_string()
        );
        assert!(to_bytes(head.into_body(), 1024).await.unwrap().is_empty());
        for method in ["GET", "HEAD"] {
            let cached = image_request(&app, &url, "", method, &etag).await;
            assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
            assert_eq!(cached.headers()[header::ETAG], etag);
            assert!(to_bytes(cached.into_body(), 1024).await.unwrap().is_empty());
        }
        media.queue_all_cache(&pool, false).await.unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        let rebuilt = image_request(&app, &url, "", "GET", &etag).await;
        assert_eq!(rebuilt.status(), StatusCode::OK);
        assert_ne!(rebuilt.headers()[header::ETAG], etag);
        // Authorization/publication must run before conditional cache matching.
        let etag = rebuilt.headers()[header::ETAG].to_str().unwrap().to_owned();
        sqlx::query("UPDATE media_objects SET public=false WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let hidden = image_request(&app, &url, "", "GET", &etag).await;
        assert_eq!(hidden.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!hidden.headers().contains_key(header::ETAG));
        assert_eq!(hidden.headers()[header::CONTENT_TYPE], "image/svg+xml");
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn regeneration_starts_a_new_retirement_grace() {
    isolated(|pool, _| async move {
        let (_, _, media, id, source) = setup(&pool).await;
        let old = media.rendition_url(&pool, id, "card").await.unwrap().unwrap();
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '2 days' WHERE key IN (SELECT object_key FROM media_renditions WHERE source_key=$1)").bind(&source).execute(&pool).await.unwrap();
        assert_eq!(completed_prune(&media, &pool, true).await.objects, 0);
        media.queue_all_cache(&pool, false).await.unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        assert_eq!(completed_prune(&media, &pool, false).await.objects, 0);
        let app = router(test_app_config(pool.clone(), "Shop".into(), talocart_web::AppStateConfig { media, ..Default::default() }).await);
        let response = image_request(&app, &old, "", "GET", "").await;
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn explicit_rebuild_preserves_running_lease() {
    isolated(|pool, _| async move {
        let (_, s, media, _, key) = setup(&pool).await;
        sqlx::query("UPDATE media_cache_jobs SET status='running',attempts=2,lease_until=now()+interval '5 minutes' WHERE source_key=$1").bind(&key).execute(&pool).await.unwrap();
        let before:String=sqlx::query_scalar("SELECT row_to_json(j)::text FROM media_cache_jobs j WHERE source_key=$1").bind(&key).fetch_one(&pool).await.unwrap();
        let mappings:Vec<(String,String)>=sqlx::query_as("SELECT profile,object_key FROM media_renditions WHERE source_key=$1 ORDER BY profile").bind(&key).fetch_all(&pool).await.unwrap();
        for action in [talocart_platform::media::CacheAction::Repair,talocart_platform::media::CacheAction::Rebuild,talocart_platform::media::CacheAction::Clear] {
            media.queue_cache_action(&pool,&s,&s.csrf,action).await.unwrap();
            assert_eq!(before,sqlx::query_scalar::<_,String>("SELECT row_to_json(j)::text FROM media_cache_jobs j WHERE source_key=$1").bind(&key).fetch_one(&pool).await.unwrap());
            assert_eq!(mappings,sqlx::query_as::<_,(String,String)>("SELECT profile,object_key FROM media_renditions WHERE source_key=$1 ORDER BY profile").bind(&key).fetch_all(&pool).await.unwrap());
        }
        let row: (String, i32, bool) = sqlx::query_as("SELECT status,attempts,lease_until IS NULL FROM media_cache_jobs WHERE source_key=$1").bind(&key).fetch_one(&pool).await.unwrap();
        assert_eq!(row, ("running".into(), 2, false));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn missing_logo_transform_recovers_without_changing_references() {
    isolated(|pool, _| async move {
        let (identity, s, media, id, _) = setup(&pool).await;
        identity
            .select_branding(
                &s,
                &s.csrf,
                talocart_platform::identity::BrandingKind::Logo,
                id,
            )
            .await
            .unwrap();
        let key: String = sqlx::query_scalar("SELECT logo_key FROM store_settings WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let path = media.registered_path(&pool, &key).await.unwrap();
        tokio::fs::remove_file(&path).await.unwrap();
        // Reselecting also repairs immediately inside the explicit mutation, without reusing missing bytes.
        identity
            .select_branding(
                &s,
                &s.csrf,
                talocart_platform::identity::BrandingKind::Logo,
                id,
            )
            .await
            .unwrap();
        let recovered = media
            .image(
                &pool,
                &key,
                talocart_platform::media::ImageVariant::Original,
            )
            .await
            .unwrap()
            .unwrap();
        assert_ne!(recovered.metadata.key, key);
        tokio::fs::remove_file(
            media
                .registered_path(&pool, &recovered.metadata.key)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        media.queue_all_cache(&pool, true).await.unwrap();
        while media.process_cache_job(&pool).await.unwrap() {}
        identity
            .select_branding(
                &s,
                &s.csrf,
                talocart_platform::identity::BrandingKind::Logo,
                id,
            )
            .await
            .unwrap();
        assert!(
            media
                .image(
                    &pool,
                    &key,
                    talocart_platform::media::ImageVariant::Original
                )
                .await
                .unwrap()
                .is_some()
        );
        let current: String = sqlx::query_scalar("SELECT logo_key FROM store_settings WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(current, key);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn legacy_direct_url_falls_back_to_healthy_current_cache() {
    isolated(|pool, _| async move {
        let (_, _, media, _, _) = setup(&pool).await;
        let mut raw = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(600, 300)
            .write_to(&mut raw, image::ImageFormat::Png)
            .unwrap();
        let prepared = media.prepare_png(raw.into_inner()).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        MediaStore::publish_lock(&mut tx).await.unwrap();
        let stored = media
            .put_prepared(prepared, "legacy-audit.png")
            .await
            .unwrap();
        stored.register(&mut tx, true).await.unwrap();
        tx.commit().await.unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        let id: i64 = sqlx::query_scalar("SELECT id FROM media_objects WHERE key=$1")
            .bind(&stored.key)
            .fetch_one(&pool)
            .await
            .unwrap();
        let key: String = sqlx::query_scalar(
            "SELECT object_key FROM media_variants WHERE source_key=$1 AND profile='card'",
        )
        .bind(&stored.key)
        .fetch_one(&pool)
        .await
        .unwrap();
        tokio::fs::remove_file(media.registered_path(&pool, &key).await.unwrap())
            .await
            .unwrap();
        assert!(
            media
                .rendition_url(&pool, id, "card")
                .await
                .unwrap()
                .is_some()
        );
        let app = router(
            test_app_config(
                pool.clone(),
                "Shop".into(),
                talocart_web::AppStateConfig {
                    media,
                    ..Default::default()
                },
            )
            .await,
        );
        let response = image_request(
            &app,
            &format!("/media/public/{id}/{}/card", stored.key),
            "",
            "GET",
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn repair_preserves_healthy_versions_and_small_sources_share_one_file() {
    isolated(|pool,_|async move {
        let (identity,s,media,_,_)=setup(&pool).await;
        let lib=MediaLibrary::new(pool.clone(),identity,media.clone());
        let source=include_bytes!("../fixtures/avatar.png").to_vec();
        let id=lib.upload(&s,&s.csrf,None,source.clone(),"small.png",false).await.unwrap();
        let key=lib.asset(&s,id,false).await.unwrap().key;
        assert!(media.process_cache_job(&pool).await.unwrap());
        let count:(i64,i64)=sqlx::query_as("SELECT count(*),count(DISTINCT object_key) FROM media_renditions WHERE source_key=$1").bind(&key).fetch_one(&pool).await.unwrap();
        assert_eq!(count,(4,1));
        let before:Vec<(String,String,String)>=sqlx::query_as("SELECT source_key,profile,object_key FROM media_renditions ORDER BY source_key,profile").fetch_all(&pool).await.unwrap();
        media.queue_cache_action(&pool,&s,&s.csrf,talocart_platform::media::CacheAction::Repair).await.unwrap();
        while media.process_cache_job(&pool).await.unwrap() {}
        assert_eq!(before,sqlx::query_as::<_,(String,String,String)>("SELECT source_key,profile,object_key FROM media_renditions ORDER BY source_key,profile").fetch_all(&pool).await.unwrap());
        assert_eq!(source,media.read_registered(&pool,&key).await.unwrap());
        assert!(media.cache_report(&pool).await.unwrap().failed==0);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn an_expired_worker_cannot_publish_or_complete_a_new_claim() {
    isolated(|pool, _| async move {
        let (_, _, media, _, key) = setup(&pool).await;
        media.queue_all_cache(&pool, false).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_objects")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mut blocker = pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(0x4b4f414c414d4544_i64)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let p = pool.clone();
        let m = media.clone();
        let first = tokio::spawn(async move { m.process_cache_job(&p).await.unwrap() });
        let first_token = wait_claim(&pool, &key, None).await;
        sqlx::query(
            "UPDATE media_cache_jobs SET lease_until=now()-interval '1 second' WHERE source_key=$1",
        )
        .bind(&key)
        .execute(&pool)
        .await
        .unwrap();
        let p = pool.clone();
        let m = media.clone();
        let second = tokio::spawn(async move { m.process_cache_job(&p).await.unwrap() });
        let second_token = wait_claim(&pool, &key, Some(&first_token)).await;
        assert_ne!(first_token, second_token);
        blocker.commit().await.unwrap();
        assert!(!first.await.unwrap(), "stale result is discarded");
        assert!(second.await.unwrap());
        let state: (String, i32, bool) = sqlx::query_as(
            "SELECT status,attempts,lease_token IS NULL FROM media_cache_jobs WHERE source_key=$1",
        )
        .bind(&key)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, ("complete".into(), 2, true));
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM media_objects")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            after - count,
            3,
            "only one generation of 128,512,600px files"
        );
    })
    .await;
}
async fn wait_claim(pool: &PgPool, key: &str, old: Option<&str>) -> String {
    for _ in 0..200 {
        let token: Option<String> =
            sqlx::query_scalar("SELECT lease_token FROM media_cache_jobs WHERE source_key=$1")
                .bind(key)
                .fetch_one(pool)
                .await
                .unwrap();
        if let Some(token) = token
            && Some(token.as_str()) != old
        {
            return token;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("worker did not claim the job");
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn special_profiles_recover_privately_with_their_exact_geometry() {
    isolated(|pool, _| async move {
        let (identity, s, media, id, source) = setup(&pool).await;
        identity.select_avatar(&s, &s.csrf, id).await.unwrap();
        identity
            .select_branding(
                &s,
                &s.csrf,
                talocart_platform::identity::BrandingKind::Favicon,
                id,
            )
            .await
            .unwrap();
        let references: Vec<(String, String)> = sqlx::query_as(
            "SELECT kind,object_key FROM media_transforms WHERE source_key=$1 ORDER BY kind",
        )
        .bind(&source)
        .fetch_all(&pool)
        .await
        .unwrap();
        for (_, key) in &references {
            tokio::fs::remove_file(media.registered_path(&pool, key).await.unwrap())
                .await
                .unwrap();
        }
        assert!(
            identity
                .avatar_image(&s, talocart_platform::media::ImageVariant::Original)
                .await
                .unwrap()
                .is_none()
        );
        // A private avatar from a public source must never fall back to its public uncropped bytes.
        assert!(
            media
                .rendition_url(&pool, id, "avatar")
                .await
                .unwrap()
                .is_none()
        );
        assert!(media.process_cache_job(&pool).await.unwrap());
        let avatar = identity
            .avatar_image(&s, talocart_platform::media::ImageVariant::Original)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((avatar.metadata.width, avatar.metadata.height), (256, 256));
        let thumb = identity
            .avatar_image(&s, talocart_platform::media::ImageVariant::Thumbnail)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((thumb.metadata.width, thumb.metadata.height), (128, 128));
        tokio::fs::remove_file(
            media
                .registered_path(&pool, &thumb.metadata.key)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert!(
            identity
                .avatar_image(&s, talocart_platform::media::ImageVariant::Thumbnail)
                .await
                .unwrap()
                .is_some()
        );
        assert!(media.process_cache_job(&pool).await.unwrap());
        let thumb = identity
            .avatar_image(&s, talocart_platform::media::ImageVariant::Thumbnail)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((thumb.metadata.width, thumb.metadata.height), (128, 128));
        media.check(&pool).await.unwrap();
        assert_eq!(
            media.cache_report(&pool).await.unwrap().failed,
            0,
            "repair must finish, not only make bytes readable"
        );

        let app = router(
            test_app_config(
                pool.clone(),
                "Shop".into(),
                talocart_web::AppStateConfig {
                    media: media.clone(),
                    ..Default::default()
                },
            )
            .await,
        );
        let response = image_request(
            &app,
            &format!(
                "/media/public/{}/{}/original",
                avatar.metadata.id, avatar.metadata.key
            ),
            "",
            "GET",
            "",
        )
        .await;
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
        let favicon = media
            .image(
                &pool,
                &references.iter().find(|r| r.0 == "favicon").unwrap().1,
                talocart_platform::media::ImageVariant::Original,
            )
            .await
            .unwrap()
            .unwrap();
        let pixels = image::load_from_memory(&favicon.png).unwrap().to_rgba8();
        assert_eq!(pixels.dimensions(), (64, 64));
        assert_eq!(pixels.get_pixel(0, 0).0[3], 0);
        assert_eq!(
            references,
            sqlx::query_as::<_, (String, String)>(
                "SELECT kind,object_key FROM media_transforms WHERE source_key=$1 ORDER BY kind"
            )
            .bind(&source)
            .fetch_all(&pool)
            .await
            .unwrap()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn maintenance_http_requires_permission_csrf_confirmation_and_known_action() {
    isolated(|pool, _| async move {
        let (identity, _, media, _, source) = setup(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(
            test_app_config(
                pool.clone(),
                "Shop".into(),
                talocart_web::AppStateConfig {
                    media: media.clone(),
                    ..Default::default()
                },
            )
            .await,
        );
        for (body, status) in [
            (
                "csrf=wrong&confirm=true&action=repair".into(),
                StatusCode::FORBIDDEN,
            ),
            (
                format!("csrf={}&action=repair", s.csrf),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                format!("csrf={}&confirm=true&action=unknown", s.csrf),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            assert_eq!(
                request(&app, "POST", "/admin/media/cache", &cookie, &body, false)
                    .await
                    .status(),
                status
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM media_cache_jobs WHERE source_key=$1"
            )
            .bind(&source)
            .fetch_one(&pool)
            .await
            .unwrap(),
            "complete"
        );
        let body = format!("csrf={}&confirm=true&action=repair", s.csrf);
        assert_eq!(
            request(&app, "POST", "/admin/media/cache", &cookie, &body, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert!(media.process_cache_job(&pool).await.unwrap());
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "audit-viewer@example.test",
                "another correct password".into(),
                talocart_platform::identity::Role::Viewer,
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anon,
                &anon.csrf,
                "audit-viewer@example.test",
                "another correct password".into(),
            )
            .await
            .unwrap();
        let viewer = identity.session(&raw).await.unwrap().unwrap();
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/media/cache",
                &format!("talocart_session={raw}"),
                &format!("csrf={}&confirm=true&action=repair", viewer.csrf),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        identity.logout(&s, &s.csrf).await.unwrap();
        assert!(
            media
                .queue_cache_action(
                    &pool,
                    &s,
                    &s.csrf,
                    talocart_platform::media::CacheAction::Repair
                )
                .await
                .is_err()
        );
    })
    .await;
}
