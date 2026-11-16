use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn taxonomy_crud_hierarchy_duplicates_limits_and_concurrency() {
    use talocart_commerce::{Catalog, CatalogError, Kind};
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let session=taxonomy_session(&identity).await;
        let catalog=Catalog::new(pool.clone(),identity.clone(),test_media(&pool).await);
        let a=catalog.save(&session,&session.csrf,Kind::Category,None,&taxonomy_values("Abiti","abiti",None)).await.unwrap();
        let b=catalog.save(&session,&session.csrf,Kind::Category,None,&taxonomy_values("Uomo","uomo",Some(a))).await.unwrap();
        assert!(matches!(catalog.delete(&session,&session.csrf,Kind::Category,a).await,Err(CatalogError::InUse)));
        for parent in [a,b,999999] {assert!(matches!(catalog.save(&session,&session.csrf,Kind::Category,Some(a),&taxonomy_values("Abiti","abiti",Some(parent))).await,Err(CatalogError::Hierarchy)));}
        assert_eq!(catalog.entry(&session,Kind::Category,a).await.unwrap().parent_id,None);
        assert!(!catalog.parents(&session,a).await.unwrap().iter().any(|p|p.id==a || p.id==b));
        assert!(catalog.parents(&session,b).await.unwrap().iter().any(|p|p.id==a));
        let same=taxonomy_values("Altro","abiti",None);
        assert!(matches!(catalog.save(&session,&session.csrf,Kind::Category,None,&same).await,Err(CatalogError::DuplicateSlug)));
        let c=catalog.save(&session,&session.csrf,Kind::Category,None,&taxonomy_values("Accessori","accessori",None)).await.unwrap();
        let d=catalog.save(&session,&session.csrf,Kind::Category,None,&taxonomy_values("Casa","casa",None)).await.unwrap();
        let cv=taxonomy_values("Accessori","accessori",Some(d));let dv=taxonomy_values("Casa","casa",Some(c));
        let (first,second)=tokio::join!(catalog.save(&session,&session.csrf,Kind::Category,Some(c),&cv),catalog.save(&session,&session.csrf,Kind::Category,Some(d),&dv));
        assert_eq!(usize::from(first.is_ok())+usize::from(second.is_ok()),1);
        let x=taxonomy_values("Concorrenza","concorrenza",None);
        let(one,two)=tokio::join!(catalog.save(&session,&session.csrf,Kind::Brand,None,&x),catalog.save(&session,&session.csrf,Kind::Brand,None,&x));
        assert_eq!(usize::from(one.is_ok())+usize::from(two.is_ok()),1);
        assert!(matches!(catalog.save(&session,&session.csrf,Kind::Brand,None,&taxonomy_values("CONCORRENZA","diverso",None)).await,Err(CatalogError::DuplicateName)));
        let mut parent=None;
        for n in 1..=16 {parent=Some(catalog.save(&session,&session.csrf,Kind::Category,None,&taxonomy_values(&format!("Livello {n}"),&format!("livello-{n}"),parent)).await.unwrap());}
        assert!(matches!(catalog.save(&session,&session.csrf,Kind::Category,None,&taxonomy_values("Troppo","troppo",parent)).await,Err(CatalogError::Hierarchy)));
        assert!(matches!(catalog.save(&session,&session.csrf,Kind::Category,Some(a),&taxonomy_values("Abiti","abiti",parent)).await,Err(CatalogError::Hierarchy)));
        let direct=sqlx::query("UPDATE catalog_categories SET parent_id=$1 WHERE id=$2").bind(b).bind(a).execute(&pool).await;
        assert_eq!(direct.unwrap_err().as_database_error().unwrap().code().unwrap(),"23514");
        catalog.delete(&session,&session.csrf,Kind::Category,b).await.unwrap();catalog.delete(&session,&session.csrf,Kind::Category,a).await.unwrap();
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM catalog_categories").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_categories(name,slug) SELECT 'Synthetic '||n,'synthetic-'||n FROM generate_series(1,$1) n").bind(1000-count).execute(&pool).await.unwrap();
        assert!(matches!(catalog.save(&session,&session.csrf,Kind::Category,None,&taxonomy_values("Oltre limite","oltre-limite",None)).await,Err(CatalogError::Limit)));
        assert!(catalog.list(&session,Kind::Category,"",None,1).await.unwrap().len()<=21);
        assert!(catalog.list(&session,Kind::Category,"",None,0).await.is_err());
        assert!(catalog.list(&session,Kind::Category,&"a".repeat(121),None,1).await.is_err());
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn taxonomy_http_permissions_validation_pagination_and_session_revocation() {
    use talocart_commerce::{Catalog, CatalogError, Kind};
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let catalog = Catalog::new(pool.clone(), identity.clone(), test_media(&pool).await);
        for kind in [Kind::Category, Kind::Brand] {
            let path = kind.path();
            for partial in [false, true] {
                assert_eq!(
                    request(&app, "GET", path, "", "", partial).await.status(),
                    if partial {
                        StatusCode::UNAUTHORIZED
                    } else {
                        StatusCode::SEE_OTHER
                    }
                );
            }
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("{path}/new"),
                    &cookie,
                    "csrf=bad&name=A&slug=a&sort_order=0",
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            let bad = format!(
                "csrf={}&name=&slug=INVALID&sort_order=-1&description=Preserved",
                session.csrf
            );
            let response =
                request(&app, "POST", &format!("{path}/new"), &cookie, &bad, false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body = html(response).await;
            assert!(body.contains("Preserved"));
            assert!(body.contains("aria-invalid=\"true\""));
            for n in 0..23 {
                catalog
                    .save(
                        &session,
                        &session.csrf,
                        kind,
                        None,
                        &taxonomy_values(&format!("Nome {n:02}"), &format!("nome-{n:02}"), None),
                    )
                    .await
                    .unwrap();
            }
            let body = html(request(&app, "GET", path, &cookie, "", false).await).await;
            assert_eq!(body.matches("role=\"listitem\"").count(), 20);
            assert!(body.contains("rel=\"next\""));
            assert!(body.contains("<span>Catalogo</span></summary>"));
            let body = html(
                request(
                    &app,
                    "GET",
                    &format!("{path}?q=Nome%2022"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert_eq!(body.matches("role=\"listitem\"").count(), 1);
            assert!(body.contains("Nome 22"));
            assert_eq!(
                request(&app, "GET", &format!("{path}/999999"), &cookie, "", false)
                    .await
                    .status(),
                StatusCode::NOT_FOUND
            );
            let id = catalog
                .list(&session, kind, "Nome 00", None, 1)
                .await
                .unwrap()[0]
                .id;
            let mut v = taxonomy_values("<script>test</script>", &format!("escaped-{}", id), None);
            v.active = false;
            catalog
                .save(&session, &session.csrf, kind, Some(id), &v)
                .await
                .unwrap();
            let body =
                html(request(&app, "GET", &format!("{path}/{id}"), &cookie, "", false).await).await;
            assert!(!body.contains("<script>test</script>"));
            assert!(body.contains("&#60;script&#62;"));
            assert_eq!(
                catalog
                    .list(&session, kind, "", Some(false), 1)
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }
        let role = identity
            .create_role(
                &session,
                &session.csrf,
                "Catalog reader",
                &[Permission::CategoriesRead, Permission::BrandsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &session,
                &session.csrf,
                "catalog-reader@example.test",
                "correct horse reader".into(),
                Role::Custom(role.clone()),
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let reader_raw = identity
            .login(
                &anon,
                &anon.csrf,
                "catalog-reader@example.test",
                "correct horse reader".into(),
            )
            .await
            .unwrap();
        let reader = identity.session(&reader_raw).await.unwrap().unwrap();
        let reader_cookie = format!("talocart_session={reader_raw}");
        let body = html(
            request(
                &app,
                "GET",
                "/admin/catalog/categories",
                &reader_cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(!body.contains("Nuova categoria"));
        let category = catalog
            .list(&session, Kind::Category, "", None, 1)
            .await
            .unwrap()[0]
            .id;
        let readonly = html(
            request(
                &app,
                "GET",
                &format!("/admin/catalog/categories/{category}"),
                &reader_cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(readonly.contains("readonly"));
        assert!(!readonly.contains("Salva categoria"));
        assert!(!readonly.contains("Confermo l’eliminazione"));
        let body = format!(
            "csrf={}&name=Forbidden&slug=forbidden&sort_order=0",
            reader.csrf
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/catalog/categories/new",
                &reader_cookie,
                &body,
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/catalog/categories?status=unknown",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let page_two = html(
            request(
                &app,
                "GET",
                "/admin/catalog/categories?page=2",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert_eq!(page_two.matches("role=\"listitem\"").count(), 3);
        assert!(page_two.contains("Precedente"));

        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/catalog/brands/new",
                &reader_cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert!(matches!(
            catalog
                .save(
                    &reader,
                    &reader.csrf,
                    Kind::Brand,
                    None,
                    &taxonomy_values("Non permesso", "non-permesso", None)
                )
                .await,
            Err(CatalogError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        identity
            .update_role(
                &session,
                &session.csrf,
                &role,
                "Catalog writer",
                &[Permission::CategoriesWrite],
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let writer_raw = identity
            .login(
                &anon,
                &anon.csrf,
                "catalog-reader@example.test",
                "correct horse reader".into(),
            )
            .await
            .unwrap();
        let writer = identity.session(&writer_raw).await.unwrap().unwrap();
        assert!(writer.allows(Permission::CategoriesRead));
        identity
            .update_role(
                &session,
                &session.csrf,
                &role,
                "Catalog reader",
                &[Permission::CategoriesRead],
            )
            .await
            .unwrap();
        assert!(matches!(
            catalog
                .save(
                    &writer,
                    &writer.csrf,
                    Kind::Category,
                    None,
                    &taxonomy_values("Revocato", "revocato", None)
                )
                .await,
            Err(CatalogError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/catalog/categories/new",
                &format!("talocart_session={writer_raw}"),
                "",
                false
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
async fn brand_logo_media_lifecycle_validation_cache_and_cleanup() {
    use talocart_commerce::{Catalog, Kind};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let media = test_media(&pool).await;
        let catalog = Catalog::new(pool.clone(), identity.clone(), media.clone());
        let id = catalog
            .save(
                &session,
                &session.csrf,
                Kind::Brand,
                None,
                &taxonomy_values("Koala", "koala", None),
            )
            .await
            .unwrap();
        let raw = include_bytes!("../fixtures/webp-lossless.webp").to_vec();
        catalog
            .brand_logo(
                &session,
                &session.csrf,
                id,
                Some(raw.clone()),
                "Koala Demo.webp",
            )
            .await
            .unwrap();
        let old = catalog
            .entry(&session, Kind::Brand, id)
            .await
            .unwrap()
            .logo_key
            .unwrap();
        assert_eq!(old, "brand/Koala-Demo.png");
        assert!(media.root().join(&old).is_file());
        assert_eq!(media.read(&old).await.unwrap()[..8], *b"\x89PNG\r\n\x1a\n");
        assert!(
            catalog
                .brand_logo(&session, "bad", id, None, "")
                .await
                .is_err()
        );
        assert!(
            catalog
                .brand_logo(
                    &session,
                    &session.csrf,
                    id,
                    Some(b"corrupt".to_vec()),
                    "bad.webp"
                )
                .await
                .is_err()
        );
        assert_eq!(
            catalog
                .entry(&session, Kind::Brand, id)
                .await
                .unwrap()
                .logo_key
                .as_deref(),
            Some(old.as_str())
        );
        let metadata = catalog.logo_metadata(&old).await.unwrap().unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let public_path = format!("/media/public/{}/{}/card", metadata.id, old);
        let response = image_follow(&app, &public_path, "", "GET", "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .contains("must-revalidate")
        );
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '48 hours'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(completed_prune(&media, &pool, false).await.objects, 0);
        catalog
            .brand_logo(&session, &session.csrf, id, Some(raw), "Koala Demo.webp")
            .await
            .unwrap();
        let next = catalog
            .entry(&session, Kind::Brand, id)
            .await
            .unwrap()
            .logo_key
            .unwrap();
        assert_ne!(old, next);
        let released: bool =
            sqlx::query_scalar("SELECT unused_since IS NOT NULL FROM media_objects WHERE key=$1")
                .bind(&old)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(released);
        assert_eq!(completed_prune(&media, &pool, false).await.objects, 0);
        catalog
            .delete(&session, &session.csrf, Kind::Brand, id)
            .await
            .unwrap();
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '48 hours'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            completed_prune(&media, &pool, false).await.objects,
            0,
            "Unlinked sources remain in the library"
        );
        purge_library_sources(&pool, &session, &[old.clone(), next.clone()]).await;
        assert!(completed_prune(&media, &pool, false).await.objects >= 2);
        assert!(catalog.logo_metadata(&old).await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn public_category_navigation_order_hierarchy_visibility_http_and_live_updates() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let media = test_media(&pool).await;
        let catalog =
            talocart_commerce::Catalog::new(pool.clone(), identity.clone(), media.clone());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        assert!(catalog.public_navigation().await.unwrap().is_empty());
        let empty = html(request(&app, "GET", "/", "", "", false).await).await;
        assert!(empty.contains("aria-label=\"Categorie del catalogo\""));
        assert!(empty.contains("Tutti i prodotti"));
        let (root, _, class) = product_references(&pool, &session).await;
        let create = |name: &str, slug: &str, parent, order, active| {
            let mut values = taxonomy_values(name, slug, parent);
            values.sort_order = order;
            values.active = active;
            values
        };
        let mut child = 0;
        for (name, slug, parent, order, active) in [
            ("Zeta", "zeta", None, "10", true),
            ("Alfa", "alfa", None, "0", true),
            ("Seconda", "seconda", Some(root), "2", true),
            (
                "<script>Test & categoria</script>",
                "prima",
                Some(root),
                "1",
                true,
            ),
            ("Nascosta", "nascosta", Some(root), "0", false),
        ] {
            let id = catalog
                .save(
                    &session,
                    &session.csrf,
                    talocart_commerce::Kind::Category,
                    None,
                    &create(name, slug, parent, order.into(), active),
                )
                .await
                .unwrap();
            if slug == "prima" {
                child = id;
            }
        }
        let grandchild = catalog
            .save(
                &session,
                &session.csrf,
                talocart_commerce::Kind::Category,
                None,
                &create("Terzo livello", "terzo", Some(child), "0".into(), true),
            )
            .await
            .unwrap();
        let hidden = catalog
            .save(
                &session,
                &session.csrf,
                talocart_commerce::Kind::Category,
                None,
                &create("Padre spento", "padre-spento", None, "0".into(), false),
            )
            .await
            .unwrap();
        catalog
            .save(
                &session,
                &session.csrf,
                talocart_commerce::Kind::Category,
                None,
                &create(
                    "Figlia attiva",
                    "figlia-nascosta",
                    Some(hidden),
                    "0".into(),
                    true,
                ),
            )
            .await
            .unwrap();
        let entries = catalog.public_navigation().await.unwrap();
        assert_eq!(
            entries
                .iter()
                .filter(|c| c.parent_id.is_none())
                .map(|c| c.slug.as_str())
                .collect::<Vec<_>>(),
            ["alfa", "categoria-prodotti", "zeta"]
        );
        assert_eq!(
            entries
                .iter()
                .filter(|c| c.parent_id == Some(root))
                .map(|c| c.slug.as_str())
                .collect::<Vec<_>>(),
            ["prima", "seconda"]
        );
        assert!(entries.iter().any(|c| c.id == grandchild));
        assert!(!entries.iter().any(|c| c.slug.contains("nascosta")));
        let products = talocart_commerce::products::Products::new(pool.clone(), identity, media);
        let mut values = product_values("NAV", "menu-product");
        values.category_id = child.to_string();
        values.tax_class_id = class.to_string();
        values.status = "published".into();
        products
            .save(&session, &session.csrf, None, &values)
            .await
            .unwrap();
        for path in ["/", "/status", "/catalog/prima", "/products/menu-product"] {
            let response = request(&app, "GET", path, "", "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                response.headers()[header::CACHE_CONTROL]
                    .to_str()
                    .unwrap()
                    .contains("no-store")
            );
            let body = html(response).await;
            assert_eq!(
                body.matches("aria-label=\"Categorie del catalogo\"")
                    .count(),
                1
            );
            assert!(body.contains("category-mega-panel"));
            assert!(body.contains("class=\"category-columns\""));
            assert!(body.contains("class=\"category-column-title\" href=\"/catalog/prima\""));
            assert!(body.contains("class=\"category-column-links\""));
            let third = body.find("href=\"/catalog/terzo\"").unwrap();
            let second = body.find("href=\"/catalog/seconda\"").unwrap();
            assert!(
                third < second,
                "Third level stays under its own second-level column"
            );
            assert!(body.contains("&#60;script&#62;Test &#38; categoria&#60;/script&#62;"));
            assert!(!body.contains("<script>Test"));
            assert!(!body.contains("/catalog/figlia-nascosta"));
            if path.contains("prima") || path.contains("menu-product") {
                assert!(body.contains("href=\"/catalog/prima\" aria-current=\"true\""));
                assert!(body.contains("category-menu-item is-selected"));
            }
        }
        let filtered = html(request(&app, "GET", "/catalog/prima", "", "", false).await).await;
        assert!(filtered.contains("href=\"/products/menu-product\""));
        let root_page =
            html(request(&app, "GET", "/catalog/categoria-prodotti", "", "", false).await).await;
        assert!(
            root_page.contains("href=\"/products/menu-product\""),
            "Parent category includes products assigned to its descendants"
        );
        sqlx::query("UPDATE catalog_categories SET active=false WHERE id=$1")
            .bind(root)
            .execute(&pool)
            .await
            .unwrap();
        let hidden = html(request(&app, "GET", "/", "", "", false).await).await;
        assert!(!hidden.contains("/catalog/prima"));
        sqlx::query("UPDATE catalog_categories SET active=true WHERE id=$1")
            .bind(root)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE catalog_categories_translations SET name='Nome aggiornato' WHERE entity_id=$1 AND locale='it'").bind(root).execute(&pool).await.unwrap();
        let refreshed = html(request(&app, "GET", "/", "", "", false).await).await;
        assert!(refreshed.contains("Nome aggiornato"));
        assert!(refreshed.contains("/catalog/prima"));
        // Every supported hierarchy level remains available, without additional queries per node.
        let mut parent = grandchild;
        for level in 4..=16 {
            parent = catalog
                .save(
                    &session,
                    &session.csrf,
                    talocart_commerce::Kind::Category,
                    None,
                    &create(
                        &format!("Livello {level}"),
                        &format!("livello-{level}"),
                        Some(parent),
                        "0".into(),
                        true,
                    ),
                )
                .await
                .unwrap();
        }
        assert!(
            catalog
                .public_navigation()
                .await
                .unwrap()
                .iter()
                .any(|c| c.id == parent)
        );
        assert!(
            html(request(&app, "GET", "/", "", "", false).await)
                .await
                .contains("/catalog/livello-16")
        );
        let asset = request(&app, "GET", "/assets/category-menu.js", "", "", false).await;
        assert_eq!(asset.status(), StatusCode::OK);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn taxonomy_directory_cards_preserve_contents_counts_filtering_and_read_only_actions() {
    use talocart_commerce::{Catalog, Kind};
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let catalog = Catalog::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let parent = catalog
            .save(
                &s,
                &s.csrf,
                Kind::Category,
                None,
                &taxonomy_values("Padre & <test>", "parent-card", None),
            )
            .await
            .unwrap();
        for kind in [Kind::Brand, Kind::Category] {
            let id = catalog
                .save(
                    &s,
                    &s.csrf,
                    kind,
                    None,
                    &taxonomy_values(
                        "Voce & <test>",
                        "entry-card",
                        if kind == Kind::Category {
                            Some(parent)
                        } else {
                            None
                        },
                    ),
                )
                .await
                .unwrap();
            let app = router(test_app(pool.clone(), "Directory".into()).await);
            let body = html(
                request(
                    &app,
                    "GET",
                    &format!("{}?q=entry-card", kind.path()),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(body.contains(if kind == Kind::Category {
                "catalog-list category-tree-directory"
            } else {
                "catalog-list taxonomy-grid"
            }));
            assert!(body.contains("catalog-row taxonomy-card"));
            assert!(body.contains("Voce &#38; &#60;test&#62;"));
            assert!(!body.contains("Voce & <test>"));
            assert!(body.contains("<dt>Slug</dt>"));
            assert!(body.contains("entry-card"));
            assert!(body.contains(&format!("href=\"{}/{id}\"", kind.path())));
            assert!(body.contains("ui-button--compact"));
            assert!(body.contains("Modifica Voce"));
            assert!(body.contains(if kind == Kind::Category {
                "1 categoria</p>"
            } else {
                "1 marchio</p>"
            }));
            if kind == Kind::Category {
                assert!(body.contains("Categoria superiore"));
                assert!(body.contains("Padre &#38; &#60;test&#62;"));
            } else {
                assert!(body.contains("taxonomy-monogram"));
                assert!(!body.contains("<dt>Categoria superiore</dt>"));
            }
        }
        let role = identity
            .create_role(
                &s,
                &s.csrf,
                "Consultazione directory",
                &[Permission::CategoriesRead, Permission::BrandsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "directory-reader@example.com",
                "correct horse battery".into(),
                Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let reader = format!(
            "talocart_session={}",
            login_as(&identity, "directory-reader@example.com").await
        );
        let app = router(test_app(pool, "Directory".into()).await);
        for kind in [Kind::Brand, Kind::Category] {
            let body = html(request(&app, "GET", kind.path(), &reader, "", false).await).await;
            assert!(body.contains("Consulta Voce"));
            assert!(!body.contains("Modifica Voce"));
            assert!(!body.contains(&format!("href=\"{}/new\"", kind.path())));
            let empty = html(
                request(
                    &app,
                    "GET",
                    &format!("{}?q=missing-card", kind.path()),
                    &reader,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(empty.contains("ui-empty"));
            assert!(!empty.contains("catalog-row taxonomy-card"));
            assert!(empty.contains(if kind == Kind::Category {
                "0 categorie</p>"
            } else {
                "0 marchi</p>"
            }));
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn category_tree_page_preorder_filters_inactive_ancestors_and_pagination_context() {
    use talocart_commerce::{Catalog, Kind};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let catalog = Catalog::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let mut values = taxonomy_values("Radice & <padre>", "tree-root", None);
        values.sort_order = "10".into();
        values.active = false;
        let root = catalog
            .save(&s, &s.csrf, Kind::Category, None, &values)
            .await
            .unwrap();
        let other = catalog
            .save(
                &s,
                &s.csrf,
                Kind::Category,
                None,
                &taxonomy_values("Altra radice", "tree-other", None),
            )
            .await
            .unwrap();
        let child = catalog
            .save(
                &s,
                &s.csrf,
                Kind::Category,
                None,
                &taxonomy_values("Figlio", "tree-child", Some(root)),
            )
            .await
            .unwrap();
        let grand = catalog
            .save(
                &s,
                &s.csrf,
                Kind::Category,
                None,
                &taxonomy_values("Nipote %_", "tree-grandchild", Some(child)),
            )
            .await
            .unwrap();
        let all = catalog.category_tree_page(&s, "", None, 1).await.unwrap();
        assert_eq!(all.total, 4);
        assert_eq!(
            all.entries
                .iter()
                .map(|r| (r.entry.id, r.depth, r.context))
                .collect::<Vec<_>>(),
            vec![
                (other, 0, false),
                (root, 0, false),
                (child, 1, false),
                (grand, 2, false)
            ]
        );
        let selected = catalog
            .category_tree_page(&s, "%_", Some(true), 999)
            .await
            .unwrap();
        assert_eq!((selected.total, selected.page), (1, 1));
        assert_eq!(
            selected
                .entries
                .iter()
                .map(|r| (r.entry.id, r.context))
                .collect::<Vec<_>>(),
            vec![(root, true), (child, true), (grand, false)]
        );
        assert!(!selected.entries[0].entry.active);
        assert_eq!(selected.entries[2].parent_path, "Radice & <padre> / Figlio");
        for n in 0..23 {
            catalog
                .save(
                    &s,
                    &s.csrf,
                    Kind::Category,
                    None,
                    &taxonomy_values(
                        &format!("Pagina {n:02}"),
                        &format!("tree-page-{n}"),
                        Some(root),
                    ),
                )
                .await
                .unwrap();
        }
        let second = catalog
            .category_tree_page(&s, "tree-page-", None, 2)
            .await
            .unwrap();
        assert_eq!((second.total, second.page), (23, 2));
        assert_eq!(second.entries.iter().filter(|r| !r.context).count(), 3);
        assert_eq!(second.entries[0].entry.id, root);
        assert!(second.entries[0].context);
        assert!(
            second
                .entries
                .iter()
                .skip(1)
                .all(|r| r.entry.parent_id == Some(root))
        );
        assert!(catalog.category_tree_page(&s, "", None, 0).await.is_err());
        assert!(
            catalog
                .category_tree_page(&s, "bad\n", None, 1)
                .await
                .is_err()
        );
        assert!(
            catalog
                .category_tree_page(&s, &"a".repeat(121), None, 1)
                .await
                .is_err()
        );
        let empty = catalog
            .category_tree_page(&s, "not-found", None, 999)
            .await
            .unwrap();
        assert_eq!((empty.total, empty.page, empty.entries.len()), (0, 1, 0));
        let app = router(test_app(pool, "Tree".into()).await);
        let body = html(
            request(
                &app,
                "GET",
                "/admin/catalog/categories?q=tree-grandchild&status=active",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(body.contains("1 categoria</p>"));
        assert_eq!(body.matches("data-context=\"true\"").count(), 2);
        assert_eq!(body.matches("data-context=\"false\"").count(), 1);
        assert!(body.contains("Radice &#38; &#60;padre&#62;"));
        assert!(!body.contains("Radice & <padre>"));
        assert!(body.contains("Livello 3"));
        assert!(body.contains("aria-label=\"Sottocategorie di Radice &#38; &#60;padre&#62;\""));
        let body = html(
            request(
                &app,
                "GET",
                "/admin/catalog/categories?q=tree-page-&page=2",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(body.contains("Categorie 21–23 di 23"));
        assert!(body.contains("q=tree-page-&#38;status=&#38;page=1"));
        assert!(body.contains(&format!("data-category-id=\"{root}\"")));
    })
    .await;
}
