use super::*;
use talocart_platform::{
    banners::{BannerError, Banners, CONTENT, Input},
    content::{ContentError, Draft, Language, Translation},
    identity::{Permission, Role, Session},
    locale::Locale,
    media::{
        CacheAction, MediaStore,
        library::{LibraryError, MediaLibrary},
    },
    storefront::Storefront,
};
fn lang(code: &str) -> Language {
    Language::new(Locale::parse(code).unwrap(), Locale::ENGLISH)
}
fn input(image: i64) -> Input {
    Input {
        label: "Summer <sale>".into(),
        name: "Summer & offers".into(),
        description: "/catalog?campaign=summer".into(),
        image_id: image.to_string(),
        active: true,
        sort_order: "0".into(),
    }
}
fn draft() -> Draft {
    Draft {
        primary: "en".into(),
        active: Some(vec!["en".into(), "it".into()]),
        rows: vec![Translation {
            locale: "it".into(),
            name: "Offerte <estate>".into(),
            description: "/brands".into(),
            ..Translation::default()
        }],
        submitted: None,
    }
}
async fn fixture(pool: &PgPool) -> (Identity, Session, Banners, MediaStore, MediaLibrary, i64) {
    let identity = installed(pool).await;
    let s = taxonomy_session(&identity).await;
    Storefront::new(pool.clone())
        .save_languages(&s, &s.csrf, "en", &["en".into(), "it".into()])
        .await
        .unwrap();
    let media = test_media(pool).await;
    let lib = MediaLibrary::new(pool.clone(), identity.clone(), media.clone());
    let mut bytes = std::io::Cursor::new(vec![]);
    image::DynamicImage::new_rgb8(2400, 800)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let image = lib
        .upload(&s, &s.csrf, None, bytes.into_inner(), "banner.png", false)
        .await
        .unwrap();
    let banners = Banners::new(pool.clone(), identity.clone());
    (identity, s, banners, media, lib, image)
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn banners_homepage_bilingual_order_links_seo_and_base_membership() {
    isolated(|pool, _| async move {
        let (_, s, banners, media, _, image) = fixture(&pool).await;
        assert!(
            talocart_platform::modules::BASE
                .components
                .iter()
                .any(|c| c.id == "banners")
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
        let empty = html(request(&app, "GET", "/", "", "", false).await).await;
        assert!(!empty.contains("data-banners"));
        assert!(!empty.contains("class=\"hero\""));
        assert!(empty.contains("<h1"));
        let id = banners
            .save(&s, &s.csrf, 0, 0, &input(image), Some(&draft()))
            .await
            .unwrap();
        let mut other = input(image);
        other.label = "First".into();
        other.description.clear();
        other.sort_order = "1".into();
        let second = banners.save(&s, &s.csrf, 0, 0, &other, None).await.unwrap();
        for (prefix, title, link) in [
            ("", "Summer &#38; offers", "/catalog?campaign=summer"),
            ("/it", "Offerte &#60;estate&#62;", "/it/brands"),
        ] {
            let r = request(
                &app,
                "GET",
                if prefix.is_empty() { "/" } else { prefix },
                "",
                "",
                false,
            )
            .await;
            assert_eq!(r.status(), StatusCode::OK);
            let h = html(r).await;
            assert!(h.contains(title));
            assert!(h.contains(&format!("href=\"{link}\"")));
            assert!(h.contains("data-banner-controls hidden"));
            assert!(h.contains("fetchpriority=\"high\""));
            assert!(h.contains(&format!("/media/image/{image}/card 512w")));
            assert!(h.contains("width=\"2400\" height=\"800\""));
            assert!(!h.contains("class=\"hero\""));
            assert!(h.contains("home-featured"));
            assert!(h.contains("rel=\"canonical\""));
            assert_eq!(h.matches("data-banner role=").count(), 2);
        }
        assert_eq!(
            banners
                .public(lang("en"))
                .await
                .unwrap()
                .iter()
                .map(|b| b.id)
                .collect::<Vec<_>>(),
            [id, second]
        );
        banners.delete(&s, &s.csrf, second, 0).await.unwrap();
        let one = html(request(&app, "GET", "/", "", "", false).await).await;
        assert!(!one.contains("data-banner-controls"));
        assert!(one.contains("data-banners"));
        let mut disabled = input(image);
        disabled.active = false;
        banners
            .save(&s, &s.csrf, id, 0, &disabled, None)
            .await
            .unwrap();
        assert!(
            !html(request(&app, "GET", "/", "", "", false).await)
                .await
                .contains("data-banners")
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn banners_global_cache_renditions_clear_repair_usage_and_source_retention() {
    isolated(|pool,_|async move{
        let (_,s,banners,media,lib,image)=fixture(&pool).await;
        let asset=lib.asset(&s,image,false).await.unwrap();
        let original=media.read_registered(&pool,&asset.key).await.unwrap();
        let id=banners.save(&s,&s.csrf,0,0,&input(image),None).await.unwrap();
        assert!(media.process_cache_job(&pool).await.unwrap());
        let rows:Vec<(String,String,i32,i32)>=sqlx::query_as("SELECT r.profile,m.key,m.width,m.height FROM media_renditions r JOIN media_objects m ON m.key=r.object_key WHERE r.source_key=$1 ORDER BY m.width").bind(&asset.key).fetch_all(&pool).await.unwrap();
        assert_eq!(rows.len(),4);
        for (_,key,w,h) in &rows {assert!(*w<=2400);assert!((w-h*3).abs()<=1);assert!(!media.read_registered(&pool,key).await.unwrap().is_empty());}
        let mut text_only = input(image); text_only.name = "Updated alternative text".into();
        banners.save(&s,&s.csrf,id,0,&text_only,None).await.unwrap();
        assert!(!media.process_cache_job(&pool).await.unwrap(), "Text-only edits must not requeue a complete cache");
        let usage=lib.usages(&s,image,false).await.unwrap();assert!(usage.iter().any(|u|u.kind=="Banner" && u.resource_id==id));
        assert!(matches!(lib.trash(&s,&s.csrf,image,false,false,true).await,Err(LibraryError::InUse)));
        assert!(sqlx::query("DELETE FROM media_library WHERE key=$1").bind(&asset.key).execute(&pool).await.is_err(),"FK protects the selected source");
        let before=media.rendition_url(&pool,image,"large").await.unwrap().unwrap();
        media.queue_cache_action(&pool,&s,&s.csrf,CacheAction::Clear).await.unwrap();
        assert_eq!(media.read_registered(&pool,&asset.key).await.unwrap(),original);
        assert!(media.rendition_url(&pool,image,"large").await.unwrap().is_some(),"original fallback is usable during a clear");
        assert!(media.process_cache_job(&pool).await.unwrap());
        let rebuilt=media.rendition_url(&pool,image,"large").await.unwrap().unwrap();assert_ne!(before,rebuilt);
        let key:String=sqlx::query_scalar("SELECT object_key FROM media_renditions WHERE source_key=$1 AND profile='large'").bind(&asset.key).fetch_one(&pool).await.unwrap();
        tokio::fs::remove_file(media.registered_path(&pool,&key).await.unwrap()).await.unwrap();
        assert!(media.rendition_url(&pool,image,"large").await.unwrap().is_some());
        assert!(media.process_cache_job(&pool).await.unwrap());
        assert_ne!(rebuilt,media.rendition_url(&pool,image,"large").await.unwrap().unwrap());
        banners.delete(&s,&s.csrf,id,1).await.unwrap();
        assert!(lib.usages(&s,image,false).await.unwrap().is_empty());
        assert_eq!(media.read_registered(&pool,&asset.key).await.unwrap(),original);
        lib.trash(&s,&s.csrf,image,false,false,true).await.unwrap();
        assert!(matches!(banners.save(&s,&s.csrf,0,0,&input(image),None).await,Err(BannerError::Invalid(_))));
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn banners_revision_capacity_and_translations_are_atomic_under_concurrency() {
    isolated(|pool, _| async move {
        let (_, s, banners, _, _, image) = fixture(&pool).await;
        let i = input(image);
        let id = banners
            .save(&s, &s.csrf, 0, 0, &i, Some(&draft()))
            .await
            .unwrap();
        let (mut a, mut b) = (i.clone(), i.clone());
        a.label = "A".into();
        b.label = "B".into();
        let (r1, r2) = tokio::join!(
            banners.save(&s, &s.csrf, id, 0, &a, None),
            banners.save(&s, &s.csrf, id, 0, &b, None)
        );
        assert_eq!(usize::from(r1.is_ok()) + usize::from(r2.is_ok()), 1);
        assert!(matches!(
            (&r1, &r2),
            (Err(BannerError::Stale), _) | (_, Err(BannerError::Stale))
        ));
        assert!(matches!(
            banners.delete(&s, &s.csrf, id, 0).await,
            Err(BannerError::Stale)
        ));
        let mut bad = draft();
        bad.rows[0].description = "javascript:alert(1)".into();
        assert!(matches!(
            banners.save(&s, &s.csrf, id, 1, &i, Some(&bad)).await,
            Err(BannerError::Content(ContentError::Invalid(_)))
        ));
        assert_eq!(banners.get(&s, id, lang("en")).await.unwrap().revision, 1);
        assert_eq!(
            banners.get(&s, id, lang("it")).await.unwrap().description,
            "/brands"
        );
        for _ in 0..8 {
            banners.save(&s, &s.csrf, 0, 0, &i, None).await.unwrap();
        }
        let (r1, r2) = tokio::join!(
            banners.save(&s, &s.csrf, 0, 0, &i, None),
            banners.save(&s, &s.csrf, 0, 0, &i, None)
        );
        assert_eq!(usize::from(r1.is_ok()) + usize::from(r2.is_ok()), 1);
        assert!(matches!(
            (&r1, &r2),
            (Err(BannerError::Limit), _) | (_, Err(BannerError::Limit))
        ));
        assert_eq!(banners.public(lang("en")).await.unwrap().len(), 10);
        let mut hidden = i.clone();
        hidden.active = false;
        for _ in 10..50 {
            banners
                .save(&s, &s.csrf, 0, 0, &hidden, None)
                .await
                .unwrap();
        }
        assert!(matches!(
            banners.save(&s, &s.csrf, 0, 0, &hidden, None).await,
            Err(BannerError::Limit)
        ));
        assert_eq!(
            banners
                .list(&s, 999, lang("en"))
                .await
                .unwrap()
                .entries
                .len(),
            10
        );
        assert!(matches!(
            banners.list(&s, 0, lang("en")).await,
            Err(BannerError::Invalid(_))
        ));
        let mut incomplete = draft();
        incomplete.rows[0].description.clear();
        // Persisted secondary empty values keep primary fallback, without materializing it.
        banners
            .save(&s, &s.csrf, id, 1, &hidden, Some(&incomplete))
            .await
            .unwrap();
        assert!(
            CONTENT
                .load(&pool, &Identity::new(pool.clone()), &s, id)
                .await
                .unwrap()
                .iter()
                .find(|r| r.locale == "it")
                .unwrap()
                .description
                .is_empty()
        );
        Storefront::new(pool.clone())
            .save_languages(&s, &s.csrf, "it", &["en".into(), "it".into()])
            .await
            .unwrap();
        assert!(matches!(
            banners
                .save(&s, &s.csrf, id, 2, &hidden, Some(&draft()))
                .await,
            Err(BannerError::Content(ContentError::LanguageChanged))
        ));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn banners_http_permissions_csrf_picker_drafts_stale_and_deleted_forms() {
    isolated(|pool,_|async move{
        let(identity,s,banners,media,_,image)=fixture(&pool).await;
        let role=identity.create_role(&s,&s.csrf,"Banner reader",&[Permission::SettingsRead]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"reader@example.test","correct horse battery".into(),Role::parse(&role).unwrap()).await.unwrap();
        let(_,anon)=identity.anonymous().await.unwrap();let raw=identity.login(&anon,&anon.csrf,"reader@example.test","correct horse battery".into()).await.unwrap();
        let reader=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let id=banners.save(&s,&s.csrf,0,0,&input(image),None).await.unwrap();
        let app=router(test_app_config(pool.clone(),"Shop".into(), talocart_web::AppStateConfig { media, ..Default::default() }).await);
        assert_eq!(request(&app,"GET","/admin/banners",&cookie,"",false).await.status(),StatusCode::OK);
        assert_eq!(request(&app,"GET","/admin/banners/new",&cookie,"",false).await.status(),StatusCode::FORBIDDEN);
        let h=html(request(&app,"GET",&format!("/admin/banners/{id}"),&cookie,"",false).await).await;assert!(h.contains("readonly"));assert!(!h.contains("value=\"save\""));
        assert!(matches!(banners.save(&reader,&reader.csrf,id,0,&input(image),None).await,Err(BannerError::Access(talocart_platform::technical::AccessError::Forbidden))));
        let raw=authenticated(&identity).await;let owner=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let form=format!("csrf={}&revision=0&label=Preserved+draft&name=Offers&description=javascript%3Aalert%281%29&sort_order=0&image_id={image}&active=true&content_primary_locale=en&content_active_locales=en%2Cit&content_it_name=Bozza+conservata",owner.csrf);
        for hx in [false,true]{
            let r=request(&app,"POST",&format!("/admin/banners/{id}"),&cookie,&form,hx).await;assert_eq!(r.status(),StatusCode::UNPROCESSABLE_ENTITY);let h=html(r).await;assert!(h.contains("Preserved draft"));assert!(h.contains("Bozza conservata"));assert!(h.contains("aria-invalid=\"true\""));
        }
        assert_eq!(request(&app,"POST",&format!("/admin/banners/{id}"),&cookie,&form.replace(&owner.csrf,"bad"),false).await.status(),StatusCode::FORBIDDEN);
        let form=form.replace("javascript%3Aalert%281%29","%2Fcatalog");
        let picker=html(request(&app,"POST",&format!("/admin/banners/{id}"),&cookie,&format!("{form}&action=browse"),false).await).await;
        assert!(picker.contains("banner-media-choice"));assert!(picker.contains("Preserved draft"));
        assert_eq!(banners.get(&s,id,lang("en")).await.unwrap().label,"Summer <sale>");
        assert_eq!(request(&app,"POST",&format!("/admin/banners/{id}"),&cookie,&form,false).await.status(),StatusCode::SEE_OTHER);
        let r=request(&app,"POST",&format!("/admin/banners/{id}"),&cookie,&form,false).await;assert_eq!(r.status(),StatusCode::CONFLICT);assert!(html(r).await.contains("Preserved draft"));
        banners.delete(&s,&s.csrf,id,1).await.unwrap();
        let r=request(&app,"POST",&format!("/admin/banners/{id}"),&cookie,&form,false).await;assert_eq!(r.status(),StatusCode::CONFLICT);assert!(html(r).await.contains("Preserved draft"));
        identity.update_role(&s,&s.csrf,&role,"Banner reader",&[]).await.unwrap();assert!(matches!(banners.list(&reader,1,lang("en")).await,Err(BannerError::Access(talocart_platform::technical::AccessError::Forbidden))));
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn banners_links_localize_only_store_routes() {
    isolated(|pool, _| async move {
        let (_, s, banners, media, _, image) = fixture(&pool).await;
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
        let mut data = input(image);
        let id = banners.save(&s, &s.csrf, 0, 0, &data, None).await.unwrap();
        for (revision, (destination, expected)) in [
            ("/media/image/1/large", "/media/image/1/large"),
            ("/admin/login", "/admin/login"),
            ("/catalog?q=test#offers", "/it/catalog?q=test#offers"),
            ("/en#offers", "/it#offers"),
            ("https://example.test/offers", "https://example.test/offers"),
        ]
        .into_iter()
        .enumerate()
        {
            data.description = destination.into();
            banners
                .save(&s, &s.csrf, id, revision as i64, &data, None)
                .await
                .unwrap();
            let page = html(request(&app, "GET", "/it", "", "", false).await).await;
            assert!(
                page.contains(&format!("href=\"{expected}\"")),
                "{destination}"
            );
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn shared_media_picker_folders_upload_and_drafts_are_authorized_and_never_saved() {
    async fn upload(
        app: &Router,
        cookie: &str,
        fields: &[(&str, String)],
        name: &str,
        bytes: &[u8],
    ) -> Response {
        let mut body = Vec::new();
        for (k, v) in fields {
            body.extend_from_slice(
                format!("--picker\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                    .as_bytes(),
            );
        }
        body.extend_from_slice(format!("--picker\r\nContent-Disposition: form-data; name=\"picker_image\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n--picker--\r\n");
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/media/picker/upload")
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "multipart/form-data; boundary=picker")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    isolated(|pool,_| async move {
        let (identity,s,_,media,lib,image) = fixture(&pool).await;
        let folder = lib.create_folder(&s,&s.csrf,None,"Picker folder",false).await.unwrap();
        let child = lib.create_folder(&s,&s.csrf,Some(folder),"Nested",false).await.unwrap();
        let document = lib.upload(&s,&s.csrf,Some(child),b"document".to_vec(),"excluded.txt",false).await.unwrap();
        let raw = authenticated(&identity).await;
        let owner=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let app=router(test_app_config(pool.clone(),"Shop".into(), talocart_web::AppStateConfig { media, ..Default::default() }).await);
        for editor in ["banner:0", "settings"] {
            let mut base=format!("csrf={}&revision=0&label=Preserved+draft&name=Draft+store&content_primary_locale=en&content_active_locales=en%2Cit&content_it_name=Bozza&media_target=logo&picker=true&media_folder={child}&media_editor={editor}",owner.csrf);
            if editor == "settings" { base = base.replace("&revision=0", "").replace("&label=Preserved+draft", "").replace("&picker=true", ""); }
            let r=request(&app,"POST","/admin/media",&cookie,&format!("{base}&media_folder_name=&action=media_folder_{child}"),false).await;
            assert_eq!(r.status(),StatusCode::OK);let h=html(r).await;
            assert!(h.contains("class=\"admin-main media-main\""));assert!(h.contains("id=\"media-draft\""));assert!(h.contains("class=\"media-upload\" open"));assert!(h.contains("id=\"media-format\""));assert!(h.contains("media-folder-tree"));assert!(h.contains("Nested"));assert!(!h.contains(&format!("choose_{document}")));assert!(!h.contains("excluded.txt"));
            assert!(h.contains("id=\"folder-name\""), "Draft image pickers offer folder creation");
            let create = format!("{base}&media_folder_name=Created-{editor}");
            let r = request(&app,"POST","/admin/media/picker/folders",&cookie,&create,false).await;
            assert_eq!(r.status(), StatusCode::OK);
            let h = html(r).await;
            assert!(h.contains("Draft store") && h.contains("Created-"));
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_folders WHERE parent_id=$1 AND name=$2").bind(child).bind(format!("Created-{editor}")).fetch_one(&pool).await.unwrap(),1);
            let r = request(&app,"POST","/admin/media/picker/folders",&cookie,&create,false).await;
            assert_eq!(r.status(),StatusCode::CONFLICT);
            assert!(html(r).await.contains("Draft store"));
            let invalid = create.replace(&format!("Created-{editor}"), "..");
            let r = request(&app,"POST","/admin/media/picker/folders",&cookie,&invalid,false).await;
            assert_eq!(r.status(),StatusCode::UNPROCESSABLE_ENTITY);
            assert!(html(r).await.contains("Draft store"));
            assert_eq!(request(&app,"POST","/admin/media/picker/folders",&cookie,&create.replace(&owner.csrf,"bad"),false).await.status(),StatusCode::FORBIDDEN);
            let branch = request(&app,"POST",&format!("/admin/media/tree/{folder}"),&cookie,&base,true).await;
            assert_eq!(branch.status(),StatusCode::OK);assert!(html(branch).await.contains("Nested"));
            assert_eq!(request(&app,"POST",&format!("/admin/media/tree/{folder}"),&cookie,&base.replace(&owner.csrf,"bad"),true).await.status(),StatusCode::FORBIDDEN);
            let mut fields=vec![("csrf",owner.csrf.clone()),("media_editor",editor.into()),("revision","0".into()),("label","Preserved draft".into()),("name","Draft store".into()),("content_primary_locale","en".into()),("content_active_locales","en,it".into()),("content_it_name","Bozza".into()),("media_target","logo".into()),("media_folder",child.to_string())];
            if editor == "settings" { fields.retain(|(key,_)| !["revision", "label"].contains(key)); }
            let r=upload(&app,&cookie,&fields,"invalid.png",b"bad").await;assert_eq!(r.status(),StatusCode::OK);let h=html(r).await;assert!(h.contains("media-error"));assert!(h.contains("Draft store"));
            let filename = if editor=="settings" {"settings-picker.png"} else {"banner-picker.png"};
            let r=upload(&app,&cookie,&fields,filename,include_bytes!("../fixtures/avatar.png")).await;assert_eq!(r.status(),StatusCode::OK);let h=html(r).await;
            assert!(!h.contains("media-error"));assert!(h.contains("Draft store"));assert!(h.contains(filename));
            let uploaded:i64=sqlx::query_scalar("SELECT m.id FROM media_objects m JOIN media_library l ON l.key=m.key WHERE l.name=$1 AND l.folder_id=$2").bind(filename).bind(child).fetch_one(&pool).await.unwrap();
            assert!(h.contains(&format!("choose_{uploaded}")));
            fields[0].1="bad".into();assert_eq!(upload(&app,&cookie,&fields,"forbidden.png",include_bytes!("../fixtures/avatar.png")).await.status(),StatusCode::FORBIDDEN);
            fields[0].1=owner.csrf.clone();fields[1].1="https://example.test".into();assert_eq!(upload(&app,&cookie,&fields,"forbidden.png",include_bytes!("../fixtures/avatar.png")).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
            let duplicate=format!("{base}&media_editor={editor}&action=choose_{uploaded}");
            assert_eq!(request(&app,"POST","/admin/media",&cookie,&duplicate,false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
            let selected=html(request(&app,"POST","/admin/media",&cookie,&format!("{base}&action=choose_{uploaded}"),false).await).await;
            assert!(selected.contains(&uploaded.to_string()));
        }
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM home_banners").fetch_one(&pool).await.unwrap(),0);
        assert_ne!(sqlx::query_scalar::<_,String>("SELECT name FROM store_settings").fetch_one(&pool).await.unwrap(),"Draft store");
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_library WHERE name='forbidden.png'").fetch_one(&pool).await.unwrap(),0);
        // A role requesting write access receives the corresponding read permissions.
        let role=identity.create_role(&s,&s.csrf,"Banner media writer",&[Permission::SettingsWrite,Permission::MediaRead,Permission::MediaWrite]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"banner-media@example.test","correct horse battery".into(),Role::parse(&role).unwrap()).await.unwrap();
        let(_,anon)=identity.anonymous().await.unwrap();let raw=identity.login(&anon,&anon.csrf,"banner-media@example.test","correct horse battery".into()).await.unwrap();
        let writer=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        assert!(writer.allows(Permission::SettingsRead));
        let base=format!("csrf={}&revision=0&media_editor=banner:0&media_folder={child}",writer.csrf);
        assert_eq!(request(&app,"POST","/admin/media",&cookie,&format!("{base}&action=browse"),false).await.status(),StatusCode::OK);
        assert_eq!(request(&app,"POST",&format!("/admin/media/tree/{folder}"),&cookie,&base,true).await.status(),StatusCode::OK,"Banner folder expansion must use the same permissions as opening the picker");
        let fields=vec![("csrf",writer.csrf.clone()),("media_editor","banner:0".into()),("revision","0".into()),("media_folder",child.to_string())];
        assert_eq!(upload(&app,&cookie,&fields,"writer.png",include_bytes!("../fixtures/avatar.png")).await.status(),StatusCode::OK,"A normalized media writer can upload from a banner draft");
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_library WHERE name='writer.png'").fetch_one(&pool).await.unwrap(),1);
        let settings=base.replace("banner:0","settings")+"&media_target=logo";
        for path in [format!("/admin/media/tree/{folder}"),format!("/admin/media/picker/{image}")] {
            assert_eq!(request(&app,"POST",&path,&cookie,&settings,false).await.status(),StatusCode::OK,"Normalized write permissions also allow settings draft navigation");
        }
        // A media reader can browse and choose but cannot upload through the shared endpoint.
        let role=identity.create_role(&s,&s.csrf,"Picker reader",&[Permission::SettingsWrite,Permission::MediaRead]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"picker@example.test","correct horse battery".into(),Role::parse(&role).unwrap()).await.unwrap();
        let(_,anon)=identity.anonymous().await.unwrap();let raw=identity.login(&anon,&anon.csrf,"picker@example.test","correct horse battery".into()).await.unwrap();
        let reader=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let h=html(request(&app,"POST","/admin/media",&cookie,&format!("csrf={}&revision=0&action=browse&image_id={image}&media_editor=banner:0",reader.csrf),false).await).await;assert!(h.contains("choose_"));assert!(!h.contains("picker_image"));
        assert_eq!(upload(&app,&cookie,&[("csrf",reader.csrf.clone()),("media_editor","banner:0".into())],"forbidden.png",include_bytes!("../fixtures/avatar.png")).await.status(),StatusCode::FORBIDDEN);
        assert_eq!(request(&app,"POST","/admin/media/picker/folders",&cookie,&format!("csrf={}&media_editor=banner:0&media_folder_name=Forbidden",reader.csrf),false).await.status(),StatusCode::FORBIDDEN);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn picker_details_return_to_the_same_draft_filters_and_result_page() {
    isolated(|pool, _| async move {
        let (identity, s, _, media, lib, image) = fixture(&pool).await;
        let folder = lib.create_folder(&s, &s.csrf, None, "Detail picker", false).await.unwrap();
        let mut last = image;
        for number in 0..41 {
            last = lib.upload(&s, &s.csrf, Some(folder), include_bytes!("../fixtures/avatar.png").to_vec(), &format!("detail-{number:02}.png"), false).await.unwrap();
        }
        let document = lib.upload(&s, &s.csrf, Some(folder), b"document".to_vec(), "excluded.txt", false).await.unwrap();
        let private = lib.upload(&s, &s.csrf, None, include_bytes!("../fixtures/avatar.png").to_vec(), "private.png", true).await.unwrap();
        let trashed = lib.upload(&s, &s.csrf, None, include_bytes!("../fixtures/avatar.png").to_vec(), "trashed.png", false).await.unwrap();
        lib.trash(&s, &s.csrf, trashed, false, false, true).await.unwrap();
        let raw = authenticated(&identity).await;
        let owner = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app_config(pool.clone(), "Shop".into(), talocart_web::AppStateConfig { media, ..Default::default() }).await);
        let route = format!("/admin/media/picker/{last}");
        for editor in ["banner:0", "settings"] {
            let mut form = format!("csrf={}&media_editor={editor}&media_target=logo&revision=0&label=Unsaved+detail+draft&name=Draft+%3C%26%3E&image_id={image}&content_primary_locale=en&content_active_locales=en%2Cit&content_it_name=Bozza&purchase_country=IT&purchase_country=FR&media_folder={folder}&media_query=detail-&media_format=png&media_usage=unused&media_sort=name&media_page=2&media_folders_page=3", owner.csrf);
            if editor == "settings" {
                form = form.replace("&revision=0", "").replace("&label=Unsaved+detail+draft", "").replace(&format!("&image_id={image}"), "");
            }
            for hx in [false, true] {
                let r = request(&app, "POST", &route, &cookie, &form, hx).await;
                assert_eq!(r.status(), StatusCode::OK);
                let h = html(r).await;
                assert!(h.contains("<h1>detail-40.png</h1>"));
                for (key, value) in [("media_page", "2"), ("media_query", "detail-"), ("media_format", "png"), ("media_usage", "unused"), ("media_sort", "name"), ("media_folders_page", "3")] {
                    assert!(h.contains(&format!("name=\"{key}\" value=\"{value}\"")), "{key}");
                }
                assert!(h.contains("value=\"media_return\""));
                assert!(h.contains(&format!("value=\"choose_{last}\"")));
                assert!(!h.contains("action=\"/admin/media/select\""));
                assert!(!h.contains("value=\"trash\""));
                assert!(!h.contains("value=\"move\""));
                assert_eq!(h.matches("name=\"purchase_country\"").count(), 2);
                let r = request(&app, "POST", "/admin/media", &cookie, &format!("{form}&action=media_return"), hx).await;
                assert_eq!(r.status(), StatusCode::OK);
                let h = html(r).await;
                assert!(h.contains("name=\"media_page\" value=\"2\""));
                assert_eq!(h.matches("data-media-id=").count(), 1);
                assert!(h.contains(&format!("data-media-id=\"{last}\"")));
                assert!(h.contains("value=\"detail-\""));
                assert!(h.contains("value=\"media_close\""));
            }
            assert_eq!(request(&app,"POST",&route,&cookie,&form.replace(&owner.csrf,"bad"),false).await.status(),StatusCode::FORBIDDEN);
            assert_eq!(request(&app,"POST","/admin/media",&cookie,&format!("{}&action=media_return",form.replace(&owner.csrf,"bad")),false).await.status(),StatusCode::FORBIDDEN);
            assert_eq!(request(&app,"POST",&route,&cookie,&form.replace(editor,"https://example.test"),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
            for id in [document, private, trashed, i64::MAX] {
                assert_eq!(request(&app,"POST",&format!("/admin/media/picker/{id}"),&cookie,&form,false).await.status(),StatusCode::NOT_FOUND);
            }
            let selected = request(&app,"POST","/admin/media",&cookie,&format!("{form}&media_uses_page=2&action=choose_{last}"),false).await;
            assert_eq!(selected.status(), StatusCode::OK);
            let h = html(selected).await;
            let selected_field = if editor == "settings" { "logo" } else { "image_id" };
            assert!(h.contains(&format!("name=\"{selected_field}\" value=\"{last}\"")));
            assert!(!h.contains("id=\"media-draft\""));
        }
        let h = html(request(&app,"GET",&format!("/admin/media/{last}?target=logo&folder={folder}&q=detail-&format=png&usage=unused&sort=name&page=2&folders_page=3"),&cookie,"",false).await).await;
        assert!(h.contains("page=2"));
        assert!(h.contains("q=detail-"));
        assert!(h.contains(&format!("folder={folder}")));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM home_banners").fetch_one(&pool).await.unwrap(),0);
        assert_ne!(sqlx::query_scalar::<_,String>("SELECT name FROM store_settings").fetch_one(&pool).await.unwrap(),"Draft <&>");
        let role = identity.create_role(&s,&s.csrf,"No media access",&[Permission::SettingsWrite]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"no-media@example.test","correct horse battery".into(),Role::parse(&role).unwrap()).await.unwrap();
        let (_,anon) = identity.anonymous().await.unwrap();
        let raw = identity.login(&anon,&anon.csrf,"no-media@example.test","correct horse battery".into()).await.unwrap();
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let form = format!("csrf={}&media_editor=banner:0",reader.csrf);
        assert_eq!(request(&app,"POST",&route,&format!("talocart_session={raw}"),&form,false).await.status(),StatusCode::FORBIDDEN);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn banners_save_database_failures_are_not_rendered_as_invalid_drafts() {
    isolated(|pool, _| async move {
        let (identity, _, _, media, _, image) = fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let owner = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app_config(pool.clone(), "Shop".into(), talocart_web::AppStateConfig { media, ..Default::default() }).await);
        let form = format!("csrf={}&revision=0&label=Failure+draft&name=Offers&description=%2Fcatalog&sort_order=0&image_id={image}&active=true&content_primary_locale=en&content_active_locales=en%2Cit", owner.csrf);
        sqlx::query("CREATE FUNCTION banner_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic private database detail'; END $$").execute(&pool).await.unwrap();
        for table in ["home_banners", "home_banners_translations"] {
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TRIGGER banner_failure BEFORE INSERT ON {table} FOR EACH ROW EXECUTE FUNCTION banner_failure()"))).execute(&pool).await.unwrap();
            for (locale, expected) in [("en", "Service temporarily unavailable"), ("it", "Servizio temporaneamente non disponibile")] {
                sqlx::query("UPDATE administrators SET locale=$1 WHERE id=$2").bind(locale).bind(owner.administrator_id).execute(&pool).await.unwrap();
                for hx in [false, true] {
                    let response = request(&app, "POST", "/admin/banners/new", &cookie, &form, hx).await;
                    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                    let body = html(response).await;
                    assert!(body.contains(expected), "{body}");
                    assert!(!body.contains("Failure draft"));
                    assert!(!body.contains("synthetic private database detail"));
                    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM home_banners").fetch_one(&pool).await.unwrap(), 0);
                }
            }
            sqlx::query(sqlx::AssertSqlSafe(format!("DROP TRIGGER banner_failure ON {table}"))).execute(&pool).await.unwrap();
        }
    }).await;
}
