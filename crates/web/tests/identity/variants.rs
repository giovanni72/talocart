use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn admin_variant_column_filters_are_literal_combined_and_paginated() {
    isolated(|pool, _| async move {
        let (identity, session, products, variants, product, options, ids) =
            variant_fixture(&pool).await;
        variants
            .configure(&session, &session.csrf, product, &options)
            .await
            .unwrap();
        for index in 0..45 {
            let label = if index == 0 {
                "Caffè & <M>_%".into()
            } else {
                format!("Filtro {index:02}")
            };
            let size = variants
                .save_value(&session, &session.csrf, options[1], None, &label)
                .await
                .unwrap();
            let mut values = variant_values(&format!("VFLT-{index:02}"), &[ids[0], size], "1", "0");
            values.active = index % 2 == 0;
            variants
                .save(&session, &session.csrf, product, None, &values)
                .await
                .unwrap();
        }
        let other = products
            .save(
                &session,
                &session.csrf,
                None,
                &product_values("OTHER-FILTER", "other-filter"),
            )
            .await
            .unwrap();
        variants
            .configure(&session, &session.csrf, other, &options)
            .await
            .unwrap();
        variants
            .save(
                &session,
                &session.csrf,
                other,
                None,
                &variant_values("OTHER-RED", &[ids[0], ids[2]], "1", "1"),
            )
            .await
            .unwrap();
        let cookie = format!("talocart_session={}", authenticated(&identity).await);
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = format!("/admin/catalog/products/{product}/variants");
        let filters = "combination=rosso&sku=vflt-&status=active";
        let body = html(
            request(
                &app,
                "GET",
                &format!("{path}?{filters}"),
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(
            body.contains("Varianti 1–20 di 23"),
            "All filters apply before counting and pagination"
        );
        assert!(
            body.contains("class=\"catalog-result-count muted\">23 varianti</p>"),
            "Filtered total is shown in the heading"
        );
        for size in [10, 20, 50, 100] {
            for requested in [1, 2, 999] {
                let body = html(
                    request(
                        &app,
                        "GET",
                        &format!("{path}?{filters}&per_page={size}&page={requested}"),
                        &cookie,
                        "",
                        false,
                    )
                    .await,
                )
                .await;
                let pages = (23 + size - 1) / size;
                let page = requested.min(pages);
                let first = (page - 1) * size + 1;
                let last = (page * size).min(23);
                assert!(
                    body.contains(&format!("Varianti {first}–{last} di 23")),
                    "per_page={size}, page={requested}"
                );
                assert_eq!(
                    body.matches("class=\"ui-table__link\"").count(),
                    (last - first + 1) as usize
                );
                assert!(body.contains(&format!("value=\"{size}\" selected")));
                assert!(body.contains(&format!("name=\"per_page\" value=\"{size}\"")));
                assert!(body.contains("class=\"catalog-result-count muted\">23 varianti</p>"));
            }
        }
        assert_eq!(body.matches("class=\"ui-table__link\"").count(), 20);
        assert!(body.contains(
            "combination=rosso&#38;sku=vflt-&#38;status=active&#38;per_page=20&#38;page=2"
        ));
        assert!(body.contains("class=\"ui-table__filters\""));
        for field in ["combination", "sku", "status"] {
            assert!(body.contains(&format!("name=\"{field}\"")));
        }
        assert!(body.contains("value=\"active\" selected"));
        assert!(body.contains("Caffè &#38; &#60;M&#62;_%"));
        assert!(!body.contains("OTHER-RED"));
        assert!(!body.contains(">Disattivata</span>"));
        for page in [2, 999] {
            let body = html(
                request(
                    &app,
                    "GET",
                    &format!("{path}?{filters}&page={page}"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(body.contains("Varianti 21–23 di 23"));
            assert_eq!(body.matches("class=\"ui-table__link\"").count(), 3);
            assert!(body.contains(
                "combination=rosso&#38;sku=vflt-&#38;status=active&#38;per_page=20&#38;page=1"
            ));
        }
        for (query, total) in [
            ("", 45),
            ("combination=rosso", 45),
            ("status=inactive", 22),
            ("combination=_%25", 1),
            ("sku=%25", 0),
            ("sku=_", 0),
            ("combination=rosso&sku=VFLT-00&status=inactive", 0),
        ] {
            let body =
                html(request(&app, "GET", &format!("{path}?{query}"), &cookie, "", false).await)
                    .await;
            let summary = if total == 0 {
                "Varianti 0 di 0".into()
            } else {
                format!("Varianti 1–{} di {total}", total.min(20))
            };
            assert!(body.contains(&summary), "{query}");
            let count = if total == 1 {
                "1 variante".into()
            } else {
                format!("{total} varianti")
            };
            assert!(body.contains(&format!("class=\"catalog-result-count muted\">{count}</p>")));
            assert!(body.contains("class=\"ui-table__filters\""));
            if total == 0 {
                assert!(body.contains("Nessun risultato."));
                assert!(body.contains("colspan=\"7\""));
            }
        }
        for query in [
            "per_page=0".into(),
            "per_page=21".into(),
            "per_page=101".into(),
            "per_page=-1".into(),
            "per_page=nope".into(),
            "status=published".into(),
            "combination=%0A".into(),
            format!("sku={}", "x".repeat(121)),
            "page=0".into(),
            format!("page={}", i64::MAX),
        ] {
            assert_eq!(
                request(&app, "GET", &format!("{path}?{query}"), &cookie, "", false)
                    .await
                    .status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{query}"
            );
        }
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/catalog/products/9223372036854775807/variants?status=active",
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
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn variants_options_combinations_publication_prices_and_restrictive_references() {
    use talocart_commerce::{products::ProductError, variants::VariantError};
    isolated(|pool, _| async move {
        let (_, s, products, variants, product, options, ids) = variant_fixture(&pool).await;
        assert!(!products.public_entry("model").await.unwrap().has_variants);
        variants
            .configure(&s, &s.csrf, product, &options)
            .await
            .unwrap();
        assert!(matches!(
            products.public_entry("model").await,
            Err(ProductError::NotFound)
        ));
        let red = variants
            .save(
                &s,
                &s.csrf,
                product,
                None,
                &variant_values(" red-s ", &[ids[0], ids[2]], "0,05", "2"),
            )
            .await
            .unwrap();
        let blue = variants
            .save(
                &s,
                &s.csrf,
                product,
                None,
                &variant_values("BLUE-M", &[ids[1], ids[3]], "0,10", "0"),
            )
            .await
            .unwrap();
        assert_eq!(variants.entry(&s, product, red).await.unwrap().sku, "RED-S");
        assert!(products.public_entry("model").await.unwrap().has_variants);
        assert_eq!(products.public_entry("model").await.unwrap().price_cents, 5);
        assert!(matches!(
            variants
                .save(
                    &s,
                    &s.csrf,
                    product,
                    None,
                    &variant_values("OTHER", &[ids[2], ids[0]], "1", "1")
                )
                .await,
            Err(VariantError::DuplicateCombination)
        ));
        let matching = variants
            .save(
                &s,
                &s.csrf,
                product,
                None,
                &variant_values("MODEL", &[ids[0], ids[3]], "1", "1"),
            )
            .await
            .unwrap();
        assert_eq!(
            variants.entry(&s, product, matching).await.unwrap().sku,
            "MODEL"
        );
        // Independent identifiers may have the same value in either direction.
        let simple = products
            .save(&s, &s.csrf, None, &product_values("RED-S", "sku-collision"))
            .await
            .unwrap();
        assert_eq!(products.entry(&s, simple).await.unwrap().model, "RED-S");
        variants
            .delete(&s, &s.csrf, product, matching)
            .await
            .unwrap();
        for values in [
            vec![ids[0]],
            vec![ids[0], ids[1]],
            vec![ids[0], 999999],
            vec![ids[0], ids[0]],
        ] {
            assert!(matches!(
                variants
                    .save(
                        &s,
                        &s.csrf,
                        product,
                        None,
                        &variant_values("INVALID", &values, "1", "1")
                    )
                    .await,
                Err(VariantError::Combination)
            ));
        }
        assert!(
            variants
                .public_selection(product, &[ids[0], 0])
                .await
                .unwrap()
                .1
                .is_none()
        );
        assert!(
            variants
                .public_selection(product, &[ids[0], ids[3]])
                .await
                .unwrap()
                .1
                .is_none()
        );
        assert!(
            variants
                .public_selection(product, &[ids[2], ids[0]])
                .await
                .unwrap()
                .1
                .is_none()
        );
        let selected = variants
            .public_selection(product, &[ids[0], ids[2]])
            .await
            .unwrap()
            .1
            .unwrap();
        assert_eq!((selected.price_cents, selected.stock), (5, 2));
        assert!(matches!(
            variants.configure(&s, &s.csrf, product, &[]).await,
            Err(VariantError::InUse)
        ));
        assert!(matches!(
            variants.delete_value(&s, &s.csrf, options[0], ids[0]).await,
            Err(VariantError::InUse)
        ));
        assert!(matches!(
            variants.delete_option(&s, &s.csrf, options[0]).await,
            Err(VariantError::InUse)
        ));
        assert_eq!(
            variants.option_values(&s, options[0]).await.unwrap().len(),
            2,
            "Failed option deletion rolls back value deletions"
        );
        assert!(matches!(
            variants.save_option(&s, &s.csrf, None, "colore").await,
            Err(VariantError::DuplicateName)
        ));
        assert!(matches!(
            variants
                .save_value(&s, &s.csrf, options[0], None, "rosso")
                .await,
            Err(VariantError::DuplicateName)
        ));
        variants
            .save_value(&s, &s.csrf, options[0], Some(ids[0]), "Rosso nuovo")
            .await
            .unwrap();
        assert!(
            variants
                .entry(&s, product, red)
                .await
                .unwrap()
                .label
                .contains("Rosso nuovo")
        );
        let mut values = variants
            .entry(&s, product, red)
            .await
            .unwrap()
            .values("inclusive");
        values.active = false;
        variants
            .save(&s, &s.csrf, product, Some(red), &values)
            .await
            .unwrap();
        let public = products.public_entry("model").await.unwrap();
        assert_eq!((public.price_cents, public.stock), (10, 0));
        assert!(
            variants
                .public_selection(product, &[ids[0], ids[2]])
                .await
                .unwrap()
                .1
                .is_none()
        );
        variants.delete(&s, &s.csrf, product, blue).await.unwrap();
        assert!(matches!(
            products.public_entry("model").await,
            Err(ProductError::NotFound)
        ));
        let restarted = talocart_commerce::variants::Variants::new(
            pool.clone(),
            Identity::new(pool.clone()),
            test_media(&pool).await,
        );
        assert_eq!(
            restarted.entry(&s, product, red).await.unwrap().price_cents,
            5
        );
        products.delete(&s, &s.csrf, product).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_variants")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_variant_values")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        variants
            .delete_option(&s, &s.csrf, options[0])
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn variants_http_csrf_validation_ownership_selection_and_pagination() {
    isolated(|pool,_|async move{
 let (identity,s,products,variants,product,options,ids)=variant_fixture(&pool).await;let raw=authenticated(&identity).await;let session=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");let app=router(test_app(pool.clone(),"Shop".into()).await);
 let new=format!("/admin/catalog/products/{product}/variants/new");assert_eq!(request(&app,"GET",&new,"","",false).await.status(),StatusCode::SEE_OTHER);
 for uri in ["/admin/catalog/options/new".to_string(),format!("/admin/catalog/products/{product}/options")] {let body=if uri.ends_with("options"){"csrf=wrong&option_1=1"}else{"csrf=wrong&name=Color"};assert_eq!(request(&app,"POST",&uri,&cookie,body,true).await.status(),StatusCode::FORBIDDEN);}
 assert_eq!(request(&app,"POST",&format!("/admin/catalog/products/{product}/options"),&cookie,&format!("csrf={}&option_1={}&option_2={}",session.csrf,options[0],options[1]),false).await.status(),StatusCode::SEE_OTHER);
 let body=format!("csrf={}&sku=RED-S&price=0.05&price_mode=inclusive&stock=2&active=true&value_1={}&value_2={}",session.csrf,ids[0],ids[2]);let response=request(&app,"POST",&new,&cookie,&body,true).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);let path=response.headers()[header::LOCATION].to_str().unwrap().split('?').next().unwrap().to_owned();
 assert_eq!(request(&app,"POST",&path,&cookie,&body,false).await.status(),StatusCode::CONFLICT,"Missing stock revision must reload");
 assert_eq!(request(&app,"POST",&path,&cookie,&format!("{body}&stock_revision=0"),false).await.status(),StatusCode::SEE_OTHER);
 let selected=format!("/products/model?value_1={}&value_2={}",ids[0],ids[2]);let response=request(&app,"GET",&selected,"","",true).await;assert_eq!(response.status(),StatusCode::OK);assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");let body=html(response).await;assert!(body.contains("0,05 €"));assert!(body.contains("RED-S"));assert!(body.contains("0,03 €"));assert!(body.contains("0,02 €"));assert!(body.contains("hx-select=\"#product-detail\""));
 let body=html(request(&app,"GET","/products/model?value_1=999999&value_2=999999","","",false).await).await;assert!(body.contains("Combinazione non disponibile"));assert!(!body.contains("RED-S"));
 let response=request(&app,"POST",&new,&cookie,&format!("csrf={}&sku=RETAIN&price=0.001&price_mode=inclusive&stock=-1&active=true&value_1={}&value_2={}",session.csrf,ids[1],ids[3]),false).await;assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let body=html(response).await;assert!(body.contains("value=\"RETAIN\""));assert!(body.contains("aria-invalid=\"true\""));
 let response=request(&app,"POST",&new,&cookie,&format!("csrf={}&sku=RED-S&price=1&price_mode=inclusive&stock=1&active=true&value_1={}&value_2={}",session.csrf,ids[1],ids[3]),false).await;assert_eq!(response.status(),StatusCode::CONFLICT);assert!(html(response).await.contains("id=\"variant-sku\""));
 let other=products.save(&s,&s.csrf,None,&product_values("OTHER","other")).await.unwrap();let id=path.split('/').next_back().unwrap();assert_eq!(request(&app,"GET",&format!("/admin/catalog/products/{other}/variants/{id}"),&cookie,"",false).await.status(),StatusCode::NOT_FOUND);
 assert_eq!(request(&app,"POST",&format!("{path}/delete"),&cookie,&format!("csrf={}",session.csrf),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);assert_eq!(request(&app,"GET",&format!("{path}/delete"),&cookie,"",false).await.status(),StatusCode::METHOD_NOT_ALLOWED);
 // Values and option names are plain escaped text, even after rename.
 variants.save_option(&s,&s.csrf,Some(options[0]),"<b>Colore</b>").await.unwrap();let body=html(request(&app,"GET",&path,&cookie,"",false).await).await;assert!(body.contains("&#60;b&#62;Colore"));assert!(!body.contains("<b>Colore"));
 let o=variants.save_option(&s,&s.csrf,None,"Numbers").await.unwrap();let values:Vec<i64>=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Value '||n FROM generate_series(1,22) n RETURNING id").bind(o).fetch_all(&pool).await.unwrap();variants.configure(&s,&s.csrf,other,&[o]).await.unwrap();for (n,id) in values.iter().enumerate(){variants.save(&s,&s.csrf,other,None,&variant_values(&format!("PAGED-{n}"),&[*id],"1","0")).await.unwrap();}
 let listing=html(request(&app,"GET","/admin/catalog/products",&cookie,"",false).await).await;assert!(listing.contains("varianti di Prodotto demo\">22</a>"),"Count covers all pages");
 for(page,count)in[(1,20),(2,2)]{let body=html(request(&app,"GET",&format!("/admin/catalog/products/{other}/variants?page={page}"),&cookie,"",false).await).await;assert_eq!(body.matches("scope=\"row\"").count(),count);}
 sqlx::query("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Value '||n FROM generate_series(23,52) n").bind(o).execute(&pool).await.unwrap();for(page,count)in[(1,50),(2,2)]{let body=html(request(&app,"GET",&format!("/admin/catalog/options/{o}?page={page}"),&cookie,"",false).await).await;assert_eq!(body.matches("class=\"ui-edit-list__row option-value-row\"").count(),count);}
 let last=variants.option_values(&s,o).await.unwrap().last().unwrap().id;let invalid="x".repeat(81);let response=request(&app,"POST",&format!("/admin/catalog/options/{o}/values/{last}"),&cookie,&format!("csrf={}&name={invalid}",session.csrf),false).await;assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let body=html(response).await;assert!(body.contains(&format!("value=\"{invalid}\"")),"Invalid value on page two remains visible with its error");
 let response=request(&app,"POST",&format!("/admin/catalog/options/{o}/values/{last}"),&cookie,&format!("csrf={}&name=ZZZ",session.csrf),false).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);assert_eq!(response.headers()[header::LOCATION],format!("/admin/catalog/options/{o}?saved=1&page=2"));

 }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn variants_permissions_live_revocation_images_ownership_cache_and_cleanup() {
    use talocart_commerce::variants::VariantError;
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let (identity, owner, products, variants, product, options, ids) =
            variant_fixture(&pool).await;
        variants
            .configure(&owner, &owner.csrf, product, &options)
            .await
            .unwrap();
        let id = variants
            .save(
                &owner,
                &owner.csrf,
                product,
                None,
                &variant_values("RED-S", &[ids[0], ids[2]], "1", "3"),
            )
            .await
            .unwrap();
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Variants editor",
                &[Permission::ProductsWrite],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "variants@example.test",
                "correct horse variants".into(),
                Role::Custom(role.clone()),
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anon,
                &anon.csrf,
                "variants@example.test",
                "correct horse variants".into(),
            )
            .await
            .unwrap();
        let writer = identity.session(&raw).await.unwrap().unwrap();
        assert!(!writer.allows(Permission::TaxesRead));
        assert!(!writer.allows(Permission::CategoriesRead));
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = format!("/admin/catalog/products/{product}/variants/{id}");
        assert_eq!(
            request(&app, "GET", &path, &cookie, "", false)
                .await
                .status(),
            StatusCode::OK
        );
        variants
            .save_option(&writer, &writer.csrf, None, "Allowed")
            .await
            .unwrap();
        let other = products
            .save(&owner, &owner.csrf, None, &product_values("OTHER", "other"))
            .await
            .unwrap();
        assert!(matches!(
            variants
                .image(
                    &writer,
                    &writer.csrf,
                    other,
                    id,
                    Some(include_bytes!("../fixtures/webp-lossy.webp").to_vec()),
                    "Photo.webp"
                )
                .await,
            Err(VariantError::NotFound)
        ));
        products
            .image(
                &owner,
                &owner.csrf,
                product,
                Some(include_bytes!("../fixtures/webp-lossless.webp").to_vec()),
                "Parent.webp",
            )
            .await
            .unwrap();
        let parent = products
            .entry(&owner, product)
            .await
            .unwrap()
            .image_url("detail");
        let selected = format!("/products/model?value_1={}&value_2={}", ids[0], ids[2]);
        let body = html(request(&app, "GET", &selected, "", "", false).await).await;
        assert!(
            body.contains(&parent),
            "No variant image falls back to parent"
        );
        variants
            .image(
                &writer,
                &writer.csrf,
                product,
                id,
                Some(include_bytes!("../fixtures/webp-lossy.webp").to_vec()),
                "Real Photo.webp",
            )
            .await
            .unwrap();
        let entry = variants.entry(&writer, product, id).await.unwrap();
        let old = entry.image_key.clone().unwrap();
        assert_eq!(old, "Real-Photo.png");
        let url = entry.image_url("detail");
        let response = image_follow(&app, &url, "", "GET", "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .contains("must-revalidate")
        );
        assert!(
            html(request(&app, "GET", &selected, "", "", false).await)
                .await
                .contains(&url)
        );
        assert!(
            variants
                .image(&writer, "bad", product, id, None, "")
                .await
                .is_err()
        );
        assert!(
            variants
                .image(
                    &writer,
                    &writer.csrf,
                    product,
                    id,
                    Some(include_bytes!("../fixtures/webp-animated.webp").to_vec()),
                    "Bad.webp"
                )
                .await
                .is_err()
        );
        assert_eq!(
            variants
                .entry(&writer, product, id)
                .await
                .unwrap()
                .image_key
                .as_deref(),
            Some(old.as_str())
        );
        variants
            .image(
                &writer,
                &writer.csrf,
                product,
                id,
                Some(include_bytes!("../fixtures/webp-lossless.webp").to_vec()),
                "Real Photo.webp",
            )
            .await
            .unwrap();
        let new = variants
            .entry(&writer, product, id)
            .await
            .unwrap()
            .image_key
            .unwrap();
        assert_ne!(old, new);
        let media = test_media(&pool).await;
        assert_eq!(completed_prune(&media, &pool, false).await.objects, 0);
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '48 hours'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            completed_prune(&media, &pool, false).await.objects,
            0,
            "Replaced variant sources remain in the library"
        );
        purge_library_sources(&pool, &owner, std::slice::from_ref(&old)).await;
        assert!(completed_prune(&media, &pool, false).await.objects >= 1);
        assert!(media.read(&new).await.is_ok());
        assert!(
            media
                .read(
                    &products
                        .entry(&owner, product)
                        .await
                        .unwrap()
                        .image_key
                        .unwrap()
                )
                .await
                .is_ok()
        );
        identity
            .update_role(
                &owner,
                &owner.csrf,
                &role,
                "Variants reader",
                &[Permission::ProductsRead],
            )
            .await
            .unwrap();
        assert!(matches!(
            variants
                .save_option(&writer, &writer.csrf, None, "Revoked")
                .await,
            Err(VariantError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert_eq!(
            request(&app, "GET", &path, &cookie, "", false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        let (_, anon) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anon,
                &anon.csrf,
                "variants@example.test",
                "correct horse variants".into(),
            )
            .await
            .unwrap();
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let body = html(request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(body.contains("readonly"));
        assert!(!body.contains("Salva variante"));
        assert!(!body.contains("type=\"file\""));
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("/admin/catalog/products/{product}/variants/new"),
                &cookie,
                "",
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
                &format!("{path}/image/remove"),
                &cookie,
                &format!("csrf={}", reader.csrf),
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        variants
            .delete(&owner, &owner.csrf, product, id)
            .await
            .unwrap();
        assert_eq!(completed_prune(&media, &pool, false).await.objects, 0);
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '48 hours'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            completed_prune(&media, &pool, false).await.objects,
            0,
            "Removed variants retain library sources"
        );
        purge_library_sources(&pool, &owner, std::slice::from_ref(&new)).await;
        assert!(completed_prune(&media, &pool, false).await.objects >= 1);
        assert!(media.read(&new).await.is_err());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn variants_unlimited_cardinality_and_independent_code_concurrency() {
    isolated(|pool,_|async move{
 let (_,s,products,variants,product,options,ids)=variant_fixture(&pool).await;variants.configure(&s,&s.csrf,product,&options).await.unwrap();
 // Product models and variant SKUs have independent uniqueness, including concurrent writes.
 let (a,b)=tokio::join!(sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents,stock) VALUES('Concurrent','SHARED','shared',0,0)").execute(&pool),sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,$2,'SHARED',0,0)").bind(product).bind(vec![ids[0],ids[2]]).execute(&pool));assert!(a.is_ok() && b.is_ok(), "A model may equal a variant SKU");
 assert!(sqlx::query("DELETE FROM catalog_product_options WHERE product_id=$1").bind(product).execute(&pool).await.is_err(),"Configured options remain frozen while variants exist");
 // Independent writers may both cross the former cardinality boundaries.
 let numbers=variants.save_option(&s,&s.csrf,None,"Numbers").await.unwrap();let vals:Vec<i64>=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Number '||n FROM generate_series(1,99)n RETURNING id").bind(numbers).fetch_all(&pool).await.unwrap();
 let(a,b)=tokio::join!(sqlx::query("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Last A')").bind(numbers).execute(&pool),sqlx::query("INSERT INTO catalog_option_values(option_id,name) VALUES($1,'Last B')").bind(numbers).execute(&pool));assert!(a.is_ok() && b.is_ok());
 let p=products.save(&s,&s.csrf,None,&product_values("CAPACITY","capacity")).await.unwrap();variants.configure(&s,&s.csrf,p,&[options[0],numbers]).await.unwrap();
 // Three axes supply more than 500 explicit combinations.
 let third=variants.save_option(&s,&s.csrf,None,"Third").await.unwrap();let third_values:Vec<i64>=sqlx::query_scalar("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Third '||n FROM generate_series(1,3)n RETURNING id").bind(third).fetch_all(&pool).await.unwrap();variants.configure(&s,&s.csrf,p,&[options[0],numbers,third]).await.unwrap();
 sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) SELECT $1,ARRAY[c.id,n.id,t.id],'LIMIT-'||row_number() OVER(),0,0 FROM catalog_option_values c,catalog_option_values n,catalog_option_values t WHERE c.option_id=$2 AND n.option_id=$3 AND t.option_id=$4 LIMIT 499").bind(p).bind(options[0]).bind(numbers).bind(third).execute(&pool).await.unwrap();
 let missing:Vec<Vec<i64>>=sqlx::query_scalar("SELECT ARRAY(SELECT x FROM unnest(ARRAY[c.id,n.id,t.id])x ORDER BY x) FROM catalog_option_values c,catalog_option_values n,catalog_option_values t WHERE c.option_id=$1 AND n.option_id=$2 AND t.option_id=$3 AND NOT EXISTS(SELECT 1 FROM catalog_variants v WHERE v.product_id=$4 AND v.value_ids=ARRAY(SELECT x FROM unnest(ARRAY[c.id,n.id,t.id])x ORDER BY x)) LIMIT 2").bind(options[0]).bind(numbers).bind(third).bind(p).fetch_all(&pool).await.unwrap();
 let(a,b)=tokio::join!(sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,$2,'LAST-A',0,0)").bind(p).bind(&missing[0]).execute(&pool),sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) VALUES($1,$2,'LAST-B',0,0)").bind(p).bind(&missing[1]).execute(&pool));assert!(a.is_ok() && b.is_ok());assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_variants WHERE product_id=$1").bind(p).fetch_one(&pool).await.unwrap(),501);
 let option_count:i64=sqlx::query_scalar("SELECT count(*) FROM catalog_options").fetch_one(&pool).await.unwrap();for n in option_count..99{variants.save_option(&s,&s.csrf,None,&format!("Capacity {n}")).await.unwrap();}
 let(a,b)=tokio::join!(sqlx::query("INSERT INTO catalog_options(name) VALUES('Last A')").execute(&pool),sqlx::query("INSERT INTO catalog_options(name) VALUES('Last B')").execute(&pool));assert!(a.is_ok() && b.is_ok());
 assert_eq!(variants.options(&s).await.unwrap().len(),101);
 assert_eq!(variants.option_values(&s,numbers).await.unwrap().len(),101);
 assert_eq!(vals.len(),99);assert_eq!(third_values.len(),3);
 }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn variant_configuration_cannot_move_to_another_product_through_sql() {
    isolated(|pool,_|async move{
 let(_,s,products,variants,product,options,ids)=variant_fixture(&pool).await;variants.configure(&s,&s.csrf,product,&options).await.unwrap();variants.save(&s,&s.csrf,product,None,&variant_values("RED-S",&[ids[0],ids[2]],"1","1")).await.unwrap();let other=products.save(&s,&s.csrf,None,&product_values("OTHER","other")).await.unwrap();
 assert!(sqlx::query("UPDATE catalog_product_options SET product_id=$1 WHERE product_id=$2 AND position=1").bind(other).bind(product).execute(&pool).await.is_err(),"An axis with variants must not move to another product");
 assert_eq!(variants.configuration(&s,product).await.unwrap().axes.len(),2);assert!(variants.configuration(&s,other).await.unwrap().axes.is_empty());
 }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn variant_filters_and_sitemap_follow_active_stock_not_parent_stock() {
    isolated(|pool, _| async move {
        let (_, s, products, variants, product, options, ids) = variant_fixture(&pool).await;
        variants
            .configure(&s, &s.csrf, product, &options)
            .await
            .unwrap();
        let mut empty = variant_values("EMPTY-ACTIVE", &[ids[0], ids[2]], "5.00", "0");
        let first = variants
            .save(&s, &s.csrf, product, None, &empty)
            .await
            .unwrap();
        let mut stocked = variant_values("STOCKED-INACTIVE", &[ids[1], ids[3]], "1.00", "99");
        stocked.active = false;
        let second = variants
            .save(&s, &s.csrf, product, None, &stocked)
            .await
            .unwrap();
        assert!(products.entry(&s, product).await.unwrap().stock > 0);
        assert_eq!(
            products.public_entry("model").await.unwrap().price_cents,
            500
        );
        assert!(
            products
                .public_filtered("", "", "", true, 1)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            products
                .public_list("STOCKED-INACTIVE", "", 1)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(products.sitemap_products(1).await.unwrap(), ["model"]);

        stocked.active = true;
        variants
            .save(&s, &s.csrf, product, Some(second), &stocked)
            .await
            .unwrap();
        let visible = products
            .public_filtered(
                "stocked-inactive",
                "categoria-prodotti",
                "marchio-prodotti",
                true,
                1,
            )
            .await
            .unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!((visible[0].price_cents, visible[0].stock), (100, 99));
        empty.active = false;
        stocked.active = false;
        variants
            .save(&s, &s.csrf, product, Some(first), &empty)
            .await
            .unwrap();
        variants
            .save(&s, &s.csrf, product, Some(second), &stocked)
            .await
            .unwrap();
        assert!(products.sitemap_products(1).await.unwrap().is_empty());
        assert_eq!(products.sitemap_count().await.unwrap(), 0);
        assert!(products.public_list("", "", 1).await.unwrap().is_empty());
        let app = router(test_app(pool, "Shop".into()).await);
        assert_eq!(
            request(&app, "GET", "/products/model", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(&app, "GET", "/sitemap-products.xml", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        let index = html(request(&app, "GET", "/sitemap.xml", "", "", false).await).await;
        assert!(index.contains("/sitemap-catalog.xml"));
        assert!(!index.contains("/sitemap-products.xml"));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn three_option_selection_respects_axis_order_and_rejects_foreign_values() {
    isolated(|pool, _| async move {
        let (_, s, _, variants, product, options, ids) = variant_fixture(&pool).await;
        let material = variants
            .save_option(&s, &s.csrf, None, "Materiale")
            .await
            .unwrap();
        let cotton = variants
            .save_value(&s, &s.csrf, material, None, "Cotone")
            .await
            .unwrap();
        // Axis order deliberately differs from option/value ID order.
        variants
            .configure(&s, &s.csrf, product, &[material, options[1], options[0]])
            .await
            .unwrap();
        let variant = variants
            .save(
                &s,
                &s.csrf,
                product,
                None,
                &variant_values("THREE", &[ids[0], cotton, ids[2]], "12.34", "1"),
            )
            .await
            .unwrap();
        let chosen = [cotton, ids[2], ids[0]];
        let (axes, selection) = variants.public_selection(product, &chosen).await.unwrap();
        assert_eq!(
            axes.iter().map(|a| a.id).collect::<Vec<_>>(),
            [material, options[1], options[0]]
        );
        let selection = selection.unwrap();
        assert_eq!(selection.id, variant);
        assert_eq!(
            selection.label,
            "Materiale: Cotone / Taglia: S / Colore: Rosso"
        );
        for invalid in [
            vec![],
            vec![cotton],
            vec![ids[0], ids[2], cotton],
            vec![cotton, ids[2], i64::MAX],
            vec![cotton, ids[3], ids[0]],
        ] {
            assert!(
                variants
                    .public_selection(product, &invalid)
                    .await
                    .unwrap()
                    .1
                    .is_none()
            );
        }
        let app = router(test_app(pool, "Shop".into()).await);
        for partial in [false, true] {
            let response = request(
                &app,
                "GET",
                &format!(
                    "/products/model?value_1={cotton}&value_2={}&value_3={}",
                    ids[2], ids[0]
                ),
                "",
                "",
                partial,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let page = html(response).await;
            assert!(page.contains("12,34"));
            assert!(page.contains("THREE"));
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn unlimited_options_values_and_variants_remain_reachable_over_old_limits() {
    use talocart_commerce::variants::VariantError;
    isolated(|pool, _| async move {
        let (identity, s, _, variants, product, options, ids) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        sqlx::query("INSERT INTO catalog_options(name) SELECT 'Extra '||lpad(n::text,3,'0') FROM generate_series(1,98)n").execute(&pool).await.unwrap();
        for name in ["ZZ Beyond 100", "ZZ Beyond 101"] {
            let response = request(&app, "POST", "/admin/catalog/options/new", &cookie, &format!("csrf={}&name={name}", session.csrf), false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER, "New options have no catalog cap");
        }
        assert_eq!(variants.options(&s).await.unwrap().len(), 102);
        assert_eq!(variants.options_page(&s,1).await.unwrap().len(),21);
        for base in ["/admin/catalog/options".to_string(),format!("/admin/catalog/options/{}",options[0]),format!("/admin/catalog/products/{product}/variants")] {
            for page in [0,-1,i64::MAX] {
                assert_eq!(request(&app,"GET",&format!("{base}?page={page}"),&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
            }
            assert_eq!(request(&app,"GET",&format!("{base}?page=1000001"),&cookie,"",false).await.status(),StatusCode::OK);
        }

        let body = html(request(&app,"GET","/admin/catalog/options?page=6",&cookie,"",false).await).await;
        assert_eq!(body.matches("role=\"listitem\"").count(),2);
        assert!(body.contains("ZZ Beyond 101"));
        assert!(!body.contains("rel=\"next\""));
        let body = html(request(&app,"GET",&format!("/admin/catalog/products/{product}"),&cookie,"",false).await).await;
        assert_eq!(body.matches(">ZZ Beyond 101</option>").count(),3,"All three selectors reach the final option");
        let third = variants.options(&s).await.unwrap().last().unwrap().id;
        variants.save_value(&s,&s.csrf,third,None,"Third value").await.unwrap();
        assert!(matches!(variants.configure(&s,&s.csrf,product,&[options[0],options[1],third,third-1]).await,Err(VariantError::Options)));
        assert!(variants.configuration(&s,product).await.unwrap().axes.is_empty());
        variants.configure(&s,&s.csrf,product,&[options[0],options[1],third]).await.unwrap();
        assert!(sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,4,$2)").bind(product).bind(third-1).execute(&pool).await.is_err());
        sqlx::query("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Value '||lpad(n::text,3,'0') FROM generate_series(1,503)n").bind(options[0]).execute(&pool).await.unwrap();
        let value = variants.option_values(&s,options[0]).await.unwrap().last().unwrap().id;
        assert_eq!(variants.option_values(&s,options[0]).await.unwrap().len(),505);
        let conf=variants.configuration(&s,product).await.unwrap();
        assert_eq!(conf.axes.iter().map(|a|a.values.len()).collect::<Vec<_>>(),vec![505,2,1],"No truncation at 300 or loss of later axes");
        let third_value=conf.axes[2].values[0].id;
        sqlx::query("INSERT INTO catalog_variants(product_id,value_ids,sku,price_cents,stock) SELECT $1,ARRAY[v.id,$3,$4],'BULK-'||lpad(row_number() OVER(ORDER BY v.id)::text,3,'0'),100,10 FROM catalog_option_values v WHERE v.option_id=$2 ORDER BY v.id LIMIT 501").bind(product).bind(options[0]).bind(ids[2]).bind(third_value).execute(&pool).await.unwrap();
        let new_uri=format!("/admin/catalog/products/{product}/variants/new");
        let body=html(request(&app,"GET",&new_uri,&cookie,"",false).await).await;
        assert!(body.contains(">Value 503</option>"));
        assert!(body.contains(">Third value</option>"));
        let response=request(&app,"POST",&new_uri,&cookie,&format!("csrf={}&sku=BEYOND-501&price=1&price_mode=inclusive&stock=10&active=true&value_1={value}&value_2={}&value_3={third_value}",session.csrf,ids[2]),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(variants.configuration(&s,product).await.unwrap().count,502);
        let body=html(request(&app,"GET",&format!("/admin/catalog/products/{product}/variants?page=26"),&cookie,"",false).await).await;
        assert_eq!(body.matches("scope=\"row\"").count(),2);
        assert!(body.contains("BEYOND-501"));
        for htmx in [false,true] {
            let body=html(request(&app,"GET",&format!("/products/model?value_1={value}&value_2={}&value_3={third_value}",ids[2]),"","",htmx).await).await;
            assert!(body.contains("BEYOND-501"));
            assert!(body.contains(">Value 503</option>"));
            assert!(body.contains(">Third value</option>"));
        }
        let uri=format!("/admin/catalog/options/{}/values/{value}",options[0]);
        let invalid="x".repeat(81);
        let response=request(&app,"POST",&uri,&cookie,&format!("csrf={}&name={invalid}",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        let body=html(response).await;
        assert!(body.contains("Pagina 11"));
        assert!(body.contains(&format!("value=\"{invalid}\"")));
        assert_eq!(body.matches("class=\"ui-edit-list__row option-value-row\"").count(),5);
        let response=request(&app,"POST",&uri,&cookie,&format!("csrf={}&name=AAA",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION],format!("/admin/catalog/options/{}?saved=1",options[0]));
        let response=request(&app,"POST",&format!("/admin/catalog/options/{}/values",options[0]),&cookie,&format!("csrf={}&name=ZZ Last",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION],format!("/admin/catalog/options/{}?saved=1&page=11",options[0]));
        let body=html(request(&app,"GET",&format!("/admin/catalog/options/{}?page=11",options[0]),&cookie,"",false).await).await;
        assert_eq!(body.matches("class=\"ui-edit-list__row option-value-row\"").count(),6);
        assert!(!body.contains("Massimo 100"));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn public_options_only_include_values_used_by_this_products_active_variants() {
    isolated(|pool, _| async move {
        let (_, s, products, variants, product, options, ids) = variant_fixture(&pool).await;
        variants
            .configure(&s, &s.csrf, product, &options)
            .await
            .unwrap();
        variants
            .save(
                &s,
                &s.csrf,
                product,
                None,
                &variant_values("LOCAL-EMPTY", &[ids[0], ids[2]], "1", "0"),
            )
            .await
            .unwrap();
        let mut inactive = variant_values("LOCAL-INACTIVE", &[ids[1], ids[3]], "1", "10");
        inactive.active = false;
        variants
            .save(&s, &s.csrf, product, None, &inactive)
            .await
            .unwrap();
        let other = products
            .save(&s, &s.csrf, None, &product_values("OTHER", "other"))
            .await
            .unwrap();
        variants
            .configure(&s, &s.csrf, other, &options)
            .await
            .unwrap();
        variants
            .save(
                &s,
                &s.csrf,
                other,
                None,
                &variant_values("OTHER-ACTIVE", &[ids[1], ids[3]], "1", "10"),
            )
            .await
            .unwrap();
        let unused = variants
            .save_value(&s, &s.csrf, options[0], None, "Unused global")
            .await
            .unwrap();
        let (axes, _) = variants.public_selection(product, &[]).await.unwrap();
        assert_eq!(
            axes.iter()
                .map(|a| a.values.iter().map(|v| v.id).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            vec![vec![ids[0]], vec![ids[2]]]
        );
        let admin = variants.configuration(&s, product).await.unwrap();
        assert!(admin.axes[0].values.iter().any(|v| v.id == unused));
        assert!(admin.axes[0].values.iter().any(|v| v.id == ids[1]));
        assert!(
            variants
                .public_selection(product, &[ids[1], ids[3]])
                .await
                .unwrap()
                .1
                .is_none()
        );
        assert!(
            variants
                .public_selection(product, &[ids[0], ids[2]])
                .await
                .unwrap()
                .1
                .is_some()
        );
        let app = router(test_app(pool, "Shop".into()).await);
        for partial in [false, true] {
            for query in [
                String::new(),
                format!("?value_1={}&value_2={}", ids[0], ids[2]),
                format!("?value_1={}&value_2={}", ids[1], ids[3]),
            ] {
                let response = request(
                    &app,
                    "GET",
                    &format!("/products/model{query}"),
                    "",
                    "",
                    partial,
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let body = html(response).await;
                for id in [ids[0], ids[2]] {
                    assert!(body.contains(&format!("<option value=\"{id}\"")));
                }
                for id in [ids[1], ids[3], unused] {
                    assert!(!body.contains(&format!("<option value=\"{id}\"")));
                }
            }
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn model_sku_namespaces_remain_independent_and_individually_unique() {
    isolated(|pool, _| async move {
        let (_, s, products, variants, product, options, ids) = variant_fixture(&pool).await;
        variants.configure(&s, &s.csrf, product, &options).await.unwrap();
        let columns: Vec<String> = sqlx::query_scalar("SELECT column_name FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='catalog_products'").fetch_all(&pool).await.unwrap();
        assert!(columns.iter().any(|c| c == "model"));
        assert!(!columns.iter().any(|c| c == "sku"));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM pg_trigger WHERE tgrelid='catalog_products'::regclass AND tgname='product_variant_sku_guard'").fetch_one(&pool).await.unwrap(), 0);
        for table in ["guest_cart_items", "order_lines"] {
            let columns: Vec<String> = sqlx::query_scalar("SELECT column_name FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1").bind(table).fetch_all(&pool).await.unwrap();
            assert!(columns.iter().any(|c| c == "code"), "{table}: generic sale-line code");
            assert!(!columns.iter().any(|c| c == "sku"));
        }
        let same = variants.save(&s, &s.csrf, product, None, &variant_values("MODEL", &[ids[0],ids[2]], "1", "4")).await.unwrap();
        let first = variant_values("SAME-SKU", &[ids[1],ids[2]], "1", "4");
        let second = variant_values("same-sku", &[ids[0],ids[3]], "1", "4");
        let (a,b) = tokio::join!(
            variants.save(&s, &s.csrf, product, None, &first),
            variants.save(&s, &s.csrf, product, None, &second)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(matches!(a,Err(talocart_commerce::variants::VariantError::DuplicateSku)) || matches!(b,Err(talocart_commerce::variants::VariantError::DuplicateSku)));
        let same_code_variant = a.or(b).unwrap();
        let first = product_values("SAME-SKU", "simple-a");
        let second = product_values("same-sku", "simple-b");
        let (a,b) = tokio::join!(products.save(&s,&s.csrf,None,&first),products.save(&s,&s.csrf,None,&second));
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(matches!(a,Err(talocart_commerce::products::ProductError::DuplicateModel)) || matches!(b,Err(talocart_commerce::products::ProductError::DuplicateModel)));
        let simple = a.or(b).unwrap();
        let mut simple_values = products.entry(&s, simple).await.unwrap().values();
        simple_values.status = "published".into();
        simple_values.tax_class_id = products.entry(&s, product).await.unwrap().tax_class_id.unwrap().to_string();
        products.save(&s, &s.csrf, Some(simple), &simple_values).await.unwrap();
        let mut tx=pool.begin().await.unwrap();
        let rows=talocart_commerce::products::Products::cart_merchandise(&mut tx,&[(product,same),(product,same_code_variant),(simple,0)],talocart_platform::content::Language::new(talocart_platform::locale::Locale::ITALIAN, talocart_platform::locale::Locale::ITALIAN)).await.unwrap();
        assert_eq!(rows.len(),3);
        assert_eq!(rows.iter().filter(|r|r.code=="SAME-SKU").count(),2,"Equal codes never collapse distinct purchases");
        assert!(rows.iter().any(|r|r.product_id==product && r.variant_id==same && r.code=="MODEL"));
        assert!(rows.iter().any(|r|r.product_id==simple && r.variant_id==0 && r.code=="SAME-SKU"));
        tx.rollback().await.unwrap();
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn option_sort_order_is_saved_and_rendered() {
    isolated(|pool, _| async move {
        let (identity, _, _, _, _, options, _) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = format!("/admin/catalog/options/{}", options[0]);
        let response = request(
            &app,
            "POST",
            &path,
            &cookie,
            &format!("csrf={}&name=Colore&sort_order=42", session.csrf),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let body = html(request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(
            body.contains(
                "name=\"sort_order\" type=\"number\" min=\"0\" max=\"9999\" value=\"42\""
            ),
            "Saved sort order must be editable"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn option_sort_order_controls_lists_pagination_and_public_selection() {
    use talocart_commerce::variants::{OptionInput, ValueSearch};
    isolated(|pool, _| async move {
        let (_, session, _, variants, product, options, ids) = variant_fixture(&pool).await;
        variants.configure(&session, &session.csrf, product, &options).await.unwrap();
        for (sku, values) in [("RED-S", [ids[0], ids[2]]), ("BLUE-M", [ids[1], ids[3]])] {
            variants.save(&session, &session.csrf, product, None, &variant_values(sku, &values, "1", "3")).await.unwrap();
        }
        for (id, name, order) in [(options[0], "Colore", "20"), (options[1], "Taglia", "10")] {
            variants.save_option_details(&session, &session.csrf, Some(id), &OptionInput { name, sort_order: Some(order) }, None).await.unwrap();
        }
        variants.save_value_details(&session, &session.csrf, options[0], Some(ids[0]), &OptionInput { name: "Rosso", sort_order: Some("10") }, None).await.unwrap();
        variants.save_value_details(&session, &session.csrf, options[0], Some(ids[1]), &OptionInput { name: "Blu", sort_order: Some("20") }, None).await.unwrap();
        // Old API/import callers changing names cannot silently reset priorities.
        variants.save_option(&session, &session.csrf, Some(options[1]), "Taglia").await.unwrap();
        variants.save_value(&session, &session.csrf, options[0], Some(ids[0]), "Rosso").await.unwrap();
        for language in [talocart_platform::content::Language::new(talocart_platform::locale::Locale::ITALIAN, talocart_platform::locale::Locale::ITALIAN), talocart_platform::content::Language::new(talocart_platform::locale::Locale::ENGLISH, talocart_platform::locale::Locale::ITALIAN)] {
            let localized = variants.localized(language);
            assert_eq!(localized.options(&session).await.unwrap().iter().map(|o| o.id).collect::<Vec<_>>(), [options[1], options[0]]);
            assert_eq!(localized.options_counted(&session, 1).await.unwrap().entries[0].id, options[1]);
            assert_eq!(localized.options_page(&session, 1).await.unwrap()[0].id, options[1]);
            assert_eq!(localized.option_values(&session, options[0]).await.unwrap().iter().map(|v| v.id).collect::<Vec<_>>(), [ids[0], ids[1]]);
            let (axes, selected) = localized.public_selection(product, &[ids[0], ids[2]]).await.unwrap();
            assert_eq!(axes.iter().map(|axis| axis.id).collect::<Vec<_>>(), [options[1], options[0]]);
            assert_eq!(axes[1].values.iter().map(|v| v.id).collect::<Vec<_>>(), [ids[0], ids[1]]);
            assert_eq!(selected.unwrap().sku, "RED-S");
            assert!(localized.public_selection(product, &[ids[2], ids[0]]).await.unwrap().1.is_none());
            // The administrative assignment contract stays in its configured positions.
            assert_eq!(localized.configuration(&session, product).await.unwrap().axes.iter().map(|axis| axis.id).collect::<Vec<_>>(), options);
        }
        let extra = variants.save_option(&session, &session.csrf, None, "Pagine").await.unwrap();
        sqlx::query("INSERT INTO catalog_option_values(option_id,name) SELECT $1,'Value '||lpad(n::text,2,'0') FROM generate_series(1,52)n").bind(extra).execute(&pool).await.unwrap();
        let first = variants.option_values(&session, extra).await.unwrap()[0].id;
        variants.save_value_details(&session, &session.csrf, extra, Some(first), &OptionInput { name: "Value 01", sort_order: Some("9999") }, None).await.unwrap();
        let all = ValueSearch::default();
        assert_eq!(variants.option_value_page(&session, extra, first, &all).await.unwrap(), 2);
        assert_eq!(variants.values_counted(&session, extra, 2, &all).await.unwrap().entries.last().unwrap().id, first);
        assert_eq!(variants.option_values_page(&session, extra, 2).await.unwrap().last().unwrap().id, first);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn option_sort_order_http_validation_preserves_input_atomicity_and_permissions() {
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let (identity, owner, _, variants, _, options, ids) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let option_path = format!("/admin/catalog/options/{}", options[0]);
        let value_path = format!("{option_path}/values/{}", ids[0]);
        for path in [
            &option_path,
            &value_path,
            &format!("{option_path}/values"),
            &"/admin/catalog/options/new".to_string(),
        ] {
            for invalid in ["-1", "10000", "1.5", "abc", "", "999999999999999999999"] {
                let response = request(
                    &app,
                    "POST",
                    path,
                    &cookie,
                    &format!("csrf={}&name=Changed&sort_order={invalid}", session.csrf),
                    false,
                )
                .await;
                assert_eq!(
                    response.status(),
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "{path} {invalid}"
                );
                let body = html(response).await;
                assert!(body.contains(&format!("value=\"{invalid}\" required")));
                assert!(body.contains("aria-invalid=\"true\""));
                assert!(body.contains("Inserisci un ordinamento intero"));
            }
            assert_eq!(
                request(
                    &app,
                    "POST",
                    path,
                    &cookie,
                    "csrf=wrong&name=Changed&sort_order=3",
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            variants.option(&owner, options[0]).await.unwrap().name,
            "Colore"
        );
        assert_eq!(
            variants
                .option(&owner, options[0])
                .await
                .unwrap()
                .sort_order,
            0
        );
        assert_eq!(
            variants
                .option_values(&owner, options[0])
                .await
                .unwrap()
                .len(),
            2
        );
        // Invalid translations cannot partly save the priority or primary name.
        let response = request(
            &app,
            "POST",
            &option_path,
            &cookie,
            &format!(
                "csrf={}&name=Changed&sort_order=8&content_primary_locale=it&content_en_name={}",
                session.csrf,
                "x".repeat(81)
            ),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            variants
                .option(&owner, options[0])
                .await
                .unwrap()
                .sort_order,
            0
        );
        for path in [&option_path, &value_path] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    path,
                    &cookie,
                    &format!("csrf={}&name=New&sort_order=9999", session.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::SEE_OTHER
            );
        }
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Option reader",
                &[Permission::ProductsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "option-reader@example.test",
                "correct horse battery".into(),
                Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let raw = login_as(&identity, "option-reader@example.test").await;
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let body = html(request(&app, "GET", &option_path, &cookie, "", false).await).await;
        assert!(body.contains("value=\"9999\" required readonly"));
        for path in [&option_path, &value_path] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    path,
                    &cookie,
                    &format!("csrf={}&name=Blocked&sort_order=0", reader.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        // Database invariants protect callers outside HTTP too.
        assert!(
            sqlx::query("UPDATE catalog_options SET sort_order=-1 WHERE id=$1")
                .bind(options[0])
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE catalog_option_values SET sort_order=10000 WHERE id=$1")
                .bind(ids[0])
                .execute(&pool)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn option_value_deletion_errors_are_localized_and_associated_with_the_action() {
    isolated(|pool, _| async move {
        let (identity, s, _, variants, product, options, ids) = variant_fixture(&pool).await;
        variants
            .configure(&s, &s.csrf, product, &options)
            .await
            .unwrap();
        variants
            .save(
                &s,
                &s.csrf,
                product,
                None,
                &variant_values("USED-DELETE", &[ids[0], ids[2]], "1", "1"),
            )
            .await
            .unwrap();
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = format!(
            "/admin/catalog/options/{}/values/{}/delete",
            options[0], ids[0]
        );
        for (locale, used, confirmation) in [
            (
                "it",
                "è utilizzato da una o più varianti",
                "Conferma l’eliminazione del valore",
            ),
            (
                "en",
                "is used by one or more variants",
                "Confirm deletion of the value",
            ),
        ] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            for (body, expected_status, expected_text) in [
                (
                    format!("csrf={}&confirm=true", session.csrf),
                    StatusCode::CONFLICT,
                    used,
                ),
                (
                    format!("csrf={}", session.csrf),
                    StatusCode::UNPROCESSABLE_ENTITY,
                    confirmation,
                ),
            ] {
                let response = request(&app, "POST", &path, &cookie, &body, false).await;
                assert_eq!(response.status(), expected_status);
                let body = html(response).await;
                let input = body
                    .split("id=\"option-name\"")
                    .nth(1)
                    .unwrap()
                    .split('>')
                    .next()
                    .unwrap();
                assert!(
                    !input.contains("aria-invalid"),
                    "Deletion must not invalidate the option name"
                );
                let marker = format!("action=\"{path}\"");
                let action = body
                    .split(marker.as_str())
                    .nth(1)
                    .unwrap()
                    .split("</form>")
                    .next()
                    .unwrap();
                assert!(
                    action.contains("id=\"option-value-delete-error\""),
                    "Feedback belongs inside the affected deletion form"
                );
                assert!(action.contains(expected_text));
                assert!(action.contains("Rosso"));
                assert!(action.contains("aria-describedby=\"option-value-delete-error\""));
                assert!(action.contains("tabindex=\"-1\" autofocus"));
            }
        }
        assert!(matches!(
            variants.delete_value(&s, &s.csrf, options[0], ids[0]).await,
            Err(talocart_commerce::variants::VariantError::InUse)
        ));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn option_value_deletion_preserves_page_and_clamps_only_when_it_becomes_empty() {
    isolated(|pool, _| async move {
        let (identity, s, _, variants, product, _, _) = variant_fixture(&pool).await;
        let option = variants
            .save_option(&s, &s.csrf, None, "Deletion pages")
            .await
            .unwrap();
        let mut ids = Vec::new();
        for n in 0..55 {
            ids.push(
                variants
                    .save_value(&s, &s.csrf, option, None, &format!("Value {n:02}"))
                    .await
                    .unwrap(),
            );
        }
        variants
            .configure(&s, &s.csrf, product, &[option])
            .await
            .unwrap();
        let used = variants
            .save(
                &s,
                &s.csrf,
                product,
                None,
                &variant_values("PAGED-DELETE", &[ids[54]], "1", "1"),
            )
            .await
            .unwrap();
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let endpoint = |value| format!("/admin/catalog/options/{option}/values/{value}/delete");
        let response = request(
            &app,
            "POST",
            &endpoint(ids[54]),
            &cookie,
            &format!("csrf={}&confirm=true&page=2", session.csrf),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = html(response).await;
        assert!(
            body.contains("name=\"name\" value=\"Value 54\""),
            "Blocked value from page two remains visible"
        );
        assert!(body.contains("Pagina 2 di 2"));
        assert!(body.contains("name=\"page\" value=\"2\""));
        variants.delete(&s, &s.csrf, product, used).await.unwrap();
        for index in (50..55).rev() {
            let response = request(
                &app,
                "POST",
                &endpoint(ids[index]),
                &cookie,
                &format!("csrf={}&confirm=true&page=2", session.csrf),
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let page = if index == 50 { 1 } else { 2 };
            assert_eq!(
                response.headers()[header::LOCATION],
                format!("/admin/catalog/options/{option}?deleted=1&page={page}")
            );
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn option_value_deletion_success_and_missing_values_have_correct_feedback_and_security() {
    isolated(|pool, _| async move {
        let (identity, s, _, variants, _, options, ids) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let role = identity
            .create_role(
                &s,
                &s.csrf,
                "Option values reader",
                &[talocart_platform::identity::Permission::ProductsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "value-reader@example.test",
                "correct horse values".into(),
                talocart_platform::identity::Role::Custom(role),
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let reader_raw = identity
            .login(
                &anon,
                &anon.csrf,
                "value-reader@example.test",
                "correct horse values".into(),
            )
            .await
            .unwrap();
        let reader = identity.session(&reader_raw).await.unwrap().unwrap();
        let protected = format!(
            "/admin/catalog/options/{}/values/{}/delete",
            options[0], ids[0]
        );
        assert_eq!(
            request(
                &app,
                "POST",
                &protected,
                &format!("talocart_session={reader_raw}"),
                &format!("csrf={}&confirm=true", reader.csrf),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        for (locale, success) in [("it", "Valore eliminato."), ("en", "Value deleted.")] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let value = variants
                .save_value(&s, &s.csrf, options[0], None, locale)
                .await
                .unwrap();
            let path = format!(
                "/admin/catalog/options/{}/values/{value}/delete",
                options[0]
            );
            assert_eq!(
                request(&app, "GET", &path, &cookie, "", false)
                    .await
                    .status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &path,
                    &cookie,
                    "csrf=wrong&confirm=true",
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            let wrong = format!(
                "/admin/catalog/options/{}/values/{value}/delete",
                options[1]
            );
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &wrong,
                    &cookie,
                    &format!("csrf={}&confirm=true", session.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
            let response = request(
                &app,
                "POST",
                &path,
                &cookie,
                &format!("csrf={}&confirm=true", session.csrf),
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let target = response.headers()[header::LOCATION].to_str().unwrap();
            let body = html(request(&app, "GET", target, &cookie, "", false).await).await;
            assert!(
                body.contains(success),
                "A deletion must report deletion, in the selected language"
            );
            assert!(
                !body.contains("Opzione e valori salvati.")
                    && !body.contains("Option and values saved.")
            );
            let response = request(
                &app,
                "POST",
                &path,
                &cookie,
                &format!("csrf={}&confirm=true", session.csrf),
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            let body = html(response).await;
            let input = body
                .split("id=\"option-name\"")
                .nth(1)
                .unwrap()
                .split('>')
                .next()
                .unwrap();
            assert!(!input.contains("aria-invalid"));
        }
        assert!(
            variants
                .option_values(&s, options[0])
                .await
                .unwrap()
                .iter()
                .any(|v| v.id == ids[0])
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn option_value_deletion_concurrent_order_keeps_error_with_value() {
    isolated(|pool, _| async move {
        let (identity, s, _, variants, product, _, _) = variant_fixture(&pool).await;
        let option = variants.save_option(&s, &s.csrf, None, "Concurrent deletion").await.unwrap();
        let mut target = 0;
        for n in 0..55 {
            target = variants.save_value(&s, &s.csrf, option, None, &format!("Value {n:02}")).await.unwrap();
        }
        sqlx::query("UPDATE catalog_option_values SET sort_order=10 WHERE option_id=$1").bind(option).execute(&pool).await.unwrap();
        variants.configure(&s, &s.csrf, product, &[option]).await.unwrap();
        variants.save(&s, &s.csrf, product, None, &variant_values("CONCURRENT-DELETE", &[target], "1", "1")).await.unwrap();
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let endpoint = format!("/admin/catalog/options/{option}/values/{target}/delete");
        for (locale, order, previous, current, message) in [
            ("it", 0, 2, 1, "è utilizzato da una o più varianti"),
            ("en", 20, 1, 2, "is used by one or more variants"),
        ] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let mut edit = pool.begin().await.unwrap();
            talocart_platform::database::commercial_lock::exclusive(&mut edit).await.unwrap();
            sqlx::query("UPDATE catalog_option_values SET sort_order=$1 WHERE id=$2").bind(order).bind(target).execute(&mut *edit).await.unwrap();
            let request_app = app.clone();
            let request_path = endpoint.clone();
            let request_cookie = cookie.clone();
            let body = format!("csrf={}&confirm=true&page={previous}", session.csrf);
            let deletion = tokio::spawn(async move {
                request(&request_app, "POST", &request_path, &request_cookie, &body, false).await
            });
            // Wait for a real lock conflict, rather than assuming a timing delay
            // proves that the handler has finished its pre-mutation page lookup.
            let blocked = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND classid=($1::bigint >> 32)::oid AND objid=($1::bigint & 4294967295)::oid)")
                        .bind(talocart_platform::database::commercial_lock::KEY)
                        .fetch_one(&pool).await.unwrap();
                    if waiting { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }).await;
            edit.commit().await.unwrap();
            let response = tokio::time::timeout(std::time::Duration::from_secs(10), deletion).await.expect("Deletion finishes after the concurrent edit commits").unwrap();
            assert!(blocked.is_ok(), "Deletion was waiting behind the concurrent reorder");
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let body = html(response).await;
            let marker = format!("action=\"{endpoint}\"");
            let action = body.split(marker.as_str()).nth(1).expect("The reordered value remains on the error page").split("</form>").next().unwrap();
            assert!(action.contains("id=\"option-value-delete-error\""));
            assert!(action.contains(message));
            assert!(action.contains("Value 54"));
            assert!(action.contains(&format!("name=\"page\" value=\"{current}\"")));
            assert!(action.contains("tabindex=\"-1\" autofocus"));
            assert_eq!(body.matches("class=\"ui-edit-list__row option-value-row\"").count(), if current == 1 {50} else {5});
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn option_value_search_matches_active_languages_and_survives_actions() {
    isolated(|pool, _| async move {
        let (identity, s, _, variants, _, _, _) = variant_fixture(&pool).await;
        let option = variants.save_option(&s, &s.csrf, None, "Searchable").await.unwrap();
        let mut ids = Vec::new();
        for n in 0..60 {
            ids.push(variants.save_value(&s, &s.csrf, option, None, &format!("Tono {n:02}")).await.unwrap());
        }
        let green = variants.save_value(&s, &s.csrf, option, None, "Verde 50%_scuro").await.unwrap();
        let red = variants.save_value(&s, &s.csrf, option, None, "Rosso").await.unwrap();
        sqlx::query("INSERT INTO catalog_option_values_translations(entity_id,locale,name,option_id) VALUES($1,'en','Dark green',$3),($2,'en','Red',$3)")
            .bind(green).bind(red).bind(option).execute(&pool).await.unwrap();
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let base = format!("/admin/catalog/options/{option}");
        let rows = |body: &str| body.matches("class=\"ui-edit-list__row option-value-row\"").count();
        // Secondary-language translations, the primary name and literal wildcards all match.
        for (query, expected) in [("GREEN", green), ("verde", green), ("50%25_", green), ("red", red)] {
            let body = html(request(&app, "GET", &format!("{base}?q={query}"), &cookie, "", false).await).await;
            assert_eq!(rows(&body), 1, "{query}");
            assert!(body.contains(&format!("action=\"{base}/values/{expected}\"")), "{query}");
        }
        let body = html(request(&app, "GET", &format!("{base}?q=GREEN"), &cookie, "", false).await).await;
        assert!(body.contains("1 valore corrisponde a «GREEN»."));
        assert!(body.contains("id=\"option-values-q\" type=\"search\" name=\"q\" value=\"GREEN\""));
        assert_eq!(body.matches("<input type=\"hidden\" name=\"q\" value=\"GREEN\">").count(), 3, "New, edit and delete forms keep the search");
        // Search results are paginated with the search kept in the links.
        let body = html(request(&app, "GET", &format!("{base}?q=tono"), &cookie, "", false).await).await;
        assert_eq!(rows(&body), 50);
        assert!(body.contains("60 valori corrispondono a «tono»."));
        assert!(body.contains("?page=2&#38;q=tono"));
        let body = html(request(&app, "GET", &format!("{base}?q=tono&page=2"), &cookie, "", false).await).await;
        assert_eq!(rows(&body), 10);
        let body = html(request(&app, "GET", &format!("{base}?q=assente"), &cookie, "", false).await).await;
        assert_eq!(rows(&body), 0);
        assert!(body.contains("Nessun valore corrisponde alla ricerca."));
        // Invalid text is reported on the field and never applied.
        let response = request(&app, "GET", &format!("{base}?q={}", "x".repeat(81)), &cookie, "", false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = html(response).await;
        assert!(body.contains("aria-invalid=\"true\""));
        assert!(body.contains("id=\"option-values-q-error\""));
        assert_eq!(rows(&body), 50);
        assert!(body.contains("name=\"q\" value=\"xxx"), "The field keeps the text to correct");
        assert!(!body.contains("type=\"hidden\" name=\"q\""), "Invalid search is not propagated to forms");
        // Saving inside a search returns to the value's page within the same search.
        let save = |name: &str| format!("csrf={}&name={name}&q=tono", session.csrf);
        let response = request(&app, "POST", &format!("{base}/values/{}", ids[55]), &cookie, &save("Tono+55+bis"), false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], format!("{base}?saved=1&page=2&q=tono"));
        // A value renamed outside the search stays visible: the search is cleared.
        let response = request(&app, "POST", &format!("{base}/values/{}", ids[0]), &cookie, &save("Azzurro"), false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], format!("{base}?saved=1"));
        // Errors keep the filtered list and the search for the next submission.
        let response = request(&app, "POST", &format!("{base}/values/{}", ids[1]), &cookie, &save("Rosso"), false).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = html(response).await;
        assert!(body.contains("59 valori corrispondono a «tono»."));
        assert!(body.contains("<input type=\"hidden\" name=\"q\" value=\"tono\">"));
        assert!(!body.contains(&format!("action=\"{base}/values/{red}\"")), "Rows outside the search stay hidden");
        // Deletion inside a search keeps both page and search.
        let response = request(&app, "POST", &format!("{base}/values/{}/delete", ids[59]), &cookie, &format!("csrf={}&confirm=true&page=2&q=tono", session.csrf), false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], format!("{base}?deleted=1&page=2&q=tono"));
        let body = html(request(&app, "GET", &format!("{base}?deleted=1&page=2&q=tono"), &cookie, "", false).await).await;
        assert_eq!(rows(&body), 8);
        // A stale deletion (value already gone) keeps the search and its page.
        let response = request(&app, "POST", &format!("{base}/values/{}/delete", ids[59]), &cookie, &format!("csrf={}&confirm=true&page=2&q=tono", session.csrf), false).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = html(response).await;
        assert!(body.contains("Il valore non è più presente"));
        assert!(body.contains("58 valori corrispondono a «tono»."), "The search survives a stale deletion");
        assert!(body.contains("<input type=\"hidden\" name=\"q\" value=\"tono\">"));
        assert!(body.contains("Pagina 2 di 2"));
        assert_eq!(rows(&body), 8, "Page two of the search, not of the full list");
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn option_value_deletion_concurrent_rename_keeps_filtered_error_visible() {
    isolated(|pool, _| async move {
        let (identity, s, _, variants, product, _, _) = variant_fixture(&pool).await;
        let option = variants.save_option(&s, &s.csrf, None, "Concurrent deletion").await.unwrap();
        let mut target = 0;
        for n in 0..55 {
            target = variants.save_value(&s, &s.csrf, option, None, &format!("Value {n:02}")).await.unwrap();
        }
        sqlx::query("UPDATE catalog_option_values SET sort_order=10 WHERE option_id=$1").bind(option).execute(&pool).await.unwrap();
        variants.configure(&s, &s.csrf, product, &[option]).await.unwrap();
        variants.save(&s, &s.csrf, product, None, &variant_values("CONCURRENT-DELETE", &[target], "1", "1")).await.unwrap();
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let endpoint = format!("/admin/catalog/options/{option}/values/{target}/delete");
        for (locale, order, previous, current, message) in [
            ("it", 0, 2, 1, "è utilizzato da una o più varianti"),
            ("en", 20, 1, 2, "is used by one or more variants"),
        ] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            // Restore a matching name before each language's concurrent rename.
            sqlx::query("UPDATE catalog_option_values SET name='Value 54' WHERE id=$1").bind(target).execute(&pool).await.unwrap();
            sqlx::query("UPDATE catalog_option_values_translations SET name='Value 54' WHERE entity_id=$1").bind(target).execute(&pool).await.unwrap();
            let mut edit = pool.begin().await.unwrap();
            talocart_platform::database::commercial_lock::exclusive(&mut edit).await.unwrap();
            sqlx::query("UPDATE catalog_option_values SET sort_order=$1, name='Renamed elsewhere' WHERE id=$2").bind(order).bind(target).execute(&mut *edit).await.unwrap();
            sqlx::query("UPDATE catalog_option_values_translations SET name='Renamed elsewhere' WHERE entity_id=$1").bind(target).execute(&mut *edit).await.unwrap();
            let request_app = app.clone();
            let request_path = endpoint.clone();
            let request_cookie = cookie.clone();
            let body = format!("csrf={}&confirm=true&page={previous}&q=Value", session.csrf);
            let deletion = tokio::spawn(async move {
                request(&request_app, "POST", &request_path, &request_cookie, &body, false).await
            });
            // Wait for a real lock conflict, rather than assuming a timing delay
            // proves that the handler has finished its pre-mutation page lookup.
            let blocked = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND classid=($1::bigint >> 32)::oid AND objid=($1::bigint & 4294967295)::oid)")
                        .bind(talocart_platform::database::commercial_lock::KEY)
                        .fetch_one(&pool).await.unwrap();
                    if waiting { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }).await;
            edit.commit().await.unwrap();
            let response = tokio::time::timeout(std::time::Duration::from_secs(10), deletion).await.expect("Deletion finishes after the concurrent edit commits").unwrap();
            assert!(blocked.is_ok(), "Deletion was waiting behind the concurrent rename");
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let body = html(response).await;
            let marker = format!("action=\"{endpoint}\"");
            let action = body.split(marker.as_str()).nth(1).expect("The renamed value remains on the error page").split("</form>").next().unwrap();
            assert!(action.contains("id=\"option-value-delete-error\""));
            assert!(action.contains(message));
            assert!(action.contains("Renamed elsewhere"));
            assert!(!body.contains("type=\"hidden\" name=\"q\""), "Search is cleared only to keep the renamed value visible");
            assert!(body.contains("id=\"option-values-q\" type=\"search\" name=\"q\" value=\"\""));
            assert!(body.contains("<details class=\"ui-confirm\" open>"));
            assert!(action.contains(&format!("name=\"page\" value=\"{current}\"")));
            assert!(action.contains("tabindex=\"-1\" autofocus"));
            assert_eq!(body.matches("class=\"ui-edit-list__row option-value-row\"").count(), if current == 1 {50} else {5});
        }
    }).await;
}
