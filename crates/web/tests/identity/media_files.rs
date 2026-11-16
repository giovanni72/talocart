use super::*;
use talocart_platform::media::{
    MediaStore,
    library::{Filter, MediaLibrary},
};

async fn resident(media: &MediaStore, path: &str, bytes: &[u8]) {
    let path = media.root().join(path);
    tokio::fs::create_dir_all(path.parent().expect("test parent"))
        .await
        .unwrap();
    tokio::fs::write(&path, bytes).await.unwrap();
    std::fs::File::open(path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(SystemTime::now() - std::time::Duration::from_secs(5)),
        )
        .unwrap();
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn automatic_media_files_preserve_sources_privacy_and_cache_boundaries_under_concurrency() {
    isolated(|pool,_|async move{
        let identity=installed(&pool).await;let s=taxonomy_session(&identity).await;let media=test_media(&pool).await;
        let library=MediaLibrary::new(pool.clone(),identity.clone(),media.clone());
        let png=include_bytes!("../fixtures/avatar.png");let pdf=b"%PDF-1.4\nSynthetic file\n%%EOF\n";
        let private=format!("private/account-{}/document.pdf",s.administrator_id.unwrap());
        for (path,bytes) in [("root.pdf",pdf.as_slice()),("FTP cartella/Foto.png",png.as_slice()),("FTP cartella/Manuale d'uso è.txt",b"UTF-8 source".as_slice()),("objects/resident.png",png.as_slice()),("brand/resident.png",png.as_slice()),(private.as_str(),pdf.as_slice()),("cache/1/generated.png",png.as_slice()),("staging/unpublished.pdf",pdf.as_slice()),("FTP cartella/.hidden.txt",b"private".as_slice()),("FTP cartella/copy.pdf.part",pdf.as_slice())] {resident(&media,path,bytes).await;}
        tokio::fs::create_dir_all(media.root().join("FTP cartella/Empty")).await.unwrap();
        #[cfg(unix)] std::os::unix::fs::symlink(std::env::temp_dir(),media.root().join("escape")).unwrap();
        let (first,second)=tokio::join!(media.discover_files(&pool),media.discover_files(&pool));first.unwrap();second.unwrap();
        media.discover_files(&pool).await.unwrap();
        let paths:Vec<String>=sqlx::query_scalar("SELECT path FROM media_locations ORDER BY path").fetch_all(&pool).await.unwrap();
        assert_eq!(paths.len(),6,"Only original sources become assets: {paths:?}");
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_cache_jobs").fetch_one(&pool).await.unwrap(),3,"Documents never create photographic cache jobs");
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_folders WHERE path='FTP cartella/Empty'").fetch_one(&pool).await.unwrap(),1);
        assert_eq!(library.assets(&s,&Filter::default()).await.unwrap().len(),5);
        assert_eq!(library.assets(&s,&Filter{images_only:true,..Default::default()}).await.unwrap().len(),3);
        let documents=library.assets(&s,&Filter{format:"pdf".into(),..Default::default()}).await.unwrap();assert_eq!(documents.len(),1);assert!(!documents[0].is_image());
        assert_eq!(library.assets(&s,&Filter{format:"other".into(),..Default::default()}).await.unwrap().len(),1);
        let private_id:i64=sqlx::query_scalar("SELECT m.id FROM media_locations p JOIN media_objects m ON m.key=p.key WHERE p.path=$1").bind(&private).fetch_one(&pool).await.unwrap();
        assert!(library.asset(&s,private_id,false).await.is_err());assert!(library.asset(&s,private_id,true).await.is_ok());
        let role=identity.create_role(&s,&s.csrf,"Other media",&[talocart_platform::identity::Permission::MediaRead]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"other-media@example.test","correct horse battery".into(),talocart_platform::identity::Role::parse(&role).unwrap()).await.unwrap();
        let other=identity.session(&login_as(&identity,"other-media@example.test").await).await.unwrap().unwrap();
        assert!(library.asset(&other,private_id,true).await.is_err());
        media.queue_all_cache(&pool,false).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_cache_jobs j JOIN media_objects m ON m.key=j.source_key WHERE m.width=0").fetch_one(&pool).await.unwrap(),0);
        for (path,bytes) in [("root.pdf",pdf.as_slice()),("FTP cartella/Foto.png",png.as_slice())] {assert_eq!(tokio::fs::read(media.root().join(path)).await.unwrap(),bytes);}
        assert_eq!(media.discover_files(&pool).await.unwrap().acquired,0,"Repeated discovery is idempotent");
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn media_documents_download_management_and_image_selection_are_separate_contracts() {
    isolated(|pool,_|async move{
        let identity=installed(&pool).await;let s=taxonomy_session(&identity).await;let media=test_media(&pool).await;
        let library=MediaLibrary::new(pool.clone(),identity.clone(),media.clone());let pdf=b"%PDF-1.4\nSynthetic document\n%%EOF\n";
        let id=library.upload(&s,&s.csrf,None,pdf.to_vec(),"manual.pdf",false).await.unwrap();
        for invalid in [Vec::new(),vec![0;10*1024*1024+1]] {
            assert!(matches!(library.upload(&s,&s.csrf,None,invalid,"invalid.pdf",false).await,Err(talocart_platform::media::library::LibraryError::FileInvalid)));
        }
        let original=library.asset(&s,id,false).await.unwrap();assert!(!original.is_image());
        let folder=library.create_folder(&s,&s.csrf,None,"Documents",false).await.unwrap();
        let copy=library.copy_named(&s,&s.csrf,id,folder,"copy.pdf",false).await.unwrap();
        library.move_asset(&s,&s.csrf,copy,"Documents","renamed.pdf",false).await.unwrap();
        let copied=library.asset(&s,copy,false).await.unwrap();assert_eq!(media.read_registered(&pool,&copied.key).await.unwrap(),pdf);
        library.trash(&s,&s.csrf,copy,false,false,true).await.unwrap();library.trash(&s,&s.csrf,copy,true,false,false).await.unwrap();
        let mut tx=pool.begin().await.unwrap();assert!(MediaStore::selected_key(&mut tx,id,None).await.is_err());tx.rollback().await.unwrap();
        let cookie=format!("talocart_session={}",authenticated(&identity).await);
        let app=router(test_app_config(pool.clone(),"Files".into(), talocart_web::AppStateConfig { media: media.clone(), ..Default::default() }).await);
        let download=request(&app,"GET",&format!("/admin/media/{id}/download"),&cookie,"",false).await;
        assert_eq!(download.status(),StatusCode::OK);assert!(download.headers()[header::CONTENT_DISPOSITION].to_str().unwrap().starts_with("attachment"));assert_eq!(download.headers()[header::X_CONTENT_TYPE_OPTIONS],"nosniff");
        assert_eq!(to_bytes(download.into_body(),1024).await.unwrap().as_ref(),pdf);
        assert_ne!(request(&app,"GET",&format!("/admin/media/{id}/download"),"","",false).await.status(),StatusCode::OK);
        assert_eq!(request(&app,"GET",&format!("/admin/media/{id}/preview"),&cookie,"",false).await.status(),StatusCode::NOT_FOUND);
        let public=image_request(&app,&format!("/media/image/{id}/original"),"","GET","").await;
        assert_ne!(public.headers().get(header::CONTENT_TYPE).and_then(|v|v.to_str().ok()),Some("application/pdf"));
        for locale in ["en","it"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let body=html(request(&app,"GET",&format!("/admin/media/{id}"),&cookie,"",false).await).await;
            let listing=html(request(&app,"GET",&format!("/admin/media?q=missing&selected={id}"),&cookie,"",false).await).await;
            assert!(listing.contains(if locale=="en" {"aria-label=\"File pagination\""}else{"aria-label=\"Paginazione file\""}));
            assert!(listing.contains(if locale=="en" {"Newly selected file"}else{"File appena selezionato"}));
            assert!(body.contains("media-document"));assert!(!body.contains(&format!("<img src=\"/admin/media/{id}/preview")));
            assert!(body.contains(if locale=="en" {"No attachments. The file stays"} else {"Nessun collegamento. Il file rimane"}));
        }
        // Download streams beyond the former in-memory image limit.
        resident(&media,"large.pdf",pdf).await;
        let file=tokio::fs::OpenOptions::new().write(true).open(media.root().join("large.pdf")).await.unwrap();file.set_len(33*1024*1024).await.unwrap();drop(file);
        std::fs::File::open(media.root().join("large.pdf")).unwrap().set_times(std::fs::FileTimes::new().set_modified(SystemTime::now()-std::time::Duration::from_secs(5))).unwrap();
        media.discover_files(&pool).await.unwrap();
        let large:i64=sqlx::query_scalar("SELECT m.id FROM media_locations p JOIN media_objects m ON m.key=p.key WHERE p.path='large.pdf'").fetch_one(&pool).await.unwrap();
        let download=request(&app,"GET",&format!("/admin/media/{large}/download"),&cookie,"",false).await;
        assert_eq!(to_bytes(download.into_body(),34*1024*1024).await.unwrap().len(),33*1024*1024);
        let copied_large=library.copy_named(&s,&s.csrf,large,folder,"large-copy.pdf",false).await.unwrap();
        assert_eq!(library.asset(&s,copied_large,false).await.unwrap().byte_size,33*1024*1024);
        media.discover_files(&pool).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_library WHERE name='large-copy.pdf'").fetch_one(&pool).await.unwrap(),1,"Retired aliases do not become duplicate assets");
        tokio::fs::remove_file(media.registered_path(&pool,&original.key).await.unwrap()).await.unwrap();
        assert_eq!(request(&app,"GET",&format!("/admin/media/{id}/download"),&cookie,"",false).await.status(),StatusCode::NOT_FOUND);

    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn document_copy_works_with_a_single_connection_pool() {
    isolated(|pool, options| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let media = test_media(&pool).await;
        let one = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let library = MediaLibrary::new(one.clone(), identity, media);
        let id = library
            .upload(
                &s,
                &s.csrf,
                None,
                b"Synthetic document".to_vec(),
                "source.txt",
                false,
            )
            .await
            .unwrap();
        let asset = library.asset(&s, id, false).await.unwrap();
        let copied = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            library.copy_named(&s, &s.csrf, id, asset.folder_id, "copy.txt", false),
        )
        .await
        .expect(
            "Copy must not wait for another connection while holding the pool's only connection",
        )
        .unwrap();
        assert_eq!(
            library.asset(&s, copied, false).await.unwrap().byte_size,
            18
        );
        one.close().await;
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn folder_audit_unicode_move_keeps_descendant_paths() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity, media.clone());
        let parent = lib
            .create_folder(&s, &s.csrf, None, "caffè", false)
            .await
            .unwrap();
        let child = lib
            .create_folder(&s, &s.csrf, Some(parent), "sub", false)
            .await
            .unwrap();
        let id = lib
            .upload(
                &s,
                &s.csrf,
                Some(child),
                b"preserved".to_vec(),
                "file.txt",
                false,
            )
            .await
            .unwrap();
        lib.move_folder(&s, &s.csrf, parent, None, "nuovo", false)
            .await
            .unwrap();
        assert_eq!(
            lib.folder(&s, child, false).await.unwrap().path,
            "nuovo/sub"
        );
        let asset = lib.asset(&s, id, false).await.unwrap();
        assert_eq!(
            media.read_registered(&pool, &asset.key).await.unwrap(),
            b"preserved"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn folder_audit_discovery_does_not_resurrect_moved_directories() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity, media.clone());
        let parent = lib
            .create_folder(&s, &s.csrf, None, "before", false)
            .await
            .unwrap();
        lib.create_folder(&s, &s.csrf, Some(parent), "empty", false)
            .await
            .unwrap();
        lib.upload(
            &s,
            &s.csrf,
            Some(parent),
            b"preserved".to_vec(),
            "file.txt",
            false,
        )
        .await
        .unwrap();
        lib.move_folder(&s, &s.csrf, parent, None, "after", false)
            .await
            .unwrap();
        media.discover_files(&pool).await.unwrap();
        let stale: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM media_folders WHERE path='before' OR path LIKE 'before/%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stale, 0, "Retired directory aliases must not reappear");
        assert!(
            media.root().join("after/empty").is_dir(),
            "Empty descendants are physical folders too"
        );
        resident(&media, "before/new.txt", b"new FTP file").await;
        media.discover_files(&pool).await.unwrap();
        assert!(
            lib.assets(&s, &Filter::default())
                .await
                .unwrap()
                .iter()
                .any(|a| a.name == "new.txt")
        );
        sqlx::query("UPDATE media_retired_files SET retired_at=now()-interval '25 hours'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE media_retired_directories SET retired_at=now()-interval '25 hours'")
            .execute(&pool)
            .await
            .unwrap();
        completed_prune(&media, &pool, false).await;
        assert_eq!(
            tokio::fs::read(media.root().join("before/new.txt"))
                .await
                .unwrap(),
            b"new FTP file"
        );
        assert!(!media.root().join("before/empty").exists());
        assert!(media.root().join("after/empty").is_dir());
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn folder_audit_tree_uses_only_real_nontechnical_directories() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let media = test_media(&pool).await;
        let lib = MediaLibrary::new(pool.clone(), identity, media.clone());
        sqlx::query("INSERT INTO media_folders(name,path) VALUES('Missing','missing'),('Cache','cache'),('Staging','staging')").execute(&pool).await.unwrap();
        tokio::fs::create_dir_all(media.root().join("cache")).await.unwrap();
        tokio::fs::create_dir_all(media.root().join("staging")).await.unwrap();
        assert!(lib.folder_tree(&s, None, 1, false).await.unwrap().iter().all(|b| b.entries.total == 0));
        let real = lib.create_folder(&s, &s.csrf, None, "real", false).await.unwrap();
        let tree = lib.folder_tree(&s, None, 1, false).await.unwrap();
        assert!(tree.iter().flat_map(|b| &b.entries.entries).any(|f| f.id == real));
        tokio::fs::remove_dir(media.root().join("real")).await.unwrap();
        let root = lib.folder(&s, real, false).await.unwrap().parent_id;
        let tree = lib.folder_tree(&s, root, 1, false).await.unwrap();
        assert!(!tree.iter().flat_map(|b| &b.entries.entries).any(|f| f.id == real));
    }).await;
}
