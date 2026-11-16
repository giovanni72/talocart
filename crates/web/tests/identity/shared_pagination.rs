use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn counted_catalog_tax_options_roles_and_directory_share_navigation() {
    isolated(|pool, _| async move {
        let (identity, _, _, _, product, options, _) = variant_fixture(&pool).await;
        let cookie = format!("talocart_session={}", authenticated(&identity).await);
        sqlx::query("INSERT INTO catalog_categories(name,slug,active) SELECT 'Paging category '||n,'paging-category-'||n,n<=23 FROM generate_series(1,25)n").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_brands(name,slug,active) SELECT 'Paging brand '||n,'paging-brand-'||n,n<=103 FROM generate_series(1,105)n").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO tax_rates(name,basis_points,active) SELECT 'Paging rate '||n,2200,n<=23 FROM generate_series(1,25)n").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO tax_classes(name,rate_id,active) SELECT 'Paging class '||n,(SELECT min(id) FROM tax_rates),n<=23 FROM generate_series(1,25)n").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_options(name) SELECT 'Paging option '||n FROM generate_series(1,23)n").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Paging value '||n FROM generate_series(1,53)n").bind(options[0]).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO admin_roles(key,name) SELECT 'role_'||lpad(to_hex(n),16,'0'),'Paging role '||n FROM generate_series(1,23)n").execute(&pool).await.unwrap();
        let roles: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_roles").fetch_one(&pool).await.unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (route,label,total) in [
            ("/admin/catalog/categories?q=Paging&status=active", "Categorie",23),
            ("/admin/catalog/brands?q=Paging&status=inactive", "Marchi",2),
            ("/admin/settings/taxes/rates?q=Paging&status=active", "Aliquote",23),
            ("/admin/settings/taxes/classes?q=Paging&status=active", "Classi fiscali",23),
            ("/admin/catalog/options?", "Opzioni",25),
            ("/admin/roles?", "Ruoli",roles),
        ] {
            let response=request(&app,"GET",&format!("{route}&page=999"),&cookie,"",false).await;
            assert_eq!(response.status(),StatusCode::OK,"{route}");
            let body=html(response).await;
            let pages=(total-1)/20+1;
            assert!(body.contains(&format!("{label} {}–{total} di {total}",(pages-1)*20+1)),"{route}");
            assert!(body.contains(&format!("Pagina {pages} di {pages}")),"{route}");
            assert!(!body.contains("rel=\"next\""));
            assert_eq!(body.contains("rel=\"prev\""),pages>1);
            if route.contains("q=Paging") && pages>1 { assert!(body.contains("q=Paging&#38;status=active&#38;page=1")); }
        }
        let body=html(request(&app,"GET",&format!("/admin/catalog/options/{}?page=2",options[0]),&cookie,"",false).await).await;
        assert!(body.contains("Valori 51–55 di 55"), "Values use the explicit 50-row contract");
        assert!(body.contains("Pagina 2 di 2") && body.contains("rel=\"prev\""));
        assert!(!body.contains("rel=\"next\""));
        let body=html(request(&app,"GET",&format!("/admin/catalog/products/{product}/variants?page=99"),&cookie,"",false).await).await;
        assert!(body.contains("Varianti 0 di 0"));
        for route in ["/admin/catalog/categories?q=missing", "/admin/settings/taxes/rates?q=missing"] {
            let body=html(request(&app,"GET",route,&cookie,"",false).await).await;
            assert!(body.contains("0 di 0") && body.contains("Pagina 1 di 1"));
        }
        let public=html(request(&app,"GET","/brands?page=99","","",false).await).await;
        assert!(public.contains("Marchi 101–104 di 104"));
        assert!(public.contains("Pagina 2 di 2"));
        assert!(!public.contains("Paging brand 104") && !public.contains("Paging brand 105"));
        assert!(public.contains("rel=\"canonical\" href=\"http://127.0.0.1:3000/brands?page=2\""));
        for route in ["/admin/catalog/categories?page=0","/admin/roles?page=0","/admin/users?page=0","/brands?page=0"] {
            assert_eq!(request(&app,"GET",route,&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY,"{route}");
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn counted_reader_keeps_total_and_rows_in_one_concurrent_snapshot() {
    isolated(|pool, _| async move {
        sqlx::query("CREATE TABLE pagination_fixture(id bigint PRIMARY KEY)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO pagination_fixture SELECT generate_series(1,21)").execute(&pool).await.unwrap();
        let mut writer=pool.acquire().await.unwrap();
        let key: i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *writer).await.unwrap();
        sqlx::query("SELECT pg_advisory_lock($1)").bind(i64::from(key)).execute(&mut *writer).await.unwrap();
        let reader_pool=pool.clone();
        let reader=tokio::spawn(async move {
            talocart_platform::pagination::read::<(i64,)>(&reader_pool,
                sqlx::query("WITH gate AS MATERIALIZED (SELECT pg_advisory_xact_lock($1)) SELECT id FROM pagination_fixture CROSS JOIN gate").bind(i64::from(key)), "id",1,20).await.unwrap()
        });
        tokio::time::timeout(std::time::Duration::from_secs(5),async {
            loop {
                let waiting: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND objid=$1::oid AND NOT granted)").bind(key).fetch_one(&pool).await.unwrap();
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("reader waiting after acquiring snapshot");
        sqlx::query("INSERT INTO pagination_fixture VALUES(0)").execute(&mut *writer).await.unwrap();
        sqlx::query("SELECT pg_advisory_unlock($1)").bind(i64::from(key)).execute(&mut *writer).await.unwrap();
        let result=reader.await.unwrap();
        assert_eq!(result.total,21);
        assert_eq!(result.entries,(1..=20).map(|n|(n,)).collect::<Vec<_>>());
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL dedicato"]
async fn counted_media_keeps_filters_private_files_folders_and_usages_separate() {
    use talocart_platform::media::library::{Filter, MediaLibrary};
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;
        let raw=authenticated(&identity).await;
        let s=identity.session(&raw).await.unwrap().unwrap();
        let media=test_media(&pool).await;
        let lib=MediaLibrary::new(pool.clone(),identity,media.clone());
        let folder=lib.create_folder(&s,&s.csrf,None,"Paged",false).await.unwrap();
        let mut first=0;
        for n in 1..=43 {
            let id=lib.upload(&s,&s.csrf,Some(folder),include_bytes!("../fixtures/avatar.png").to_vec(),&format!("Paging-{n:03}.png"),false).await.unwrap();
            if n==1 { first=id; }
        }
        lib.upload(&s,&s.csrf,None,include_bytes!("../fixtures/avatar.png").to_vec(),"Paging-private.png",true).await.unwrap();
        let asset=lib.asset(&s,first,false).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,image_key,price_cents) SELECT 'Usage '||n,'USE-'||n,'usage-'||n,$1,100 FROM generate_series(1,103)n").bind(&asset.key).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO media_folders(parent_id,name,path) SELECT $1,'Child '||n,'Paged/paging-child-'||n FROM generate_series(1,102)n").bind(asset.folder_id).execute(&pool).await.unwrap();
        for n in 1..=102 { tokio::fs::create_dir_all(media.root().join(format!("Paged/paging-child-{n}"))).await.unwrap(); }
        let rows=lib.assets_counted(&s,&Filter { query:"Paging".into(),format:"png".into(),usage:"unused".into(),page:99,..Default::default() }).await.unwrap();
        assert_eq!((rows.total,rows.page,rows.entries.len()),(42,2,2));
        assert!(rows.entries.iter().all(|a|a.owner_id.is_none() && a.uses==0));
        let folders=lib.folders_counted(&s,Some(asset.folder_id),99,false).await.unwrap();
        assert_eq!((folders.total,folders.page,folders.entries.len()),(102,2,2));
        let usages=lib.usages_counted(&s,first,false,99).await.unwrap();
        assert_eq!((usages.total,usages.page,usages.entries.len()),(103,2,3));
        let app=router(test_app(pool,"Media".into()).await);
        let cookie=format!("talocart_session={raw}");
        let body=html(request(&app,"GET",&format!("/admin/media?q=Paging&format=png&usage=unused&sort=newest&page=99&selected={first}"),&cookie,"",false).await).await;
        assert!(body.contains("File 41–42 di 42"));
        assert!(body.contains("aria-label=\"Immagine appena selezionata\""));
        assert!(!body.contains("Paging-private"));
        assert!(body.contains("q=Paging&#38;format=png&#38;usage=unused&#38;sort=newest&#38;page=1"));
        let body=html(request(&app,"GET",&format!("/admin/media?folder={}&folders_page=99",asset.folder_id),&cookie,"",false).await).await;
        assert!(body.contains("Cartelle 101–102 di 102"));
        let body=html(request(&app,"GET",&format!("/admin/media/{first}?uses_page=99"),&cookie,"",false).await).await;
        assert!(body.contains("Utilizzi 101–103 di 103"));
        let body=html(request(&app,"GET","/admin/media?q=missing",&cookie,"",false).await).await;
        assert!(body.contains("File 0 di 0"));
    }).await;
}
