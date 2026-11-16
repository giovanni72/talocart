use super::*;
use talocart_commerce::content::{Content, Resource};
use talocart_platform::identity::{Permission, Role};
use talocart_platform::{
    content::{ContentError, Translation},
    locale::Locale,
};

fn text(locale: &str, name: &str, description: &str) -> Translation {
    Translation {
        locale: locale.into(),
        name: name.into(),
        description: description.into(),
        ..Translation::default()
    }
}
fn form(pairs: &[(&str, &str)]) -> String {
    fn escape(value: &str) -> String {
        value
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    }
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", escape(k), escape(v)))
        .collect::<Vec<_>>()
        .join("&")
}
fn product_body(csrf: &str, tax: i64, name: &str, secondary: &str) -> String {
    form(&[
        ("csrf", csrf),
        ("name", name),
        ("model", "MODEL"),
        ("slug", "model"),
        ("description", "Originale"),
        ("price", "12.34"),
        ("price_mode", "inclusive"),
        ("manage_stock", "true"),
        ("stock", "10"),
        ("stock_revision", "0"),
        ("status", "published"),
        ("tax_class_id", &tax.to_string()),
        ("content_primary_locale", "it"),
        ("content_en_name", secondary),
    ])
}
#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn content_translations_preserve_ids_localize_search_and_fallback_per_field() {
    isolated(|pool,_|async move{
        let (identity,session,products,variants,product,options,values)=variant_fixture(&pool).await;
        let original=products.entry(&session,product).await.unwrap();
        let content=Content::new(pool.clone(),identity.clone());
        for (resource,id,it,en) in [(Resource::Category,original.category_id.unwrap(),"Magliette","T-shirts"),(Resource::Brand,original.brand_id.unwrap(),"Marchio italiano","English brand"),(Resource::Product,product,"Maglietta","Shirt <script>"),(Resource::Option,options[0],"Colore","Colour"),(Resource::Value,values[0],"Rosso","Red")] {
            let description=if matches!(resource,Resource::Category|Resource::Brand|Resource::Product){"Descrizione italiana"}else{""};
            content.save(&session,&session.csrf,resource,id,"it",&[text("it",it,description),text("en",en,"")]).await.unwrap();
        }
        variants.configure(&session,&session.csrf,product,&options).await.unwrap();
        variants.save(&session,&session.csrf,product,None,&variant_values("CONTENT-RED-S",&[values[0],values[2]],"12.34","10")).await.unwrap();
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        for path in ["/en/catalog","/en/catalog/categoria-prodotti","/en/brands/marchio-prodotti","/en/search/Shirt","/en/products/model"] {
            let response=request(&app,"GET",path,"","",false).await;assert_eq!(response.status(),StatusCode::OK,"{path}");
            let page=html(response).await;assert!(page.contains("Shirt &#60;script&#62;"),"{path}");assert!(page.contains("T-shirts"),"{path}");assert!(!page.contains("Shirt <script>"));
        }
        let en=html(request(&app,"GET","/en/products/model","","",false).await).await;
        assert!(en.contains("Descrizione italiana") && en.contains("Colour") && en.contains("Red"));
        let it=html(request(&app,"GET","/products/model","","",false).await).await;assert!(it.contains("Maglietta") && it.contains("Colore") && it.contains("Rosso"));
        let missing=html(request(&app,"GET","/en/search/Maglietta","","",false).await).await;assert!(missing.contains("0 products"));
        let rows=content.load(&session,Resource::Product,product).await.unwrap();assert_eq!(rows.iter().find(|r|r.locale=="en").unwrap().description,"");
        let after=products.entry(&session,product).await.unwrap();assert_eq!(after.slug,original.slug);assert_eq!(after.price_cents,original.price_cents);assert_eq!(after.stock,original.stock);
        let mut update=rows; update.iter_mut().find(|r|r.locale=="it").unwrap().description="Nuovo ripiego".into();
        content.save(&session,&session.csrf,Resource::Product,product,"it",&update).await.unwrap();
        assert!(html(request(&app,"GET","/en/products/model","","",true).await).await.contains("Nuovo ripiego"));
        // Fallback follows the configured primary, even when it changes after creation.
        talocart_platform::storefront::Storefront::new(pool.clone()).save_languages(&session,&session.csrf,"en",&["it".into(),"en".into()]).await.unwrap();
        sqlx::query("UPDATE catalog_products_translations SET name='',description='' WHERE entity_id=$1 AND locale='it'").bind(product).execute(&pool).await.unwrap();
        let switched=html(request(&app,"GET","/it/products/model","","",false).await).await;assert!(switched.contains("Shirt &#60;script&#62;"));
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1 WHERE id=$2").bind(locale).bind(session.administrator_id).execute(&pool).await.unwrap();
            assert_eq!(request(&app,"GET","/admin/content/home/1",&cookie,"",false).await.status(), StatusCode::NOT_FOUND);
        }

        let editor = html(request(&app,"GET",&format!("/admin/catalog/products/{product}"),&cookie,"",false).await).await;
        assert!(editor.contains("value=\"Shirt &#60;script&#62;\""),"Ordinary editor reads the primary content language");
        let localized=products.localized(talocart_platform::content::Language::new(Locale::ITALIAN,Locale::ENGLISH));
        let result=localized.public_page("Shirt","","",false,999,talocart_commerce::products::CatalogPageSize::default()).await.unwrap();assert_eq!(result.total,1);assert_eq!(result.page,1);assert_eq!(result.entries[0].id,product);
    }).await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn content_editor_validation_permissions_csrf_atomicity_and_homepage() {
    isolated(|pool, _| async move {
        let (identity, session, products, _, product, _, _) = variant_fixture(&pool).await;
        let entry = products.entry(&session, product).await.unwrap();
        let raw = authenticated(&identity).await;
        let web_session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let content = Content::new(pool.clone(), identity.clone());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = format!("/admin/catalog/products/{product}");
        let page = html(request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(
            page.contains("name=\"content_en_name\"") && !page.contains("href=\"/admin/content/")
        );
        let before = content
            .load(&session, Resource::Product, product)
            .await
            .unwrap();
        let invalid = product_body(
            &web_session.csrf,
            entry.tax_class_id.unwrap(),
            "Salvato",
            &"x".repeat(121),
        ) + "&content_en_description=Keep+me";
        let response = request(&app, "POST", &path, &cookie, &invalid, false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let page = html(response).await;
        assert!(page.contains("Keep me") && page.contains("aria-invalid=\"true\""));
        assert_eq!(
            content
                .load(&session, Resource::Product, product)
                .await
                .unwrap(),
            before
        );
        assert_eq!(
            products.entry(&session, product).await.unwrap().name,
            entry.name
        );
        let invalid = product_body(
            &web_session.csrf,
            entry.tax_class_id.unwrap(),
            "Salvato",
            "Retained English",
        )
        .replace("price=12.34", "price=bad");
        let page = html(request(&app, "POST", &path, &cookie, &invalid, false).await).await;
        assert!(page.contains("Retained English") && page.contains("value=\"bad\""));
        let bad = product_body("bad", entry.tax_class_id.unwrap(), "No", "No");
        assert_eq!(
            request(&app, "POST", &path, &cookie, &bad, false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&app, "GET", &path, "", "", false).await.status(),
            StatusCode::SEE_OTHER
        );
        let good = product_body(
            &web_session.csrf,
            entry.tax_class_id.unwrap(),
            "Prodotto",
            "Product",
        ) + "&seo_title=SEO+IT&content_en_seo_title=SEO+EN";
        assert_eq!(
            request(&app, "POST", &path, &cookie, &good, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert!(
            html(request(&app, "GET", "/en/products/model", "", "", false).await)
                .await
                .contains("SEO EN")
        );
        let wrong = good.replace("content_primary_locale=it", "content_primary_locale=en");
        assert_eq!(
            request(&app, "POST", &path, &cookie, &wrong, false)
                .await
                .status(),
            StatusCode::CONFLICT
        );
        let store = talocart_platform::storefront::Storefront::new(pool.clone());
        store
            .save_translations(
                &session,
                &session.csrf,
                "it",
                &[
                    text("it", "Benvenuti", "Introduzione italiana"),
                    text("en", "Welcome", ""),
                ],
            )
            .await
            .unwrap();
        let home = html(request(&app, "GET", "/en", "", "", false).await).await;
        assert!(home.contains("Welcome") && home.contains("Introduzione italiana"));
        let rows = content
            .load(&session, Resource::Product, product)
            .await
            .unwrap();
        assert!(matches!(
            content
                .save(&session, "bad", Resource::Product, product, "it", &rows)
                .await,
            Err(ContentError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert!(matches!(
            content.load(&session, Resource::Product, 999999).await,
            Err(ContentError::NotFound)
        ));
        store
            .save_languages(&session, &session.csrf, "it", &["it".into()])
            .await
            .unwrap();
        assert_eq!(
            content
                .load(&session, Resource::Product, product)
                .await
                .unwrap(),
            rows
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn content_concurrent_saves_and_current_permissions_protect_all_languages() {
    isolated(|pool, _| async move {
        let (identity, session, products, _, product, _, _) = variant_fixture(&pool).await;
        let content = Content::new(pool.clone(), identity.clone());
        let role = identity
            .create_role(
                &session,
                &session.csrf,
                "Content reader",
                &[Permission::ProductsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &session,
                &session.csrf,
                "content-reader@example.test",
                "correct horse battery".into(),
                Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let raw = login_as(&identity, "content-reader@example.test").await;
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = format!("/admin/catalog/products/{product}");
        let cookie = format!("talocart_session={raw}");
        let page = html(request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(page.contains("readonly") && !page.contains("type=\"submit\">Salva traduzioni"));
        assert_eq!(
            request(
                &app,
                "POST",
                &path,
                &cookie,
                &product_body(
                    &reader.csrf,
                    products
                        .entry(&session, product)
                        .await
                        .unwrap()
                        .tax_class_id
                        .unwrap(),
                    "No",
                    "No"
                ),
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
                "/admin/catalog/categories/1",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let first = [text("it", "Primo", "Uno"), text("en", "First", "One")];
        let second = [text("it", "Secondo", "Due"), text("en", "Second", "Two")];
        let mut first_values = products.entry(&session, product).await.unwrap().values();
        first_values.name = "Primo".into();
        first_values.description = "Uno".into();
        first_values.stock = "11".into();
        let mut second_values = first_values.clone();
        second_values.name = "Secondo".into();
        second_values.description = "Due".into();
        second_values.stock = "22".into();
        let first_draft = talocart_platform::content::Draft {
            primary: "it".into(),
            rows: first.to_vec(),
            ..Default::default()
        };
        let second_draft = talocart_platform::content::Draft {
            primary: "it".into(),
            rows: second.to_vec(),
            ..Default::default()
        };
        let (a, b) = tokio::join!(
            products.save_with_content(
                &session,
                &session.csrf,
                Some(product),
                &first_values,
                Some(&first_draft)
            ),
            products.save_with_content(
                &session,
                &session.csrf,
                Some(product),
                &second_values,
                Some(&second_draft)
            ),
        );
        assert_eq!([a.is_ok(), b.is_ok()].into_iter().filter(|v| *v).count(), 1);
        let conflict = if a.is_err() { a } else { b };
        assert!(matches!(
            conflict,
            Err(talocart_commerce::products::ProductError::StockChanged)
        ));
        let rows = content
            .load(&session, Resource::Product, product)
            .await
            .unwrap();
        assert!(
            rows.iter().all(|r| first.contains(r)) || rows.iter().all(|r| second.contains(r)),
            "No mixture of concurrent documents"
        );
        let saved = products.entry(&session, product).await.unwrap();
        assert!(
            (saved.name == "Primo" && saved.stock == 11)
                || (saved.name == "Secondo" && saved.stock == 22)
        );
        assert_eq!(
            rows.iter().find(|r| r.locale == "it").unwrap().name,
            saved.name
        );
        identity
            .update_role(&session, &session.csrf, &role, "Revoked", &[])
            .await
            .unwrap();
        assert!(matches!(
            content.load(&reader, Resource::Product, product).await,
            Err(ContentError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn inline_languages_create_every_resource_and_rollback_the_whole_form() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let content = Content::new(pool.clone(), identity.clone());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (resource, route) in [
            (Resource::Category, "categories"),
            (Resource::Brand, "brands"),
            (Resource::Option, "options"),
        ] {
            let path = format!("/admin/catalog/{route}/new");
            let page = html(request(&app, "GET", &path, &cookie, "", false).await).await;
            assert!(
                page.contains("name=\"content_en_name\""),
                "{route}: secondary language in new form"
            );
            assert!(!page.contains("href=\"/admin/content/"));
            let bad = form(&[
                ("csrf", &session.csrf),
                ("name", "Nome italiano"),
                ("slug", "multilingual"),
                ("sort_order", "0"),
                ("active", "true"),
                ("content_primary_locale", "it"),
                ("content_en_name", &"x".repeat(121)),
            ]);
            let response = request(&app, "POST", &path, &cookie, &bad, false).await;
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{route}"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                    "SELECT count(*) FROM {}",
                    resource.definition().table
                )))
                .fetch_one(&pool)
                .await
                .unwrap(),
                0
            );
            let good = bad.replace(&"x".repeat(121), "English name");
            let response = request(&app, "POST", &path, &cookie, &good, false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER, "{route}");
            let id = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT id FROM {}",
                resource.definition().table
            )))
            .fetch_one(&pool)
            .await
            .unwrap();
            let rows = content.load(&session, resource, id).await.unwrap();
            assert_eq!(
                rows.iter().find(|r| r.locale == "en").unwrap().name,
                "English name"
            );
            if resource == Resource::Option {
                let path = format!("/admin/catalog/options/{id}/values");
                let body = form(&[
                    ("csrf", &session.csrf),
                    ("name", "Rosso"),
                    ("content_primary_locale", "it"),
                    ("content_en_name", "Red"),
                ]);
                assert_eq!(
                    request(&app, "POST", &path, &cookie, &body, false)
                        .await
                        .status(),
                    StatusCode::SEE_OTHER
                );
                let value = sqlx::query_scalar::<_, i64>("SELECT id FROM catalog_option_values")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                assert_eq!(
                    content
                        .load(&session, Resource::Value, value)
                        .await
                        .unwrap()
                        .iter()
                        .find(|r| r.locale == "en")
                        .unwrap()
                        .name,
                    "Red"
                );
            }
        }
        let (category, brand, class) = product_references(&pool, &session).await;
        let _ = (category, brand);
        let path = "/admin/catalog/products/new";
        let bad = product_body(&session.csrf, class, "Prodotto nuovo", &"x".repeat(121));
        assert_eq!(
            request(&app, "POST", path, &cookie, &bad, false)
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_products")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        let good = product_body(&session.csrf, class, "Prodotto nuovo", "New product");
        assert_eq!(
            request(&app, "POST", path, &cookie, &good, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        let product = sqlx::query_scalar::<_, i64>("SELECT id FROM catalog_products")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            content
                .load(&session, Resource::Product, product)
                .await
                .unwrap()
                .iter()
                .find(|r| r.locale == "en")
                .unwrap()
                .name,
            "New product"
        );
        let before = talocart_platform::storefront::Storefront::new(pool.clone())
            .settings()
            .await
            .unwrap();
        let body = form(&[
            ("csrf", &session.csrf),
            ("palette", "clay"),
            ("typography", "modern"),
            ("density", "compact"),
            ("heading", "Benvenuti"),
            ("show_hero", "true"),
            ("introduction", "Originale"),
            ("seo_title", "SEO"),
            ("seo_description", ""),
            ("public_origin", ""),
            ("content_primary_locale", "it"),
            ("content_en_name", &"x".repeat(121)),
        ]);
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/storefront",
                &cookie,
                &body,
                false
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let after = talocart_platform::storefront::Storefront::new(pool.clone())
            .settings()
            .await
            .unwrap();
        assert_eq!(before.palette, after.palette);
        assert_eq!(before.heading, after.heading);
        let good = body.replace(&"x".repeat(121), "Welcome");
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/storefront",
                &cookie,
                &good,
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        let home = html(request(&app, "GET", "/en", "", "", false).await).await;
        assert!(home.contains("Welcome"));
        let store = talocart_platform::storefront::Storefront::new(pool.clone());
        store
            .save_languages(&session, &session.csrf, "en", &["it".into(), "en".into()])
            .await
            .unwrap();
        let page = html(
            request(
                &app,
                "GET",
                "/admin/catalog/categories/new",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(
            page.contains("name=\"content_it_name\"")
                && page.contains("name=\"content_primary_locale\" value=\"en\"")
        );
        let body = form(&[
            ("csrf", &session.csrf),
            ("name", "English primary"),
            ("slug", "english-primary-content"),
            ("sort_order", "0"),
            ("content_primary_locale", "en"),
            ("content_it_name", "Nome italiano"),
        ]);
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/catalog/categories/new",
                &cookie,
                &body,
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        let id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM catalog_categories WHERE slug='english-primary-content'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let rows = content
            .load(&session, Resource::Category, id)
            .await
            .unwrap();
        assert_eq!(
            rows.iter().find(|r| r.locale == "it").unwrap().name,
            "Nome italiano"
        );
        assert_eq!(
            rows.iter().find(|r| r.locale == "en").unwrap().name,
            "English primary"
        );
        // Historical clients still submit the primary fields without a language marker.
        let legacy = form(&[
            ("csrf", &session.csrf),
            ("palette", "invalid"),
            ("typography", "modern"),
            ("density", "compact"),
            ("heading", "English heading"),
            ("introduction", ""),
            ("seo_title", ""),
            ("seo_description", ""),
            ("public_origin", ""),
        ]);
        let error = html(
            request(
                &app,
                "POST",
                "/admin/settings/storefront",
                &cookie,
                &legacy,
                false,
            )
            .await,
        )
        .await;
        assert!(
            error.contains("name=\"content_primary_locale\" value=\"en\""),
            "Legacy error keeps the actual primary content language"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn audit_inactive_languages_are_not_editable() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let store = talocart_platform::storefront::Storefront::new(pool.clone());
        store
            .save_languages(&session, &session.csrf, "en", &["en".into()])
            .await
            .unwrap();
        let app = router(test_app(pool, "Shop".into()).await);
        for path in [
            "/admin/catalog/products/new",
            "/admin/catalog/categories/new",
            "/admin/catalog/brands/new",
            "/admin/catalog/options/new",
            "/admin/settings/storefront",
        ] {
            let page = html(
                request(
                    &app,
                    "GET",
                    path,
                    &format!("talocart_session={raw}"),
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(
                !page.contains("name=\"content_it_name\""),
                "Inactive Italian shown at {path}"
            );
            assert!(page.contains("name=\"content_primary_locale\" value=\"en\""));
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn audit_omitted_translation_fields_are_preserved() {
    isolated(|pool, _| async move {
        let (identity, session, products, _, product, _, _) = variant_fixture(&pool).await;
        let entry = products.entry(&session, product).await.unwrap();
        let content = Content::new(pool.clone(), identity.clone());
        content
            .save(
                &session,
                &session.csrf,
                Resource::Product,
                product,
                "it",
                &[
                    text("it", "Maglietta", "Descrizione"),
                    text("en", "Shirt", "Keep description"),
                ],
            )
            .await
            .unwrap();
        let raw = authenticated(&identity).await;
        let web_session = identity.session(&raw).await.unwrap().unwrap();
        let app = router(test_app(pool, "Shop".into()).await);
        let body = product_body(
            &web_session.csrf,
            entry.tax_class_id.unwrap(),
            "Maglietta",
            "New shirt",
        );
        assert_eq!(
            request(
                &app,
                "POST",
                &format!("/admin/catalog/products/{product}"),
                &format!("talocart_session={raw}"),
                &body,
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        let rows = content
            .load(&session, Resource::Product, product)
            .await
            .unwrap();
        assert_eq!(
            rows.iter().find(|r| r.locale == "en").unwrap().description,
            "Keep description"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn audit_primary_switch_does_not_compare_names_across_languages() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let a: i64 = sqlx::query_scalar(
            "INSERT INTO catalog_brands(name,slug) VALUES('Rosso','audit-a') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let b: i64 = sqlx::query_scalar(
            "INSERT INTO catalog_brands(name,slug) VALUES('Red','audit-b') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let content = Content::new(pool.clone(), identity);
        content
            .save(
                &session,
                &session.csrf,
                Resource::Brand,
                a,
                "it",
                &[text("it", "Rosso", ""), text("en", "Red", "")],
            )
            .await
            .unwrap();
        content
            .save(
                &session,
                &session.csrf,
                Resource::Brand,
                b,
                "it",
                &[text("it", "Red", ""), text("en", "Blue", "")],
            )
            .await
            .unwrap();
        talocart_platform::storefront::Storefront::new(pool)
            .save_languages(&session, &session.csrf, "en", &["it".into(), "en".into()])
            .await
            .unwrap();
        content
            .save(
                &session,
                &session.csrf,
                Resource::Brand,
                a,
                "en",
                &[text("it", "Rosso", ""), text("en", "Red", "Updated")],
            )
            .await
            .expect("Different language names cannot conflict");
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn audit_active_snapshot_preserves_inactive_text_and_rejects_stale_forms() {
    isolated(|pool, _| async move {
        let (identity, session, products, _, product, _, _) = variant_fixture(&pool).await;
        let entry = products.entry(&session, product).await.unwrap();
        let content = Content::new(pool.clone(), identity.clone());
        content
            .save(
                &session,
                &session.csrf,
                Resource::Product,
                product,
                "it",
                &[
                    text("it", "Maglietta", ""),
                    text("en", "Shirt", "Saved English"),
                ],
            )
            .await
            .unwrap();
        let raw = authenticated(&identity).await;
        let csrf = identity.session(&raw).await.unwrap().unwrap().csrf;
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = format!("/admin/catalog/products/{product}");
        let store = talocart_platform::storefront::Storefront::new(pool.clone());
        let old_form = product_body(
            &csrf,
            entry.tax_class_id.unwrap(),
            "Must not save",
            "Must not save",
        ) + "&content_active_locales=en,it";
        store
            .save_languages(&session, &session.csrf, "it", &["it".into()])
            .await
            .unwrap();
        let response = request(&app, "POST", &path, &cookie, &old_form, false).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(html(response).await.contains("Must not save"));
        let mut body = product_body(
            &csrf,
            entry.tax_class_id.unwrap(),
            "Nome aggiornato",
            "unused",
        );
        body = body.replace("&content_en_name=unused", "");
        body.push_str("&content_active_locales=it");
        assert_eq!(
            request(&app, "POST", &path, &cookie, &body, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        let rows = content
            .load(&session, Resource::Product, product)
            .await
            .unwrap();
        assert_eq!(
            rows.iter().find(|r| r.locale == "en").unwrap(),
            &text("en", "Shirt", "Saved English")
        );
        store
            .save_languages(&session, &session.csrf, "it", &["en".into(), "it".into()])
            .await
            .unwrap();
        let page = html(request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(page.contains("Saved English"));
        let body = product_body(
            &csrf,
            entry.tax_class_id.unwrap(),
            "Nome aggiornato",
            "Shirt",
        ) + "&content_active_locales=en,it&content_en_description=";
        let revision = products
            .entry(&session, product)
            .await
            .unwrap()
            .stock_revision;
        let body = body.replace("stock_revision=0", &format!("stock_revision={revision}"));
        assert_eq!(
            request(&app, "POST", &path, &cookie, &body, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            content
                .load(&session, Resource::Product, product)
                .await
                .unwrap()
                .iter()
                .find(|r| r.locale == "en")
                .unwrap()
                .description,
            ""
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires a dedicated TALOCART_TEST_DATABASE_URL"]
async fn audit_name_uniqueness_is_per_language_and_value_owner_and_concurrent() {
    isolated(|pool, _| async move {
        let (identity, session, _, variants, _, options, values) = variant_fixture(&pool).await;
        let content = Content::new(pool.clone(), identity);
        for (resource, a, b) in [(Resource::Option, options[0], options[1]), (Resource::Value, values[0], values[1])] {
            content.save(&session, &session.csrf, resource, a, "it", &[text("it", "Rosso", ""), text("en", "Red", "")]).await.unwrap();
            content.save(&session, &session.csrf, resource, b, "it", &[text("it", "Red", ""), text("en", "Blue", "")]).await.unwrap();
        }
        let store = talocart_platform::storefront::Storefront::new(pool.clone());
        store.save_languages(&session, &session.csrf, "en", &["en".into(), "it".into()]).await.unwrap();
        variants.save_option(&session, &session.csrf, Some(options[0]), "Red").await.unwrap();
        variants.save_value(&session, &session.csrf, options[0], Some(values[0]), "Red").await.unwrap();
        // Same label in a different option is allowed, case-insensitive duplicate within one is not.
        variants.save_value(&session, &session.csrf, options[1], None, "Red").await.unwrap();
        assert!(variants.save_value(&session, &session.csrf, options[0], None, "RED").await.is_err());
        let (a,b) = tokio::join!(
            variants.save_option(&session, &session.csrf, None, "Concurrent"),
            variants.save_option(&session, &session.csrf, None, "CONCURRENT")
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        // Unique indexes also reject writes bypassing application validation.
        assert!(sqlx::query("UPDATE catalog_options_translations SET name='Red' WHERE entity_id=$1 AND locale='en'").bind(options[1]).execute(&pool).await.is_err());
        let before = content.load(&session, Resource::Option, options[1]).await.unwrap();
        assert!(content.save(&session, &session.csrf, Resource::Option, options[1], "en", &[text("it", "Will roll back", ""), text("en", "Red", "")]).await.is_err());
        assert_eq!(content.load(&session, Resource::Option, options[1]).await.unwrap(), before);
    }).await;
}
