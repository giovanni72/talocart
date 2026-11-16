use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn acquirable_source_must_survive_prune() {
    isolated(|pool, _| async move {
        let _identity = installed(&pool).await;
        let media = test_media(&pool).await;
        let dir = media.root().join("incoming");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("supplier.png");
        tokio::fs::write(&path, include_bytes!("../fixtures/avatar.png"))
            .await
            .unwrap();
        let file = std::fs::File::open(&path).unwrap();
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(172800))
            .unwrap();
        drop(file);
        assert_eq!(
            media
                .acquire(&pool, "incoming", true)
                .await
                .unwrap()
                .acquired,
            1
        );
        completed_prune(&media, &pool, false).await;
        assert!(
            path.is_file(),
            "Prune deleted a valid source accepted by media-acquire dry-run"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn restore_must_preserve_running_claim() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = talocart_platform::media::library::MediaLibrary::new(pool.clone(), identity, media);
        let id = lib.upload(&s, &s.csrf, None, include_bytes!("../fixtures/avatar.png").to_vec(), "probe.png", false).await.unwrap();
        let key = lib.asset(&s,id,false).await.unwrap().key;
        lib.trash(&s,&s.csrf,id,false,false,true).await.unwrap();
        sqlx::query("UPDATE media_cache_jobs SET status='running',attempts=2,lease_until=now()+interval '5 minutes',lease_token='audit-running-claim' WHERE source_key=$1").bind(&key).execute(&pool).await.unwrap();
        lib.trash(&s,&s.csrf,id,true,false,false).await.unwrap();
        let state:(String,i32,bool,Option<String>)=sqlx::query_as("SELECT status,attempts,lease_until IS NOT NULL,lease_token FROM media_cache_jobs WHERE source_key=$1").bind(&key).fetch_one(&pool).await.unwrap();
        assert_eq!(state,("running".into(),2,true,Some("audit-running-claim".into())),"Restore reset an active job");
        for status in ["pending", "failed", "complete"] {
            sqlx::query("UPDATE media_cache_jobs SET status=$1,attempts=2,lease_until=NULL,lease_token=NULL WHERE source_key=$2").bind(status).bind(&key).execute(&pool).await.unwrap();
            lib.trash(&s,&s.csrf,id,true,false,false).await.unwrap();
            let actual:(String,i32,Option<String>)=sqlx::query_as("SELECT status,attempts,lease_token FROM media_cache_jobs WHERE source_key=$1").bind(&key).fetch_one(&pool).await.unwrap();
            assert_eq!(actual,("pending".into(),if status=="pending" {2} else {0},None));
        }

    }).await;
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn legacy_writer_must_reject_symlink_directory() {
    isolated(|pool, _| async move {
        let _identity = installed(&pool).await;
        let media = test_media(&pool).await;
        tokio::fs::create_dir_all(media.root()).await.unwrap();
        let outside_path = media.root().with_extension("audit-outside");
        tokio::fs::create_dir_all(&outside_path).await.unwrap();
        std::os::unix::fs::symlink(outside_path.as_path(), media.root().join("objects")).unwrap();
        let prepared = media
            .prepare_product(include_bytes!("../fixtures/avatar.png").to_vec())
            .await
            .unwrap();
        let result = media.put_prepared(prepared, "audit-symlink.png").await;
        let escaped = outside_path.join("audit-symlink.png").exists();
        tokio::fs::remove_dir_all(&outside_path).await.unwrap();
        assert!(
            result.is_err() && !escaped,
            "Legacy writer accepted objects symlink and wrote outside media root"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn new_sources_use_only_renditions_and_old_profile_urls_redirect_safely() {
    isolated(|pool, _| async move {
        let _identity = installed(&pool).await;
        let media = test_media(&pool).await;
        let raw = large_image_fixture();
        let photo = media.prepare_product(raw.clone()).await.unwrap();
        let logo = media.prepare_logo(raw.clone()).await.unwrap();
        let imported = media.prepare_import(raw.clone()).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        talocart_platform::media::MediaStore::publish_lock(&mut tx)
            .await
            .unwrap();
        let photo = media.put_prepared(photo, "photo.png").await.unwrap();
        let logo = media.put_brand_prepared(logo, "brand.png").await.unwrap();
        let imported = media
            .put_imported(imported, "supplier/original.png")
            .await
            .unwrap();
        for stored in [&photo, &logo, &imported] {
            stored.register(&mut tx, true).await.unwrap();
        }
        tx.commit().await.unwrap();
        assert_eq!(
            media.read_registered(&pool, &imported.key).await.unwrap(),
            raw
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_objects")
                .fetch_one(&pool)
                .await
                .unwrap(),
            3
        );
        let before: Vec<(String, String)> =
            sqlx::query_as("SELECT key,path FROM media_locations ORDER BY key")
                .fetch_all(&pool)
                .await
                .unwrap();
        while media.process_cache_job(&pool).await.unwrap() {}
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_variants")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_renditions")
                .fetch_one(&pool)
                .await
                .unwrap(),
            12
        );
        for (key, path) in before {
            assert_eq!(
                media.registered_path(&pool, &key).await.unwrap(),
                media.root().join(path)
            );
        }
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
        let meta = media.metadata(&pool, &photo.key).await.unwrap().unwrap();
        let old_url = format!("/media/public/{}/{}/thumbnail", meta.id, meta.key);
        let response = image_request(&app, &old_url, "", "GET", "").await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let response = image_request(&app, &location, "", "GET", "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=0, must-revalidate"
        );
        let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024)
            .await
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!(decoded.width().max(decoded.height()), 128);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn orphan_cleanup_removes_only_technical_files_and_preserves_registered_cache() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let token = authenticated(&identity).await;
        let s = identity.session(&token).await.unwrap().unwrap();
        let media = test_media(&pool).await;
        let lib = talocart_platform::media::library::MediaLibrary::new(
            pool.clone(),
            identity,
            media.clone(),
        );
        let id = lib
            .upload(
                &s,
                &s.csrf,
                None,
                include_bytes!("../fixtures/avatar.png").to_vec(),
                "keep.png",
                false,
            )
            .await
            .unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        let source = lib.asset(&s, id, false).await.unwrap().key;
        let locations: Vec<String> = sqlx::query_scalar("SELECT path FROM media_locations")
            .fetch_all(&pool)
            .await
            .unwrap();
        let old = SystemTime::now() - std::time::Duration::from_secs(172800);
        for path in &locations {
            std::fs::File::open(media.root().join(path))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let revision = "a".repeat(64);
        let stale = [
            format!("cache/9999/{revision}/card.jpg"),
            format!("staging/{revision}/crashed.png"),
            format!("objects/.upload-{revision}.pending"),
        ];
        let unknown = [
            "incoming/supplier.png",
            "objects/orphan.png",
            "brand/orphan.png",
            "library-other/source.png",
        ];
        for name in stale.iter().map(String::as_str).chain(unknown) {
            let path = media.root().join(name);
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&path, b"synthetic").await.unwrap();
            std::fs::File::open(path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let dry = completed_prune(&media, &pool, true).await;
        assert_eq!(dry.files, 3);
        assert!(stale.iter().all(|p| media.root().join(p).is_file()));
        assert_eq!(completed_prune(&media, &pool, false).await.files, 3);
        assert!(stale.iter().all(|p| !media.root().join(p).exists()));
        assert!(unknown.iter().all(|p| media.root().join(p).is_file()));
        assert!(locations.iter().all(|p| media.root().join(p).is_file()));
        assert!(media.preview_file(&pool, &source).await.unwrap().is_some());
    })
    .await;
}
