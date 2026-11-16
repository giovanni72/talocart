use super::*;
use talocart_platform::identity::IdentityError;
use talocart_platform::media::library::{Filter, LibraryError, MediaLibrary};

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn trash_purge_is_immediate_confirmed_and_scoped_to_its_owner() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
        let bytes = include_bytes!("../fixtures/avatar.png").to_vec();
        let shared = lib
            .upload(&s, &s.csrf, None, bytes.clone(), "shared.png", false)
            .await
            .unwrap();
        let private = lib
            .upload(&s, &s.csrf, None, bytes.clone(), "private.png", true)
            .await
            .unwrap();
        let active = lib
            .upload(&s, &s.csrf, None, bytes, "active.png", false)
            .await
            .unwrap();
        let shared_key = lib.asset(&s, shared, false).await.unwrap().key;
        let private_key = lib.asset(&s, private, true).await.unwrap().key;
        lib.trash(&s, &s.csrf, shared, false, false, true)
            .await
            .unwrap();
        lib.trash(&s, &s.csrf, private, false, true, true)
            .await
            .unwrap();
        identity.create_administrator(&s, &s.csrf, "private-trash@example.test",
            "correct horse battery".into(), talocart_platform::identity::Role::Viewer).await.unwrap();
        let other_raw = login_as(&identity, "private-trash@example.test").await;
        let other = identity.session(&other_raw).await.unwrap().unwrap();
        let other_id = lib.upload(&other, &other.csrf, None,
            include_bytes!("../fixtures/avatar.png").to_vec(), "other.png", true).await.unwrap();
        lib.trash(&other, &other.csrf, other_id, false, true, true).await.unwrap();
        assert!(matches!(
            lib.purge(&s, &s.csrf, false, false).await,
            Err(LibraryError::Confirmation)
        ));
        assert!(matches!(
            lib.purge(&s, "invalid", false, true).await,
            Err(LibraryError::Identity(_))
        ));
        assert!(lib.asset(&s, shared, false).await.unwrap().trashed);
        // A business reference added after trashing must still prevent permanent removal.
        let product: i64 = sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,image_key,price_cents) VALUES('Protected','TRASH','trash-protected',$1,0) RETURNING id")
            .bind(&shared_key).fetch_one(&pool).await.unwrap();
        assert_eq!(lib.purge(&s, &s.csrf, false, true).await.unwrap(), 0);
        sqlx::query("DELETE FROM catalog_products WHERE id=$1").bind(product).execute(&pool).await.unwrap();
        assert_eq!(
            lib.purge(&s, &s.csrf, false, true).await.unwrap(),
            1,
            "A newly trashed source must be purgeable without backdating its timestamp"
        );
        assert!(lib.asset(&s, shared, false).await.is_err());
        assert!(lib.asset(&s, private, true).await.unwrap().trashed);
        assert!(!lib.asset(&s, active, false).await.unwrap().trashed);
        assert_eq!(lib.purge(&s, &s.csrf, false, true).await.unwrap(), 0);
        assert_eq!(lib.purge(&s, &s.csrf, true, true).await.unwrap(), 1);
        assert!(lib.asset(&other, other_id, true).await.unwrap().trashed,
            "Emptying one private archive must preserve another owner's trash");
        assert_eq!(lib.purge(&other, &other.csrf, true, true).await.unwrap(), 1);
        media.discover_files(&pool).await.unwrap();
        assert!(
            lib.asset(&s, shared, false).await.is_err(),
            "Discovery must not restore a purged source while physical cleanup is pending"
        );
        completed_prune(&media, &pool, false).await;
        assert!(media.read_registered(&pool, &shared_key).await.is_err());
        assert!(media.read_registered(&pool, &private_key).await.is_err());
        media.discover_files(&pool).await.unwrap();
        assert!(
            lib.asset(&s, shared, false).await.is_err(),
            "Purged sources must not be rediscovered"
        );
        assert!(lib.asset(&s, active, false).await.is_ok());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn trash_purge_http_requires_confirmation_csrf_and_permission_in_both_languages() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
        let app = router(
            test_app_config(
                pool.clone(),
                "Trash".into(),
                talocart_web::AppStateConfig {
                    media,
                    ..Default::default()
                },
            )
            .await,
        );
        let cookie = format!("talocart_session={raw}");
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "trash-viewer@example.test",
                "correct horse battery".into(),
                talocart_platform::identity::Role::Viewer,
            )
            .await
            .unwrap();
        let viewer_raw = login_as(&identity, "trash-viewer@example.test").await;
        let viewer = identity.session(&viewer_raw).await.unwrap().unwrap();
        assert_eq!(
            request(&app, "GET", "/admin/media/purge", &cookie, "", false)
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let anonymous = request(
            &app,
            "POST",
            "/admin/media/purge",
            "",
            "confirm=true",
            false,
        )
        .await;
        assert_eq!(anonymous.headers()[header::LOCATION], "/admin/login");
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/media/purge",
                &format!("talocart_session={viewer_raw}"),
                &format!("csrf={}&confirm=true", viewer.csrf),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        for locale in ["en", "it"] {
            sqlx::query("UPDATE administrators SET locale=$1 WHERE id=$2")
                .bind(locale)
                .bind(s.administrator_id)
                .execute(&pool)
                .await
                .unwrap();
            let id = lib
                .upload(
                    &s,
                    &s.csrf,
                    None,
                    include_bytes!("../fixtures/avatar.png").to_vec(),
                    &format!("trash-{locale}.png"),
                    false,
                )
                .await
                .unwrap();
            lib.trash(&s, &s.csrf, id, false, false, true)
                .await
                .unwrap();
            let body =
                html(request(&app, "GET", "/admin/media?trash=true", &cookie, "", false).await)
                    .await;
            assert!(body.contains(if locale == "it" {
                "Confermo l’eliminazione definitiva di tutti i file nel cestino"
            } else {
                "I confirm permanent deletion of all files in the trash"
            }));
            assert!(!body.contains("24 ore") && !body.contains("24 hours"));
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/admin/media/purge",
                    &cookie,
                    "csrf=invalid&confirm=true",
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
                    "/admin/media/purge",
                    &cookie,
                    &format!("csrf={}", s.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
            assert!(lib.asset(&s, id, false).await.unwrap().trashed);
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/admin/media/purge",
                    &cookie,
                    &format!("csrf={}&confirm=true", s.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::SEE_OTHER
            );
            assert!(lib.asset(&s, id, false).await.is_err());
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn deleted_cache_falls_back_without_http_writes_and_is_rebuilt_once_in_background() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
        let mut buffer = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(600, 300)
            .write_to(&mut buffer, image::ImageFormat::Png)
            .unwrap();
        let original = buffer.into_inner();
        let id = lib
            .upload(&s, &s.csrf, None, original.clone(), "repair.png", false)
            .await
            .unwrap();
        let asset = lib.asset(&s, id, false).await.unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        let before = media
            .rendition_url(&pool, id, "card")
            .await
            .unwrap()
            .unwrap();
        let key: String = sqlx::query_scalar(
            "SELECT object_key FROM media_renditions WHERE source_key=$1 AND profile='card'",
        )
        .bind(&asset.key)
        .fetch_one(&pool)
        .await
        .unwrap();
        let path = media.registered_path(&pool, &key).await.unwrap();
        tokio::fs::remove_dir_all(path.parent().unwrap())
            .await
            .unwrap();
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
        let response = image_request(&app, &before, "", "GET", "").await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let fallback = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let response = image_request(&app, &fallback, "", "GET", "").await;
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(
            to_bytes(response.into_body(), 32 * 1024 * 1024)
                .await
                .unwrap()
                .as_ref(),
            original
        );
        let (first, second) = tokio::join!(
            image_request(&app, &before, "", "HEAD", "W/\"media-old\""),
            image_request(&app, &before, "", "GET", "*")
        );
        assert_eq!(first.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(second.status(), StatusCode::TEMPORARY_REDIRECT);
        for _ in 0..20 {
            assert_eq!(
                image_request(&app, &before, "", "HEAD", "").await.status(),
                StatusCode::TEMPORARY_REDIRECT
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM media_cache_jobs WHERE source_key=$1"
            )
            .bind(&asset.key)
            .fetch_one(&pool)
            .await
            .unwrap(),
            "complete",
            "GET/HEAD only notify memory; background owns the persistent repair"
        );
        assert!(media.process_cache_job(&pool).await.unwrap());
        assert!(
            !media.process_cache_job(&pool).await.unwrap(),
            "Repeated requests produce one job"
        );
        let after = media
            .rendition_url(&pool, id, "card")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(before, after);
        let retired = image_request(&app, &before, "", "GET", "*").await;
        assert_eq!(retired.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(retired.headers()[header::LOCATION], after);
        assert_eq!(retired.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            !media.process_cache_job(&pool).await.unwrap(),
            "A stale URL does not rebuild a healthy current revision"
        );
        let response = image_request(&app, &after, "", "GET", "").await;
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(
            media.read_registered(&pool, &asset.key).await.unwrap(),
            original
        );
        assert_eq!(media.cache_report(&pool).await.unwrap().complete, 1);
    })
    .await;
}
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn cache_repair_prefers_surviving_derivatives_and_does_not_reset_running_jobs() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
        let mut buffer=std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1200,600).write_to(&mut buffer,image::ImageFormat::Png).unwrap();
        let original=buffer.into_inner();
        let id = lib.upload(&s,&s.csrf,None,original.clone(),"partial.png",false).await.unwrap();
        let asset = lib.asset(&s,id,false).await.unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        let key:String=sqlx::query_scalar("SELECT object_key FROM media_renditions WHERE source_key=$1 AND profile='card'").bind(&asset.key).fetch_one(&pool).await.unwrap();
        tokio::fs::remove_file(media.registered_path(&pool,&key).await.unwrap()).await.unwrap();
        sqlx::query("UPDATE media_cache_jobs SET status='running',attempts=2,lease_until=now()+interval '5 minutes' WHERE source_key=$1").bind(&asset.key).execute(&pool).await.unwrap();
        let before:String=sqlx::query_scalar("SELECT row_to_json(j)::text FROM media_cache_jobs j WHERE source_key=$1").bind(&asset.key).fetch_one(&pool).await.unwrap();
        let fallback=media.rendition_url(&pool,id,"card").await.unwrap().unwrap();
        assert!(fallback.contains("/detail."),"Use a surviving resized copy before the source");
        assert!(!media.process_cache_job(&pool).await.unwrap());
        assert_eq!(before,sqlx::query_scalar::<_,String>("SELECT row_to_json(j)::text FROM media_cache_jobs j WHERE source_key=$1").bind(&asset.key).fetch_one(&pool).await.unwrap());
        sqlx::query("UPDATE media_cache_jobs SET lease_until=now()-interval '1 second' WHERE source_key=$1").bind(&asset.key).execute(&pool).await.unwrap();
        let (first,second)=tokio::join!(media.process_cache_job(&pool),media.process_cache_job(&pool));
        assert_ne!(first.unwrap(),second.unwrap(),"Only one worker owns the recovery");
        assert_eq!(sqlx::query_scalar::<_,i32>("SELECT attempts FROM media_cache_jobs WHERE source_key=$1").bind(&asset.key).fetch_one(&pool).await.unwrap(),3);
        let repaired=media.rendition_url(&pool,id,"card").await.unwrap().unwrap();
        assert!(repaired.contains("/card."));
        assert_ne!(fallback,repaired);
        assert_eq!(media.read_registered(&pool,&asset.key).await.unwrap(),original);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn private_cache_repair_requires_authorization_and_missing_sources_do_not_loop() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(),identity.clone(),media.clone());
        let mut buffer = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(600,300).write_to(&mut buffer,image::ImageFormat::Png).unwrap();
        let original = buffer.into_inner();
        let id=lib.upload(&s,&s.csrf,None,original.clone(),"personal.png",true).await.unwrap();
        let asset=lib.asset(&s,id,true).await.unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        let (object_id,key):(i64,String)=sqlx::query_as("SELECT m.id,m.key FROM media_renditions r JOIN media_objects m ON m.key=r.object_key WHERE r.source_key=$1 AND r.profile='card'").bind(&asset.key).fetch_one(&pool).await.unwrap();
        let path=media.registered_path(&pool,&key).await.unwrap();
        tokio::fs::remove_dir_all(path.parent().unwrap()).await.unwrap();
        let app=router(test_app_config(pool.clone(),"Shop".into(), talocart_web::AppStateConfig { media: media.clone(), ..Default::default() }).await);
        let forbidden=image_request(&app,&format!("/media/public/{object_id}/{key}/original"),"","GET","").await;
        assert_eq!(forbidden.headers()[header::CONTENT_TYPE],"image/svg+xml");
        assert!(!media.process_cache_job(&pool).await.unwrap(),"Unauthorized reads cannot schedule private work");
        let response=image_request(&app,&format!("/admin/media/{id}/preview"),&cookie,"GET","").await;
        assert_eq!(response.headers()[header::CONTENT_TYPE],"image/png");
        assert_eq!(response.headers()[header::CACHE_CONTROL],"private, no-cache");
        assert_eq!(response.headers()["x-media-ready"],"false");
        assert_eq!(to_bytes(response.into_body(),32*1024*1024).await.unwrap().as_ref(),original);
        assert!(media.process_cache_job(&pool).await.unwrap());
        let response=image_request(&app,&format!("/admin/media/{id}/preview"),&cookie,"HEAD","").await;
        assert_eq!(response.headers()[header::CONTENT_TYPE],"image/jpeg");
        assert_eq!(response.headers()["x-media-ready"],"true");
        let etag=response.headers()[header::ETAG].to_str().unwrap().to_owned();
        let key:String=sqlx::query_scalar("SELECT object_key FROM media_renditions WHERE source_key=$1 AND profile='card'").bind(&asset.key).fetch_one(&pool).await.unwrap();
        let path=media.registered_path(&pool,&key).await.unwrap();
        tokio::fs::remove_dir_all(path.parent().unwrap()).await.unwrap();
        let source_path=media.registered_path(&pool,&asset.key).await.unwrap();
        tokio::fs::remove_file(&source_path).await.unwrap();
        let response=image_request(&app,&format!("/admin/media/{id}/preview"),&cookie,"GET",&etag).await;
        assert_eq!(response.status(),StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");
        assert_eq!(response.headers()[header::CONTENT_TYPE],"image/svg+xml");
        assert!(media.process_cache_job(&pool).await.unwrap());
        for _ in 0..3 {
            image_request(&app,&format!("/admin/media/{id}/preview"),&cookie,"HEAD","").await;
            assert!(!media.process_cache_job(&pool).await.unwrap());
        }
        assert_eq!(sqlx::query_scalar::<_,i32>("SELECT attempts FROM media_cache_jobs WHERE source_key=$1 AND status='failed'").bind(&asset.key).fetch_one(&pool).await.unwrap(),1);
        tokio::fs::write(source_path,&original).await.unwrap();
        media.queue_all_cache(&pool,false).await.unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap(),"Explicit retry works after restoring the original");
        assert_eq!(media.read_registered(&pool,&asset.key).await.unwrap(),original);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn library_sources_survive_unlink_move_collision_trash_and_private_isolation() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let token = authenticated(&identity).await;
        let session = identity.session(&token).await.unwrap().unwrap();
        let store = test_media(&pool).await;
        let library = MediaLibrary::new(pool.clone(), identity.clone(), store.clone());
        let folder = library
            .create_folder(&session, &session.csrf, None, "Estate", false)
            .await
            .unwrap();
        let raw = include_bytes!("../fixtures/webp-lossless.webp").to_vec();
        let id = library
            .upload(
                &session,
                &session.csrf,
                Some(folder),
                raw.clone(),
                "foto.webp",
                false,
            )
            .await
            .unwrap();
        let asset = library.asset(&session, id, false).await.unwrap();
        assert_eq!(store.read_registered(&pool, &asset.key).await.unwrap(), raw);
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '7 days'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(completed_prune(&store, &pool, false).await.objects, 0);
        let target = library
            .create_folder(&session, &session.csrf, None, "Inverno", false)
            .await
            .unwrap();
        library
            .move_asset(
                &session,
                &session.csrf,
                id,
                "Inverno",
                "nuovo.webp",
                false,
            )
            .await
            .unwrap();
        assert_eq!(
            library.asset(&session, id, false).await.unwrap().key,
            asset.key
        );
        assert!(store.root().join("Inverno/nuovo.webp").exists());
        assert_eq!(store.read_registered(&pool, &asset.key).await.unwrap(), raw);
        let duplicate = library
            .upload(
                &session,
                &session.csrf,
                Some(target),
                raw.clone(),
                "other.webp",
                false,
            )
            .await
            .unwrap();
        assert!(matches!(
            library
                .move_asset(
                    &session,
                    &session.csrf,
                    duplicate,
                    "Inverno",
                    "nuovo.webp",
                    false
                )
                .await,
            Err(LibraryError::Collision)
        ));
        assert!(matches!(
            library
                .delete_folder(&session, &session.csrf, target, false, true)
                .await,
            Err(LibraryError::InUse)
        ));
        let product:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,image_key,price_cents) VALUES('Prodotto','PHOTO','photo',$1,0) RETURNING id").bind(&asset.key).fetch_one(&pool).await.unwrap();
        assert!(matches!(
            library
                .trash(&session, &session.csrf, id, false, false, true)
                .await,
            Err(LibraryError::InUse)
        ));
        sqlx::query("UPDATE catalog_products SET image_key=NULL WHERE id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(completed_prune(&store, &pool, false).await.objects, 0);
        library
            .trash(&session, &session.csrf, id, false, false, true)
            .await
            .unwrap();
        assert!(library.asset(&session, id, false).await.unwrap().trashed);
        assert!(matches!(
            library.purge(&session, &session.csrf, false, false).await,
            Err(LibraryError::Confirmation)
        ));
        library
            .trash(&session, &session.csrf, id, true, false, false)
            .await
            .unwrap();
        library
            .move_folder(&session, &session.csrf, target, None, "Autunno", false)
            .await
            .unwrap();
        assert_eq!(store.read_registered(&pool, &asset.key).await.unwrap(), raw);
        assert!(store.root().join("Autunno/nuovo.webp").exists());
        assert!(matches!(
            library
                .move_folder(&session, &session.csrf, target, Some(target), "x", false)
                .await,
            Err(LibraryError::Destination)
        ));
        let private = library
            .upload(
                &session,
                &session.csrf,
                None,
                raw.clone(),
                "avatar.webp",
                true,
            )
            .await
            .unwrap();
        assert!(
            library
                .asset(&session, private, true)
                .await
                .unwrap()
                .owner_id
                .is_some()
        );
        assert!(
            library
                .assets(&session, &Filter::default())
                .await
                .unwrap()
                .iter()
                .all(|a| a.id != private)
        );
        let private_key = library.asset(&session, private, true).await.unwrap().key;
        assert!(
            store
                .public_image(
                    &pool,
                    private,
                    &private_key,
                    talocart_platform::media::ImageVariant::Original
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            library
                .create_folder(&session, "bad", None, "Denied", false)
                .await,
            Err(LibraryError::Identity(_))
        ));
        store.check(&pool).await.unwrap();
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn missing_profile_never_pins_a_mutable_fallback_as_immutable() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let media = test_media(&pool).await;
        let prepared = media
            .prepare_product(include_bytes!("../fixtures/logo.png").to_vec())
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        talocart_platform::media::MediaStore::publish_lock(&mut tx)
            .await
            .unwrap();
        let stored = media.put_prepared(prepared, "fallback.png").await.unwrap();
        stored.register(&mut tx, true).await.unwrap();
        sqlx::query("DELETE FROM media_variants WHERE source_key=$1")
            .bind(&stored.key)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let metadata = media.metadata(&pool, &stored.key).await.unwrap().unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let url = format!("/media/public/{}/{}/card", metadata.id, metadata.key);
        let response = request(&app, "GET", &url, "", "", false).await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .ends_with("/original")
        );
        drop(identity);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn shared_selection_is_atomic_and_projections_leave_sources_intact() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
        let image = lib
            .upload(&s, &s.csrf, None, large_image_fixture(), "photo.png", false)
            .await
            .unwrap();
        let source = lib.asset(&s, image, false).await.unwrap();
        let original = media.read_registered(&pool, &source.key).await.unwrap();
        let ids:Vec<i64>=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('A','A','a',0),('B','B','b',0) RETURNING id").fetch_all(&pool).await.unwrap();
        let products =
            talocart_commerce::products::Products::new(pool.clone(), identity.clone(), media.clone());
        for id in &ids {
            products
                .select_image(&s, &s.csrf, *id, image, false)
                .await
                .unwrap();
        }
        assert_eq!(lib.asset(&s, image, false).await.unwrap().uses, 2);
        assert!(
            products
                .select_gallery_images(&s, &s.csrf, ids[0], &[image, i64::MAX])
                .await
                .is_err()
        );
        assert!(products.gallery(&s, ids[0]).await.unwrap().is_empty());
        assert!(
            products
                .select_gallery_images(&s, &s.csrf, ids[0], &[image, image])
                .await
                .is_err()
        );
        products
            .select_gallery_images(&s, &s.csrf, ids[0], &[image])
            .await
            .unwrap();
        assert!(
            products
                .select_gallery_images(&s, &s.csrf, ids[0], &[image])
                .await
                .is_err()
        );
        identity
            .select_branding(
                &s,
                &s.csrf,
                talocart_platform::identity::BrandingKind::Favicon,
                image,
            )
            .await
            .unwrap();
        let icon = identity
            .branding_metadata(talocart_platform::identity::BrandingKind::Favicon)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((icon.width, icon.height), (64, 64));
        assert_ne!(icon.key, source.key);
        identity
            .select_branding(
                &s,
                &s.csrf,
                talocart_platform::identity::BrandingKind::Logo,
                image,
            )
            .await
            .unwrap();
        identity.select_avatar(&s, &s.csrf, image).await.unwrap();
        let avatar = identity
            .avatar_image(&s, talocart_platform::media::ImageVariant::Original)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((avatar.metadata.width, avatar.metadata.height), (256, 256));
        assert!(
            media
                .public_image(
                    &pool,
                    avatar.metadata.id,
                    &avatar.metadata.key,
                    talocart_platform::media::ImageVariant::Original
                )
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            media.read_registered(&pool, &source.key).await.unwrap(),
            original
        );
        assert_eq!(lib.asset(&s, image, false).await.unwrap().uses, 6);
        assert!(matches!(
            lib.trash(&s, &s.csrf, image, false, false, true).await,
            Err(LibraryError::InUse)
        ));
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_session={raw}");
        let html = html(
            request(
                &app,
                "GET",
                &format!("/admin/media?target=product:{}", ids[0]),
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(html.contains("Seleziona photo.png"));
        assert!(html.contains("File da caricare"));
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/media/select",
                &cookie,
                &format!("csrf=bad&target=product:{}&ids={image}", ids[0]),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/media?target=https://example.test",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn private_sources_are_owned_and_media_permissions_are_rechecked() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let owner_raw = authenticated(&identity).await;
        let owner = identity.session(&owner_raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "viewer@example.test",
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
                "viewer@example.test",
                "another correct password".into(),
            )
            .await
            .unwrap();
        let viewer = identity.session(&raw).await.unwrap().unwrap();
        let public_id = lib
            .upload(
                &owner,
                &owner.csrf,
                None,
                include_bytes!("../fixtures/avatar.png").to_vec(),
                "shared.png",
                false,
            )
            .await
            .unwrap();
        assert!(matches!(
            lib.asset(&viewer, public_id, true).await,
            Err(LibraryError::NotFound)
        ));
        let id = lib
            .upload(
                &viewer,
                &viewer.csrf,
                None,
                include_bytes!("../fixtures/avatar.png").to_vec(),
                "own.png",
                true,
            )
            .await
            .unwrap();
        for (account, personal, expected) in [
            (&owner, false, vec![public_id]),
            (&owner, true, vec![]),
            (&viewer, true, vec![id]),
        ] {
            let filter = Filter {
                personal,
                ..Default::default()
            };
            let counted = lib.assets_counted(account, &filter).await.unwrap();
            assert_eq!(counted.total, expected.len() as i64);
            assert_eq!(
                counted.entries.iter().map(|a| a.id).collect::<Vec<_>>(),
                expected
            );
            assert_eq!(
                lib.assets(account, &filter)
                    .await
                    .unwrap()
                    .iter()
                    .map(|a| a.id)
                    .collect::<Vec<_>>(),
                expected
            );
        }
        assert!(matches!(
            lib.asset(&viewer, id, false).await,
            Err(LibraryError::NotFound)
        ));
        assert!(matches!(
            lib.asset(&owner, id, false).await,
            Err(LibraryError::NotFound)
        ));
        assert!(matches!(
            lib.create_folder(&viewer, &viewer.csrf, None, "No", false)
                .await,
            Err(LibraryError::Identity(IdentityError::Forbidden))
        ));
        identity
            .select_avatar(&viewer, &viewer.csrf, id)
            .await
            .unwrap();
        assert!(
            identity
                .select_avatar(&owner, &owner.csrf, id)
                .await
                .is_err()
        );
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let owner_cookie = format!("talocart_session={owner_raw}");
        let viewer_cookie = format!("talocart_session={raw}");
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("/admin/media/{public_id}?personal=true&target=avatar"),
                &viewer_cookie,
                "",
                false,
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        for endpoint in ["preview", "download"] {
            assert_eq!(
                request(
                    &app,
                    "GET",
                    &format!("/admin/media/{id}/{endpoint}"),
                    &viewer_cookie,
                    "",
                    false,
                )
                .await
                .status(),
                StatusCode::OK
            );
        }
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("/admin/media/{id}/preview"),
                &owner_cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("/admin/media/{id}?personal=true"),
                &viewer_cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::OK
        );
        identity.logout(&viewer, &viewer.csrf).await.unwrap();
        assert!(
            lib.upload(
                &viewer,
                &viewer.csrf,
                None,
                include_bytes!("../fixtures/avatar.png").to_vec(),
                "revoked.png",
                true
            )
            .await
            .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn cache_versions_formats_head_conditional_get_and_repair_preserve_sources() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
        let mut buffer = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2800, 1400)
            .write_to(&mut buffer, image::ImageFormat::Png)
            .unwrap();
        let source = buffer.into_inner();
        let id = lib
            .upload(&s, &s.csrf, None, source.clone(), "large.png", false)
            .await
            .unwrap();
        let asset = lib.asset(&s, id, false).await.unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        let first = media
            .rendition_url(&pool, id, "detail")
            .await
            .unwrap()
            .unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let response = image_request(&app, &first, "", "GET", "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
        let etag = response.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .to_owned();
        let body = to_bytes(response.into_body(), 32 * 1024 * 1024)
            .await
            .unwrap();
        let decoded = image::load_from_memory(&body).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1024, 512));
        assert_eq!(
            image_request(&app, &first, "", "GET", &etag).await.status(),
            StatusCode::NOT_MODIFIED
        );
        assert_eq!(
            image_request(&app, &first, "", "HEAD", "").await.headers()[header::CONTENT_LENGTH],
            body.len().to_string()
        );
        let large = media
            .rendition_url(&pool, id, "large")
            .await
            .unwrap()
            .unwrap();
        let body = to_bytes(
            image_request(&app, &large, "", "GET", "").await.into_body(),
            32 * 1024 * 1024,
        )
        .await
        .unwrap();
        assert_eq!(image::load_from_memory(&body).unwrap().width(), 2048);
        media.queue_cache(&pool, &s, &s.csrf, true).await.unwrap();
        assert_eq!(
            media.read_registered(&pool, &asset.key).await.unwrap(),
            source
        );
        assert!(media.process_cache_job(&pool).await.unwrap());
        let second = media
            .rendition_url(&pool, id, "detail")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(
            image_request(&app, &first, "", "GET", "").await.status(),
            StatusCode::OK
        );
        let transparent = lib
            .upload(
                &s,
                &s.csrf,
                None,
                include_bytes!("../fixtures/webp-lossless.webp").to_vec(),
                "alpha.webp",
                false,
            )
            .await
            .unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        let url = media
            .rendition_url(&pool, transparent, "large")
            .await
            .unwrap()
            .unwrap();
        let response = image_request(&app, &url, "", "GET", "").await;
        assert!(matches!(
            response.headers()[header::CONTENT_TYPE].to_str().unwrap(),
            "image/webp" | "image/png"
        ));
        let pixels = image::load_from_memory(
            &to_bytes(response.into_body(), 32 * 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap()
        .to_rgba8();
        assert_eq!(pixels.dimensions(), (512, 128));
        assert_eq!(pixels.get_pixel(0, 0).0[3], 0);
        assert_eq!(pixels.get_pixel(256, 64).0[3], 128);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let key = first
                .trim_start_matches("/media/public/")
                .split_once('/')
                .unwrap()
                .1
                .trim_end_matches("/original");
            let path = media.registered_path(&pool, key).await.unwrap();
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
                .await
                .unwrap();
            assert!(media.read_registered(&pool, key).await.is_err());
            assert_eq!(
                image_request(&app, &first, "", "HEAD", "").await.status(),
                StatusCode::OK
            );
            assert_eq!(
                image_request(&app, &first, "", "GET", &etag).await.status(),
                StatusCode::NOT_MODIFIED
            );
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .await
                .unwrap();
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn explicit_acquisition_is_safe_idempotent_and_dry_run_never_registers() {
    isolated(|pool, _| async move {
        let _identity = installed(&pool).await;
        let media = test_media(&pool).await;
        let dir = media.root().join("library/Resident");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let bytes = include_bytes!("../fixtures/avatar.png");
        tokio::fs::write(dir.join("resident.png"), bytes)
            .await
            .unwrap();
        tokio::fs::write(dir.join("fake.png"), b"not image")
            .await
            .unwrap();
        let report = media.acquire(&pool, "library", true).await.unwrap();
        assert_eq!((report.acquired, report.rejected), (1, 1));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_library")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        let report = media.acquire(&pool, "library", false).await.unwrap();
        assert_eq!(report.acquired, 1);
        assert_eq!(
            tokio::fs::read(dir.join("resident.png")).await.unwrap(),
            bytes
        );
        assert_eq!(
            media
                .acquire(&pool, "library", false)
                .await
                .unwrap()
                .existing,
            1
        );
        assert!(media.acquire(&pool, "objects", false).await.is_err());
        let staging = media.root().join("staging");
        tokio::fs::create_dir_all(&staging).await.unwrap();
        tokio::fs::write(staging.join("unpublished.png"), bytes)
            .await
            .unwrap();
        assert!(matches!(
            media.acquire(&pool, "staging", false).await,
            Err(talocart_platform::media::MediaError::Invalid)
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::env::temp_dir(), dir.join("escape")).unwrap();
            assert!(
                media
                    .acquire(&pool, "library", true)
                    .await
                    .unwrap()
                    .rejected
                    >= 2
            );
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn library_pagination_permissions_collisions_recovery_and_purge_cover_boundaries() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
        let folder = lib
            .create_folder(&s, &s.csrf, None, "Paged", false)
            .await
            .unwrap();
        assert!(matches!(
            lib.create_folder(&s, &s.csrf, None, "paged", false).await,
            Err(LibraryError::Collision)
        ));
        let dir = media.root().join("Paged");
        let bytes = include_bytes!("../fixtures/avatar.png");
        for index in 0..43 {
            tokio::fs::write(dir.join(format!("photo-{index:02}.png")), bytes)
                .await
                .unwrap();
        }
        assert_eq!(
            media.acquire(&pool, "Paged", false).await.unwrap().acquired,
            43
        );
        let first = lib
            .assets(
                &s,
                &Filter {
                    folder: Some(folder),
                    page: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let second = lib
            .assets(
                &s,
                &Filter {
                    folder: Some(folder),
                    page: 2,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!((first.len(), second.len()), (41, 3));
        assert_eq!(first[40].id, second[0].id);
        assert_eq!(
            lib.assets(
                &s,
                &Filter {
                    query: "photo-4".into(),
                    format: "png".into(),
                    usage: "unused".into(),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .len(),
            3
        );
        let id = first[0].id;
        let key = first[0].key.clone();
        // Simulate cancellation after hardlink publication but before database commit. Retrying recovers it.
        std::fs::hard_link(dir.join("photo-00.png"), dir.join("recovered.png")).unwrap();
        lib.move_asset(&s, &s.csrf, id, "Paged", "recovered.png", false)
            .await
            .unwrap();
        assert_eq!(lib.asset(&s, id, false).await.unwrap().key, key);
        let child = lib
            .create_folder(&s, &s.csrf, Some(folder), "Child", false)
            .await
            .unwrap();
        lib.delete_folder(&s, &s.csrf, child, false, true)
            .await
            .unwrap();
        assert!(!dir.join("Child").exists());
        lib.trash(&s, &s.csrf, id, false, false, true)
            .await
            .unwrap();
        assert_eq!(lib.purge(&s, &s.csrf, false, true).await.unwrap(), 1);
        assert_eq!(lib.purge(&s, &s.csrf, false, true).await.unwrap(), 0);
        completed_prune(&media, &pool, false).await;
        assert!(lib.asset(&s, id, false).await.is_err());
        assert!(media.read_registered(&pool, &key).await.is_err());
        assert_eq!(
            lib.assets(
                &s,
                &Filter {
                    folder: Some(folder),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .len(),
            41
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn shared_media_list_does_not_rescan_usage_totals_per_file() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        sqlx::raw_sql("INSERT INTO media_objects(key,content_type,byte_size,width,height) SELECT lpad(to_hex(n),64,'0')||'.png','image/png',100,10,10 FROM generate_series(1,9600) n;
            INSERT INTO media_library(key,folder_id,name) SELECT key,(SELECT id FROM media_folders WHERE path='library'),'asset-'||id FROM media_objects;
            INSERT INTO catalog_products(name,model,slug,image_key,price_cents) SELECT 'Product '||id,'PLAN-'||id,'plan-'||id,key,0 FROM media_objects;
            UPDATE media_library SET owner_id=(SELECT id FROM administrators LIMIT 1) WHERE key=lpad(to_hex(1),64,'0')||'.png';
")
            .execute(&pool).await.unwrap();
        // No ANALYZE: imports can be read before autovacuum refreshes statistics.
        // Inspect the actual owner query, as in benchmark-site.cjs, without exporting
        // an application SQL API solely for tests. Check work, not machine timings.
        let source = include_str!("../../../platform/src/media/library.rs");
        let columns = source.split_once("const ASSET_LIST_SELECT: &str = \"").unwrap().1.split_once('"').unwrap().0;
        let predicate = source.split_once("\"{ASSET_LIST_SELECT} WHERE ").unwrap().1.split_once('"').unwrap().0;
        let base = format!("{columns} WHERE {predicate}");
        for query in [format!("SELECT count(*) FROM ({base}) counted"), format!("{base} ORDER BY lower(l.name),m.id LIMIT 40")] {
            let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("EXPLAIN (ANALYZE, FORMAT JSON) {query}")))
                .bind(None::<i64>).bind(None::<i64>).bind("").bind("").bind(false).bind("").bind(false)
                .fetch_one(&pool).await.unwrap();
            fn check(node: &serde_json::Value) {
                assert!(node["Rows Removed by Join Filter"].as_u64().unwrap_or(0) <= 20_000,
                    "The library must not compare every file against every usage total");
                if node["CTE Name"] == "usage_totals" {
                    assert!(node["Actual Loops"].as_u64().unwrap() <= 1,
                        "Usage totals must not be scanned once per library file: {node}");
                }
                if let Some(children) = node["Plans"].as_array() {
                    for child in children { check(child); }
                }
            }
            check(&plan[0]["Plan"]);
        }
        let library = MediaLibrary::new(pool.clone(), identity, test_media(&pool).await);
        for _ in 0..8 {
            let page = library.assets_counted(&session, &Filter::default()).await.unwrap();
            assert_eq!(page.total, 9599);
            assert_eq!(page.entries.len(), 40);
            assert_eq!(library.assets(&session, &Filter::default()).await.unwrap().len(), 41);
        }
        // PostgreSQL may switch cached statements to a generic plan after five
        // executions. Neither list nor count may leave such a statement cached.
        let mut connections = Vec::new();
        for _ in 0..pool.size() {
            connections.push(pool.acquire().await.unwrap());
        }
        for connection in &mut connections {
            let cached: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_prepared_statements WHERE statement NOT LIKE 'EXPLAIN%' AND statement NOT LIKE '%pg_prepared_statements%' AND statement LIKE '%usage_totals AS MATERIALIZED%'")
                .fetch_one(&mut **connection).await.unwrap();
            assert_eq!(cached, 0, "Media lists must plan for the current filter values");
        }
    }).await;
}
