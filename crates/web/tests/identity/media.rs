use super::*;
use talocart_platform::identity::IdentityError;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn branding_http_files_permissions_validation_replace_remove_and_restart() {
    use talocart_platform::identity::{BRANDING_UPLOAD_LIMIT, BrandingKind};
    isolated(|pool, options| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let session = service.session(&raw).await.unwrap().unwrap();
        let auth = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let before = service.settings().await.unwrap().unwrap().values;
        let media = test_media(&pool).await;
        let png = include_bytes!("../fixtures/avatar.png");
        for key in ["logo", "favicon"] {
            let path = format!("/branding/{key}");
            let response = request(&app, "GET", &path, "", "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(
                branding_upload_request(&app, "", Some(&session.csrf), key, png, false)
                    .await
                    .status(),
                StatusCode::SEE_OTHER
            );
            for csrf in [None, Some("wrong")] {
                assert_eq!(
                    branding_upload_request(&app, &auth, csrf, key, png, false)
                        .await
                        .status(),
                    StatusCode::FORBIDDEN
                );
            }
            assert_eq!(
                branding_upload_request(&app, &auth, Some(&session.csrf), key, png, true)
                    .await
                    .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
            let response =
                branding_upload_request(&app, &auth, Some(&session.csrf), key, png, false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let redirect = request(&app, "GET", &path, "", "", false).await;
            assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT);
            let response = request(
                &app,
                "GET",
                redirect.headers()[header::LOCATION].to_str().unwrap(),
                "",
                "",
                false,
            )
            .await;
            assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            let saved = to_bytes(response.into_body(), 32 * 1024 * 1024)
                .await
                .unwrap();
            assert!(saved.starts_with(b"\x89PNG"));
            let query = if key == "logo" {
                "SELECT logo_key FROM store_settings"
            } else {
                "SELECT favicon_key FROM store_settings"
            };
            let file_key: String = sqlx::query_scalar(query).fetch_one(&pool).await.unwrap();
            assert_eq!(media.read(&file_key).await.unwrap(), saved);
            for bad in [b"<svg onload='alert(1)'/>".as_slice(), &png[..25]] {
                let response =
                    branding_upload_request(&app, &auth, Some(&session.csrf), key, bad, false)
                        .await;
                assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let html = html(response).await;
                assert!(html.contains(&format!("id=\"error-{key}\"")));
                assert!(html.contains("aria-invalid=\"true\""));
                assert_eq!(
                    sqlx::query_scalar::<_, String>(query)
                        .fetch_one(&pool)
                        .await
                        .unwrap(),
                    file_key
                );
            }
            let oversized = vec![0; BRANDING_UPLOAD_LIMIT + 32 * 1024];
            let response =
                branding_upload_request(&app, &auth, Some(&session.csrf), key, &oversized, false)
                    .await;
            assert!(matches!(
                response.status(),
                StatusCode::PAYLOAD_TOO_LARGE | StatusCode::UNPROCESSABLE_ENTITY
            ));
            assert_eq!(
                sqlx::query_scalar::<_, String>(query)
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                file_key
            );
            assert_eq!(
                branding_upload_request(&app, &auth, Some(&session.csrf), key, png, false)
                    .await
                    .status(),
                StatusCode::SEE_OTHER
            );
            let replaced: String = sqlx::query_scalar(query).fetch_one(&pool).await.unwrap();
            assert_ne!(file_key, replaced);
        }
        assert!(service.settings().await.unwrap().unwrap().values == before);
        let profile: bool =
            sqlx::query_scalar("SELECT avatar_key IS NULL FROM administrators WHERE id=$1")
                .bind(session.administrator_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(profile);
        for page in ["/", "/admin", "/admin/login"] {
            let page = html(
                request(
                    &app,
                    "GET",
                    page,
                    if page == "/admin/login" { "" } else { &auth },
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(page.contains("src=\"/branding/logo?variant=card\""));
            assert!(page.contains("href=\"/branding/favicon\""));
        }
        let reader = service
            .create_administrator(
                &session,
                &session.csrf,
                "reader@example.test",
                "correct horse battery".into(),
                talocart_platform::identity::Role::Viewer,
            )
            .await
            .unwrap();
        let reader_raw = login_as(&service, "reader@example.test").await;
        let reader_session = service.session(&reader_raw).await.unwrap().unwrap();
        let reader_auth = format!("talocart_session={reader_raw}");
        let page = html(
            request(
                &app,
                "GET",
                "/admin/settings/appearance",
                &reader_auth,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(page.contains("sola lettura"));
        assert!(!page.contains("type=\"file\""));
        assert!(!page.contains("Rimuovi logo"));
        assert_eq!(
            branding_upload_request(
                &app,
                &reader_auth,
                Some(&reader_session.csrf),
                "logo",
                png,
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/branding/logo/delete",
                &reader_auth,
                &format!("csrf={}", reader_session.csrf),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert!(reader > 0);
        let reopened = PgPoolOptions::new().connect_with(options).await.unwrap();
        let restarted = test_identity(reopened.clone()).await;
        for kind in [BrandingKind::Logo, BrandingKind::Favicon] {
            assert_eq!(
                restarted.branding_image(kind).await.unwrap(),
                service.branding_image(kind).await.unwrap()
            );
            let delete = format!("/admin/settings/branding/{}/delete", kind.key());
            assert_eq!(
                request(&app, "GET", &delete, &auth, "", false)
                    .await
                    .status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
            assert_eq!(
                request(&app, "POST", &delete, &auth, "csrf=wrong", false)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &delete,
                    &auth,
                    &format!("csrf={}", session.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::SEE_OTHER
            );
            assert!(service.branding_image(kind).await.unwrap().is_none());
            assert_eq!(
                request(
                    &app,
                    "GET",
                    &format!("/branding/{}", kind.key()),
                    "",
                    "",
                    false
                )
                .await
                .headers()[header::CONTENT_TYPE],
                "image/svg+xml"
            );
        }
        assert_eq!(
            request(&app, "GET", "/branding/../../koala.toml", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(&app, "GET", "/branding/avatar", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        media.check(&pool).await.unwrap();
        reopened.close().await;
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn filesystem_failures_keep_existing_references_and_missing_files_are_reported() {
    use talocart_platform::{identity::BrandingKind, media::MediaStore};
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let session = service.session(&raw).await.unwrap().unwrap();
        let png = include_bytes!("../fixtures/avatar.png").to_vec();
        let root = test_media(&pool).await.root().to_owned();
        tokio::fs::create_dir_all(&root).await.unwrap();
        let blocked = root.join("not-directory");
        tokio::fs::write(&blocked, b"blocked").await.unwrap();
        let broken = Identity::new(pool.clone()).with_media(MediaStore::new(blocked));
        assert!(matches!(
            broken
                .update_branding(
                    &session,
                    &session.csrf,
                    BrandingKind::Logo,
                    Some(png.clone())
                )
                .await,
            Err(IdentityError::Storage)
        ));
        assert!(!service.branding().await.unwrap().unwrap().has_logo);
        service
            .update_avatar(&session, &session.csrf, Some(png.clone()))
            .await
            .unwrap();
        let before_avatar = service.avatar(&session).await.unwrap().unwrap();
        let version = service.account(&session).await.unwrap().avatar_version;
        assert!(matches!(
            broken
                .update_avatar(&session, &session.csrf, Some(png.clone()))
                .await,
            Err(IdentityError::Storage)
        ));
        assert_eq!(service.avatar(&session).await.unwrap(), Some(before_avatar));
        assert_eq!(
            service.account(&session).await.unwrap().avatar_version,
            version
        );
        service
            .update_branding(&session, &session.csrf, BrandingKind::Logo, Some(png))
            .await
            .unwrap();
        let key: String = sqlx::query_scalar("SELECT logo_key FROM store_settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        tokio::fs::remove_file(root.join("objects").join(key))
            .await
            .unwrap();
        assert!(test_media(&pool).await.check(&pool).await.is_err());
        assert!(
            service
                .branding_image(BrandingKind::Logo)
                .await
                .unwrap()
                .is_none()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn concurrent_branding_mutations_and_revoked_session_obey_contract() {
    use talocart_platform::identity::BrandingKind;
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let session = service.session(&raw).await.unwrap().unwrap();
        let png = include_bytes!("../fixtures/avatar.png").to_vec();
        let (a, b) = tokio::join!(
            service.update_branding(
                &session,
                &session.csrf,
                BrandingKind::Logo,
                Some(png.clone())
            ),
            service.update_branding(
                &session,
                &session.csrf,
                BrandingKind::Favicon,
                Some(png.clone())
            )
        );
        a.unwrap();
        b.unwrap();
        let (a, b) = tokio::join!(
            service.update_branding(
                &session,
                &session.csrf,
                BrandingKind::Logo,
                Some(png.clone())
            ),
            service.update_branding(&session, &session.csrf, BrandingKind::Logo, None)
        );
        a.unwrap();
        b.unwrap();
        let branding = service.branding().await.unwrap().unwrap();
        assert_eq!(branding.logo_version, 3);
        assert_eq!(branding.favicon_version, 1);
        assert!(branding.has_favicon);
        test_media(&pool).await.check(&pool).await.unwrap();
        service.logout(&session, &session.csrf).await.unwrap();
        for kind in [BrandingKind::Logo, BrandingKind::Favicon] {
            assert!(matches!(
                service
                    .update_branding(&session, &session.csrf, kind, Some(png.clone()))
                    .await,
                Err(IdentityError::Forbidden)
            ));
            assert!(matches!(
                service
                    .update_branding(&session, &session.csrf, kind, None)
                    .await,
                Err(IdentityError::Forbidden)
            ));
        }
        assert_eq!(service.branding().await.unwrap().unwrap().logo_version, 3);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn image_variants_versioned_cache_private_auth_and_missing_fallback() {
    use talocart_platform::identity::BrandingKind;
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let session = service.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        service
            .update_branding_named(
                &session,
                &session.csrf,
                BrandingKind::Logo,
                Some(large_image_fixture()),
                "../../shop logo.jpg",
            )
            .await
            .unwrap();
        let metadata = service
            .branding_metadata(BrandingKind::Logo)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(metadata.key, "shop-logo.png");
        let original = format!("/media/public/{}/{}/original", metadata.id, metadata.key);
        let mut original_etag = String::new();
        for (profile, width, height) in [
            ("original", 1600, 800),
            ("thumbnail", 128, 64),
            ("card", 512, 256),
            ("detail", 1024, 512),
        ] {
            let path = format!("/media/public/{}/{}/{profile}", metadata.id, metadata.key);
            let response = image_request(&app, &path, "", "GET", "").await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "public, max-age=0, must-revalidate"
            );
            let etag = response.headers()[header::ETAG]
                .to_str()
                .unwrap()
                .to_owned();
            if profile == "original" {
                original_etag = etag.clone();
            }
            let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024)
                .await
                .unwrap();
            let image = image::load_from_memory(&bytes).unwrap();
            assert_eq!((image.width(), image.height()), (width, height));
            for conditional in [etag.clone(), format!("\"other\", W/{etag}"), "*".into()] {
                let response = image_request(&app, &path, "", "GET", &conditional).await;
                assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
                assert!(
                    to_bytes(response.into_body(), 100)
                        .await
                        .unwrap()
                        .is_empty()
                );
            }
            let response = image_request(&app, &path, "", "HEAD", "").await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CONTENT_LENGTH],
                bytes.len().to_string()
            );
            assert!(
                to_bytes(response.into_body(), 100)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(
            image_request(&app, "/branding/logo?variant=arbitrary", "", "GET", "")
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        service
            .update_branding_named(
                &session,
                &session.csrf,
                BrandingKind::Logo,
                Some(large_image_fixture()),
                "shop logo.jpg",
            )
            .await
            .unwrap();
        let replacement = service
            .branding_metadata(BrandingKind::Logo)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replacement.key, "shop-logo-2.png");
        assert_ne!(metadata.id, replacement.id);
        // The old immutable URL remains valid until deferred cleanup.
        assert_eq!(
            image_request(&app, &original, "", "GET", &original_etag)
                .await
                .status(),
            StatusCode::NOT_MODIFIED
        );
        let missing = format!(
            "/media/public/{}/{}/original",
            replacement.id, replacement.key
        );
        tokio::fs::remove_file(
            test_media(&pool)
                .await
                .root()
                .join("objects")
                .join(&replacement.key),
        )
        .await
        .unwrap();
        let response = image_request(&app, &missing, "", "GET", "*").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert!(!response.headers().contains_key(header::ETAG));
        service
            .update_avatar_named(
                &session,
                &session.csrf,
                Some(avatar_fixture()),
                "portrait.jpg",
            )
            .await
            .unwrap();
        let response = image_request(
            &app,
            "/admin/account/avatar?variant=thumbnail",
            &cookie,
            "GET",
            "",
        )
        .await;
        let etag = response.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-cache"
        );
        let bytes = to_bytes(response.into_body(), 600_000).await.unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 128);
        assert_eq!(
            image_request(
                &app,
                "/admin/account/avatar?variant=thumbnail",
                &cookie,
                "GET",
                &etag
            )
            .await
            .status(),
            StatusCode::NOT_MODIFIED
        );
        let (id, key): (i64, String) = sqlx::query_as(
            "SELECT m.id,m.key FROM media_objects m JOIN administrators a ON a.avatar_key=m.key",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let response = image_request(
            &app,
            &format!("/media/public/{id}/{key}/original"),
            "",
            "GET",
            "",
        )
        .await;
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
        service.logout(&session, &session.csrf).await.unwrap();
        assert_eq!(
            image_request(
                &app,
                "/admin/account/avatar?variant=thumbnail",
                &cookie,
                "GET",
                &etag
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn media_cleanup_grace_live_variants_orphans_and_publication_lock() {
    use talocart_platform::{identity::BrandingKind, media::MediaStore};
    isolated(|pool,_|async move {
        let service=installed(&pool).await;
        let raw=authenticated(&service).await;
        let session=service.session(&raw).await.unwrap().unwrap();
        let media=test_media(&pool).await;
        service.update_branding_named(&session,&session.csrf,BrandingKind::Logo,Some(large_image_fixture()),"logo.jpg").await.unwrap();
        let first=service.branding_metadata(BrandingKind::Logo).await.unwrap().unwrap();
        let keys:Vec<String>=sqlx::query_scalar("SELECT key FROM media_objects").fetch_all(&pool).await.unwrap();
        assert_eq!(keys.len(),4);
        sqlx::query("UPDATE media_objects SET created_at=now()-interval '7 days',unused_since=now()-interval '7 days'").execute(&pool).await.unwrap();
        // Even stale timestamps cannot remove live originals or their variants.
        assert_eq!(completed_prune(&media,&pool,false).await.objects,0);
        service.update_branding_named(&session,&session.csrf,BrandingKind::Logo,Some(large_image_fixture()),"new.png").await.unwrap();
        assert_eq!(completed_prune(&media,&pool,false).await.objects,0,"Library retains replaced sources independently of timestamps");
        assert_eq!(completed_prune(&media,&pool,false).await.objects,0);
        purge_library_sources(&pool,&session,std::slice::from_ref(&first.key)).await;
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '25 hours' WHERE key=ANY($1)").bind(&keys).execute(&pool).await.unwrap();
        let mut tx=pool.begin().await.unwrap();MediaStore::publish_lock(&mut tx).await.unwrap();
        assert!(media.prune(&pool,false).await.unwrap().busy);tx.rollback().await.unwrap();
        let directory=media.root().join("objects");
        let old=std::time::SystemTime::now()-std::time::Duration::from_secs(90000);
        for name in ["orphan.png","foreign.txt",".upload-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.pending"] {
            tokio::fs::write(directory.join(name),b"orphan").await.unwrap();
            // Test fixture only; production filesystem operations remain asynchronous.
            std::fs::File::open(directory.join(name)).unwrap().set_times(std::fs::FileTimes::new().set_modified(old)).unwrap();
        }
        tokio::fs::write(directory.join("recent.png"),b"recent").await.unwrap();
        let dry=completed_prune(&media,&pool,true).await;assert_eq!(dry.objects,4);assert_eq!(dry.files,5);
        assert!(directory.join(&first.key).exists());
        let report=completed_prune(&media,&pool,false).await;assert_eq!(report.objects,4);assert_eq!(report.files,5);
        for key in &keys {assert!(!directory.join(key).exists());}
        assert!(directory.join("recent.png").exists());assert!(directory.join("foreign.txt").exists());assert!(directory.join("orphan.png").exists());
        media.check(&pool).await.unwrap();
        assert_eq!(completed_prune(&media,&pool,false).await.objects,0);
        service.update_branding_named(&session,&session.csrf,BrandingKind::Logo,Some(large_image_fixture()),"logo.jpg").await.unwrap();
        let reused=service.branding_metadata(BrandingKind::Logo).await.unwrap().unwrap();
        assert_eq!(reused.key,first.key);assert_ne!(reused.id,first.id);
        assert!(service.public_image(first.id,&first.key,talocart_platform::media::ImageVariant::Original).await.unwrap().is_none());
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn header_shows_logo_or_store_name_exclusively() {
    use talocart_platform::identity::BrandingKind;
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let session = service.session(&raw).await.unwrap().unwrap();
        let auth = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let name = service.settings().await.unwrap().unwrap().values.name;
        for present in [false, true, false] {
            service
                .update_branding(
                    &session,
                    &session.csrf,
                    BrandingKind::Logo,
                    if present {
                        Some(large_image_fixture())
                    } else {
                        None
                    },
                )
                .await
                .unwrap();
            for path in [
                "/",
                "/admin",
                "/admin/settings",
                "/admin/account",
                "/admin/users",
                "/admin/roles",
                "/admin/settings/appearance",
                "/admin/login",
                "/status",
                "/admin/status",
            ] {
                let page = html(request(&app, "GET", path, &auth, "", false).await).await;
                let start = page.find("class=\"wordmark\"").expect("Header brand link");
                let brand = page[start..].split("</a>").next().unwrap();
                let content = brand.split_once('>').unwrap().1;
                assert_eq!(content.contains("<img"), present, "{path}");
                if present {
                    assert!(
                        !content.contains(&name),
                        "Visible duplicate store name: {path}"
                    );
                    assert!(
                        brand.contains("aria-label="),
                        "Accessible logo-only link: {path}"
                    );
                } else {
                    assert_eq!(content.trim(), name, "{path}");
                }
                if path.starts_with("/admin") && path != "/admin/login" {
                    assert!(brand.contains("href=\"/admin\""));
                }
            }
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn webp_uploads_are_static_validated_and_normalized_for_all_owners() {
    use talocart_platform::identity::BrandingKind;
    async fn upload(
        app: &Router,
        cookie: &str,
        csrf: &str,
        path: &str,
        field: &str,
        name: &str,
        data: &[u8],
    ) -> Response {
        let mut body=format!("--webp\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{csrf}\r\n--webp\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{name}\"\r\nContent-Type: image/webp\r\n\r\n").into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n--webp--\r\n");
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "multipart/form-data; boundary=webp")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let session = service.session(&raw).await.unwrap().unwrap();
        let auth = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (filename, raw) in [
            (
                "picture-lossless.webp",
                include_bytes!("../fixtures/webp-lossless.webp").as_slice(),
            ),
            (
                "picture-lossy.webp",
                include_bytes!("../fixtures/webp-lossy.webp").as_slice(),
            ),
        ] {
            for (path, field, dimensions) in [
                ("/admin/settings/branding/logo", "image", (512, 128)),
                ("/admin/settings/branding/favicon", "image", (64, 64)),
                ("/admin/account/avatar", "avatar", (256, 256)),
            ] {
                assert_eq!(
                    upload(&app, &auth, "wrong", path, field, filename, raw)
                        .await
                        .status(),
                    StatusCode::FORBIDDEN
                );
                assert_eq!(
                    upload(&app, &auth, &session.csrf, path, field, filename, raw)
                        .await
                        .status(),
                    StatusCode::SEE_OTHER
                );
                let (key, png) = if field == "avatar" {
                    (
                        sqlx::query_scalar::<_, String>(
                            "SELECT avatar_key FROM administrators WHERE id=$1",
                        )
                        .bind(session.administrator_id)
                        .fetch_one(&pool)
                        .await
                        .unwrap(),
                        service.avatar(&session).await.unwrap().unwrap(),
                    )
                } else {
                    let kind = if path.ends_with("logo") {
                        BrandingKind::Logo
                    } else {
                        BrandingKind::Favicon
                    };
                    (
                        service.branding_metadata(kind).await.unwrap().unwrap().key,
                        service.branding_image(kind).await.unwrap().unwrap(),
                    )
                };
                assert!(key.starts_with(filename.trim_end_matches(".webp")));
                assert!(key.ends_with(".png"));
                assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
                let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
                assert_eq!(decoded.dimensions(), dimensions);
                assert_eq!(
                    decoded.get_pixel(dimensions.0 / 2, dimensions.1 / 2).0[3],
                    128
                );
                for invalid in [
                    include_bytes!("../fixtures/webp-animated.webp").as_slice(),
                    &raw[..20],
                    b"<svg onload='alert(1)'/>".as_slice(),
                ] {
                    let response = upload(
                        &app,
                        &auth,
                        &session.csrf,
                        path,
                        field,
                        "fake.webp",
                        invalid,
                    )
                    .await;
                    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
                    assert!(html(response).await.contains("WebP statico"));
                }
                let remaining = if field == "avatar" {
                    service.avatar(&session).await.unwrap().unwrap()
                } else {
                    service
                        .branding_image(if path.ends_with("logo") {
                            BrandingKind::Logo
                        } else {
                            BrandingKind::Favicon
                        })
                        .await
                        .unwrap()
                        .unwrap()
                };
                assert_eq!(
                    remaining, png,
                    "Invalid uploads must preserve the previous image"
                );
                assert_eq!(test_media(&pool).await.read(&key).await.unwrap(), png);
            }
        }
        for path in ["/admin/settings/appearance", "/admin/account"] {
            let page = html(request(&app, "GET", path, &auth, "", false).await).await;
            if path == "/admin/settings/appearance" {
                assert!(page.contains("id=\"settings-editor\""));
                assert!(page.contains("value=\"browse_logo\""));
                assert!(page.contains("value=\"browse_favicon\""));
            } else {
                assert!(page.contains("media-picker-link"));
            }
            assert!(!page.contains("type=\"file\""));
        }
        test_media(&pool).await.check(&pool).await.unwrap();
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn brand_directory_pruning_preserves_live_files_and_cleans_abandoned_files() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let media = test_media(&pool).await;
        let catalog = talocart_commerce::Catalog::new(pool.clone(), identity, media.clone());
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO catalog_brands(name,slug) VALUES('Brand','brand') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        catalog
            .brand_logo(
                &session,
                &session.csrf,
                id,
                Some(large_image_fixture()),
                "logo.png",
            )
            .await
            .unwrap();
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '25 hours'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(completed_prune(&media, &pool, false).await.objects, 0);
        let directory = media.root().join("brand");
        let old = SystemTime::now() - std::time::Duration::from_secs(90000);
        for name in [
            "orphan.png",
            ".upload-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.pending",
            "foreign.txt",
        ] {
            tokio::fs::write(directory.join(name), b"synthetic")
                .await
                .unwrap();
            std::fs::File::open(directory.join(name))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        tokio::fs::write(directory.join("recent.png"), b"synthetic")
            .await
            .unwrap();
        let dry = completed_prune(&media, &pool, true).await;
        assert_eq!((dry.objects, dry.files), (0, 1));
        assert!(directory.join("orphan.png").is_file());
        let report = completed_prune(&media, &pool, false).await;
        assert_eq!((report.objects, report.files), (0, 1));
        assert!(directory.join("logo.png").is_file());
        assert!(directory.join("foreign.txt").is_file());
        assert!(directory.join("recent.png").is_file());
        catalog
            .brand_logo(&session, &session.csrf, id, None, "")
            .await
            .unwrap();
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '25 hours'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            completed_prune(&media, &pool, false).await.objects,
            0,
            "Removing the brand reference retains its source"
        );
        purge_library_sources(&pool, &session, &["brand/logo.png".into()]).await;
        let report = completed_prune(&media, &pool, false).await;
        assert_eq!((report.objects, report.files), (1, 1));
        assert!(!directory.join("logo.png").exists());
        media.check(&pool).await.unwrap();
    })
    .await;
}
