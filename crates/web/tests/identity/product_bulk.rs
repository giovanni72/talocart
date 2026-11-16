use super::*;
use talocart_commerce::products::{BulkAction, BulkActions, ProductError, ProductSelection};
use talocart_platform::identity::{Permission, Role};

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bulk_publish_draft_http_visibility_confirmation_and_preserved_data() {
    isolated(|pool, _| async move {
        let (identity, _, products, _, first, _, _) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let mut values = products.entry(&session, first).await.unwrap().values();
        values.name = "Product never shown in bulk summary".into();
        values.model = "BULK-STATUS".into();
        values.slug = "bulk-status".into();
        values.status = "draft".into();
        let second = products
            .save(&session, &session.csrf, None, &values)
            .await
            .unwrap();
        let untouched = products
            .save(
                &session,
                &session.csrf,
                None,
                &product_values("KEEP", "keep"),
            )
            .await
            .unwrap();
        let before: Vec<String> = sqlx::query_scalar(
            "SELECT (to_jsonb(p)-'status')::text FROM catalog_products p ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let form = format!("csrf={}&ids={first}&ids={second}", session.csrf);
        for (action, status, message) in [
            ("publish", "published", "Prodotti selezionati pubblicati."),
            (
                "draft",
                "draft",
                "Prodotti selezionati impostati come bozza.",
            ),
            ("publish", "published", "Prodotti selezionati pubblicati."),
        ] {
            let response = request(
                &app,
                "POST",
                "/admin/catalog/products/bulk/apply",
                &cookie,
                &format!("{form}&action={action}"),
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body = html(response).await;
            assert!(body.contains("2 prodotti selezionati."));
            assert!(!body.contains("<table"));
            assert!(!body.contains("Product never shown"));
            assert!(body.contains(&format!("value=\"{action}\" data-bulk-action checked")));
            assert!(body.contains("Pubblica prodotto"));
            assert!(body.contains("Bozza prodotto"));
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/admin/catalog/products/bulk/apply",
                    &cookie,
                    &format!("csrf=wrong&ids={first}&action={action}&confirm=true"),
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            let response = request(
                &app,
                "POST",
                "/admin/catalog/products/bulk/apply",
                &cookie,
                &format!("{form}&action={action}&confirm=true"),
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let location = response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .to_owned();
            assert_eq!(
                location,
                format!("/admin/catalog/products?bulk_status={status}")
            );
            let body = html(request(&app, "GET", &location, &cookie, "", false).await).await;
            assert!(body.contains(message));
            for (id, slug) in [(first, "model"), (second, "bulk-status")] {
                assert_eq!(products.entry(&session, id).await.unwrap().status, status);
                assert_eq!(
                    request(&app, "GET", &format!("/products/{slug}"), "", "", false)
                        .await
                        .status(),
                    if status == "published" {
                        StatusCode::OK
                    } else {
                        StatusCode::NOT_FOUND
                    }
                );
            }
            assert_eq!(
                products.entry(&session, untouched).await.unwrap().status,
                "draft"
            );
            let after: Vec<String> = sqlx::query_scalar(
                "SELECT (to_jsonb(p)-'status')::text FROM catalog_products p ORDER BY id",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(before, after, "Only product status changes");
        }
        // Retired deletion URL must not mutate products.
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/catalog/products/bulk/delete",
                &cookie,
                &format!("{form}&action=draft&confirm=true"),
                false
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            products.entry(&session, first).await.unwrap().status,
            "published"
        );
        let selection = ProductSelection::new(vec![first, second]).unwrap();
        products
            .apply_actions_selected(
                &session,
                &session.csrf,
                &selection,
                &talocart_commerce::products::BulkActions::new(&[BulkAction::Publish], None)
                    .unwrap(),
            )
            .await
            .unwrap();
        products
            .apply_actions_selected(
                &session,
                &session.csrf,
                &selection,
                &talocart_commerce::products::BulkActions::new(&[BulkAction::Draft], None).unwrap(),
            )
            .await
            .unwrap();
        let publish =
            talocart_commerce::products::BulkActions::new(&[BulkAction::Publish], None).unwrap();
        let draft =
            talocart_commerce::products::BulkActions::new(&[BulkAction::Draft], None).unwrap();
        let (a, b) = tokio::join!(
            products.apply_actions_selected(&session, &session.csrf, &selection, &publish),
            products.apply_actions_selected(&session, &session.csrf, &selection, &draft)
        );
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(
            products.entry(&session, first).await.unwrap().status,
            products.entry(&session, second).await.unwrap().status,
            "Concurrent batches cannot leave mixed states"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bulk_publication_rolls_back_every_product_for_missing_or_inactive_references() {
    isolated(|pool, _| async move {
        let (identity, session, products, _, first, _, _) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let http_session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let mut values = products.entry(&session, first).await.unwrap().values();
        values.status = "draft".into();
        values.model = "INVALID".into();
        values.slug = "invalid".into();
        let second = products
            .save(&session, &session.csrf, None, &values)
            .await
            .unwrap();
        let selection = ProductSelection::new(vec![first, second]).unwrap();
        products
            .apply_actions_selected(
                &session,
                &session.csrf,
                &selection,
                &talocart_commerce::products::BulkActions::new(&[BulkAction::Draft], None).unwrap(),
            )
            .await
            .unwrap();
        let category: i64 = values.category_id.parse().unwrap();
        let brand: i64 = values.brand_id.parse().unwrap();
        let class: i64 = values.tax_class_id.parse().unwrap();
        let rate: i64 = sqlx::query_scalar("SELECT rate_id FROM tax_classes WHERE id=$1")
            .bind(class)
            .fetch_one(&pool)
            .await
            .unwrap();
        for (table, id) in [
            ("catalog_categories", category),
            ("catalog_brands", brand),
            ("tax_classes", class),
            ("tax_rates", rate),
        ] {
            // The schema prevents deactivating a rate used by an active class.
            if table == "tax_rates" {
                sqlx::query("UPDATE tax_classes SET active=false WHERE id=$1")
                    .bind(class)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET active=false WHERE id=$1"
            )))
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
            let form = format!(
                "csrf={}&ids={first}&ids={second}&action=publish&confirm=true",
                http_session.csrf
            );
            let response = request(
                &app,
                "POST",
                "/admin/catalog/products/bulk/apply",
                &cookie,
                &form,
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let body = html(response).await;
            assert!(body.contains("Per pubblicare scegli una classe fiscale"));
            assert!(body.contains("value=\"publish\" data-bulk-action checked"));
            for id in [first, second] {
                assert_eq!(products.entry(&session, id).await.unwrap().status, "draft");
            }
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE {table} SET active=true WHERE id=$1"
            )))
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
            if table == "tax_rates" {
                sqlx::query("UPDATE tax_classes SET active=true WHERE id=$1")
                    .bind(class)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
        sqlx::query("UPDATE catalog_products SET category_id=NULL WHERE id=$1")
            .bind(second)
            .execute(&pool)
            .await
            .unwrap();
        products
            .apply_actions_selected(
                &session,
                &session.csrf,
                &selection,
                &talocart_commerce::products::BulkActions::new(&[BulkAction::Publish], None)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            products.public_entry("invalid").await.unwrap().category_id,
            None
        );
        products
            .apply_actions_selected(
                &session,
                &session.csrf,
                &selection,
                &talocart_commerce::products::BulkActions::new(&[BulkAction::Draft], None).unwrap(),
            )
            .await
            .unwrap();
        sqlx::query("UPDATE catalog_products SET category_id=$2,tax_class_id=NULL WHERE id=$1")
            .bind(second)
            .bind(category)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            products
                .apply_actions_selected(
                    &session,
                    &session.csrf,
                    &selection,
                    &talocart_commerce::products::BulkActions::new(&[BulkAction::Publish], None)
                        .unwrap()
                )
                .await,
            Err(ProductError::Publication)
        ));
        let missing = ProductSelection::new(vec![first, i64::MAX]).unwrap();
        for action in [BulkAction::Publish, BulkAction::Draft] {
            assert!(matches!(
                products
                    .apply_actions_selected(
                        &session,
                        &session.csrf,
                        &missing,
                        &talocart_commerce::products::BulkActions::new(&[action], Some(1)).unwrap()
                    )
                    .await,
                Err(ProductError::NotFound)
            ));
            assert_eq!(
                products.entry(&session, first).await.unwrap().status,
                "draft"
            );
            assert!(matches!(
                products
                    .apply_actions_selected(
                        &session,
                        "bad",
                        &selection,
                        &talocart_commerce::products::BulkActions::new(&[action], Some(1)).unwrap()
                    )
                    .await,
                Err(ProductError::Access(
                    talocart_platform::technical::AccessError::Forbidden
                ))
            ));
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bulk_http_preview_confirmation_validation_and_return_context() {
    isolated(|pool, _| async move {
        let (identity, _, products, _, first, _, _) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let mut values = product_values("BULK", "bulk");
        values.name = "Bulk <script> & name".into();
        let second = products.save(&session, &session.csrf, None, &values).await.unwrap();
        let untouched = products.save(&session, &session.csrf, None, &product_values("KEEP", "keep")).await.unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let list = html(request(&app, "GET", "/admin/catalog/products?model=BULK&per_page=10&sort=id&dir=desc", &cookie, "", false).await).await;
        assert!(list.contains("form=\"product-bulk\" id=\"product-bulk-button\""));
        assert!(list.contains("name=\"ids\" form=\"product-bulk\""));
        assert!(list.contains("model=BULK&#38;category=&#38;brand=&#38;sort=id&#38;dir=desc&#38;per_page=10&#38;page=1"));
        let form = format!("csrf={}&ids={first}&ids={second}&return_to=%2Fadmin%2Fcatalog%2Fproducts%3Fmodel%3DBULK%26per_page%3D10%26sort%3Did%26dir%3Ddesc%26page%3D2", session.csrf);
        let preview = request(&app, "POST", "/admin/catalog/products/bulk", &cookie, &form, false).await;
        assert_eq!(preview.status(), StatusCode::OK);
        let body = html(preview).await;
        assert!(body.contains("2 prodotti selezionati."));
        assert!(!body.contains("Bulk &#60;script&#62; &#38; name"));
        assert!(!body.contains("<table"));
        assert!(!body.contains("MODEL"));
        assert_eq!(body.matches("name=\"ids\"").count(), 2);
        assert!(products.entry(&session, first).await.is_ok());
        for path in ["/admin/catalog/products/bulk", "/admin/catalog/products/bulk/apply"] {
            assert_eq!(request(&app, "GET", path, &cookie, "", false).await.status(), StatusCode::METHOD_NOT_ALLOWED);
            assert_eq!(request(&app, "POST", path, &cookie, &format!("csrf=bad&ids={first}&action=delete&confirm=true"), false).await.status(), StatusCode::FORBIDDEN);
        }
        for ids in [String::new(), "&ids=0".into(), "&ids=-1".into(), "&ids=abc".into(), format!("&ids={first}&ids={first}"), (1..=501).map(|id|format!("&ids={id}")).collect()] {
            assert_eq!(request(&app, "POST", "/admin/catalog/products/bulk", &cookie, &format!("csrf={}{}", session.csrf, ids), false).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
        for suffix in ["&action=delete", "&action=unknown&confirm=true"] {
            let response = request(&app, "POST", "/admin/catalog/products/bulk/apply", &cookie, &format!("{form}{suffix}"), false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert!(html(response).await.contains("role=\"alert\""));
            assert!(products.entry(&session, first).await.is_ok());
        }
        let response = request(&app, "POST", "/admin/catalog/products/bulk/apply", &cookie, &format!("{form}&action=delete&confirm=true"), false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/admin/catalog/products?model=BULK&per_page=10&sort=id&dir=desc&page=2&bulk_deleted=1");
        assert!(matches!(products.entry(&session, first).await, Err(ProductError::NotFound)));
        assert!(matches!(products.entry(&session, second).await, Err(ProductError::NotFound)));
        assert!(products.entry(&session, untouched).await.is_ok());
        let repeat = request(&app, "POST", "/admin/catalog/products/bulk/apply", &cookie, &format!("{form}&action=delete&confirm=true"), false).await;
        assert_eq!(repeat.status(), StatusCode::NOT_FOUND);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bulk_delete_rolls_back_missing_or_restricted_and_serializes_concurrent_requests() {
    isolated(|pool, _| async move {
        let (_, session, products, _, first, _, _) = variant_fixture(&pool).await;
        let second = products.save(&session, &session.csrf, None, &product_values("SECOND", "second")).await.unwrap();
        let missing = ProductSelection::new(vec![first, i64::MAX]).unwrap();
        assert!(matches!(products.delete_selected(&session, &session.csrf, &missing).await, Err(ProductError::NotFound)));
        assert!(products.entry(&session, first).await.is_ok());
        sqlx::query("CREATE TABLE synthetic_product_references(product_id BIGINT REFERENCES catalog_products(id) ON DELETE RESTRICT)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO synthetic_product_references VALUES($1)").bind(second).execute(&pool).await.unwrap();
        let selection = ProductSelection::new(vec![second, first]).unwrap();
        assert!(matches!(products.delete_selected(&session, &session.csrf, &selection).await, Err(ProductError::InUse)));
        assert!(products.entry(&session, first).await.is_ok());
        assert!(products.entry(&session, second).await.is_ok());
        sqlx::query("DELETE FROM synthetic_product_references").execute(&pool).await.unwrap();
        let (a,b) = tokio::join!(products.delete_selected(&session, &session.csrf, &selection), products.delete_selected(&session, &session.csrf, &selection));
        assert!(matches!((&a,&b), (Ok(()), Err(ProductError::NotFound)) | (Err(ProductError::NotFound), Ok(()))));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bulk_delete_cascades_variants_gallery_and_preserves_shared_media() {
    isolated(|pool, _| async move {
        let (_, session, products, variants, first, options, values) = variant_fixture(&pool).await;
        let second = products
            .save(
                &session,
                &session.csrf,
                None,
                &product_values("SECOND", "second"),
            )
            .await
            .unwrap();
        let keep = products
            .save(
                &session,
                &session.csrf,
                None,
                &product_values("KEEP", "keep"),
            )
            .await
            .unwrap();
        variants
            .configure(&session, &session.csrf, first, &options)
            .await
            .unwrap();
        variants
            .save(
                &session,
                &session.csrf,
                first,
                None,
                &variant_values("V-BULK", &[values[0], values[2]], "10", "2"),
            )
            .await
            .unwrap();
        let png = include_bytes!("../fixtures/logo.png").to_vec();
        products
            .image(
                &session,
                &session.csrf,
                first,
                Some(png.clone()),
                "main.png",
            )
            .await
            .unwrap();
        products
            .add_gallery_image(&session, &session.csrf, first, png, "gallery.png")
            .await
            .unwrap();
        let key = products
            .entry(&session, first)
            .await
            .unwrap()
            .image_key
            .unwrap();
        sqlx::query("UPDATE catalog_products SET image_key=$1 WHERE id=$2")
            .bind(&key)
            .bind(keep)
            .execute(&pool)
            .await
            .unwrap();
        products
            .delete_selected(
                &session,
                &session.csrf,
                &ProductSelection::new(vec![first, second]).unwrap(),
            )
            .await
            .unwrap();
        for table in [
            "catalog_variants",
            "catalog_product_options",
            "catalog_product_images",
        ] {
            let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {table} WHERE product_id=$1"
            )))
            .bind(first)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(count, 0);
        }
        assert_eq!(
            products
                .entry(&session, keep)
                .await
                .unwrap()
                .image_key
                .as_deref(),
            Some(key.as_str())
        );
        let media = test_media(&pool).await;
        assert!(media.read(&key).await.is_ok());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM media_objects WHERE key=$1")
            .bind(&key)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bulk_requires_write_and_rechecks_revoked_permission_inside_transaction() {
    isolated(|pool, _| async move {
        let (identity, owner, products, _, first, _, _) = variant_fixture(&pool).await;
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Bulk reader",
                &[Permission::ProductsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "bulk@example.test",
                "correct horse reader".into(),
                Role::Custom(role.clone()),
            )
            .await
            .unwrap();
        let (_, anon) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anon,
                &anon.csrf,
                "bulk@example.test",
                "correct horse reader".into(),
            )
            .await
            .unwrap();
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let body =
            html(request(&app, "GET", "/admin/catalog/products", &cookie, "", false).await).await;
        assert!(!body.contains("product-bulk-button"));
        for path in [
            "/admin/catalog/products/bulk",
            "/admin/catalog/products/bulk/apply",
        ] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    path,
                    &cookie,
                    &format!(
                        "csrf={}&ids={first}&action=delete&confirm=true",
                        reader.csrf
                    ),
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        let selection = ProductSelection::new(vec![first]).unwrap();
        assert!(matches!(
            products
                .delete_selected(&reader, &reader.csrf, &selection)
                .await,
            Err(ProductError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        // Retain an in-memory editor session, then revoke its role permissions in the database.
        sqlx::query(
            "INSERT INTO admin_role_permissions(role_key,permission) VALUES($1,'products.write')",
        )
        .bind(&role)
        .execute(&pool)
        .await
        .unwrap();
        let editor = identity.session(&raw).await.unwrap().unwrap();
        assert!(editor.allows(Permission::ProductsWrite));
        sqlx::query(
            "DELETE FROM admin_role_permissions WHERE role_key=$1 AND permission='products.write'",
        )
        .bind(&role)
        .execute(&pool)
        .await
        .unwrap();
        assert!(matches!(
            products
                .delete_selected(&editor, &editor.csrf, &selection)
                .await,
            Err(ProductError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        for action in [
            BulkAction::Delete,
            BulkAction::Publish,
            BulkAction::Draft,
            BulkAction::AssignCategory,
        ] {
            assert!(matches!(
                products
                    .apply_actions_selected(
                        &reader,
                        &reader.csrf,
                        &selection,
                        &talocart_commerce::products::BulkActions::new(&[action], Some(1)).unwrap()
                    )
                    .await,
                Err(ProductError::Access(
                    talocart_platform::technical::AccessError::Forbidden
                ))
            ));
            assert!(matches!(
                products
                    .apply_actions_selected(
                        &editor,
                        &editor.csrf,
                        &selection,
                        &talocart_commerce::products::BulkActions::new(&[action], Some(1)).unwrap()
                    )
                    .await,
                Err(ProductError::Access(
                    talocart_platform::technical::AccessError::Forbidden
                ))
            ));
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/admin/catalog/products/bulk/apply",
                    &cookie,
                    &format!(
                        "csrf={}&ids={first}&action={}&confirm=true",
                        reader.csrf,
                        action.key()
                    ),
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        let category = products
            .entry(&owner, first)
            .await
            .unwrap()
            .category_id
            .unwrap();
        for actor in [&reader, &editor] {
            let actions = BulkActions::new(
                &[BulkAction::Publish, BulkAction::AssignCategory],
                Some(category),
            )
            .unwrap();
            assert!(matches!(
                products
                    .apply_actions_selected(actor, &actor.csrf, &selection, &actions)
                    .await,
                Err(ProductError::Access(
                    talocart_platform::technical::AccessError::Forbidden
                ))
            ));
            assert!(matches!(
                products
                    .assign_category_selected(actor, &actor.csrf, &selection, category)
                    .await,
                Err(ProductError::Access(
                    talocart_platform::technical::AccessError::Forbidden
                ))
            ));
        }
        assert!(products.entry(&owner, first).await.is_ok());
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn bulk_full_500_row_page_preview_apply_validation_and_preserved_outside_rows() {
    isolated(|pool,_| async move {
        let identity=installed(&pool).await;
        let raw=authenticated(&identity).await;
        let session=identity.session(&raw).await.unwrap().unwrap();
        let (_,_,class)=product_references(&pool,&session).await;
        let ids:Vec<i64>=sqlx::query_scalar("INSERT INTO catalog_products(id,name,model,slug,price_cents,tax_class_id) OVERRIDING SYSTEM VALUE SELECT 9223372036854775000+i,'Bulk large '||i,'BULK-LARGE-'||i,'bulk-large-'||i,100,$1 FROM generate_series(1,501)i RETURNING id")
            .bind(class).fetch_all(&pool).await.unwrap();
        let cookie=format!("talocart_session={raw}");
        let app=router(test_app(pool.clone(),"Bulk large".into()).await);
        let selected=ids[..500].iter().map(|id|format!("&ids={id}")).collect::<String>();
        for locale in ["it","en"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let filter="%25F0%259F%2598%2580".repeat(120);
            let form=format!("csrf={}{}&return_to=%2Fadmin%2Fcatalog%2Fproducts%3Fper_page%3D500%26q%3D{filter}%26model%3D{filter}",session.csrf,selected);
            assert!(form.len()>16*1024,"The largest valid selection and encoded filter need a route-local body limit");
            let response=request(&app,"POST","/admin/catalog/products/bulk",&cookie,&form,false).await;
            assert_eq!(response.status(),StatusCode::OK,"All 500 visible IDs must be accepted");
            let body=html(response).await;assert_eq!(body.matches("name=\"ids\"").count(),500);
            for (action,status) in [("publish","published"),("draft","draft")] {
                let response=request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&format!("{form}&action={action}"),false).await;
                assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY,"Confirmation remains mandatory for 500 rows");
                let response=request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&format!("{form}&action={action}&confirm=true"),false).await;
                assert_eq!(response.status(),StatusCode::SEE_OTHER);
                assert!(response.headers()[header::LOCATION].to_str().unwrap().contains("per_page=500"));
                assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_products WHERE id=ANY($1) AND status=$2").bind(&ids[..500]).bind(status).fetch_one(&pool).await.unwrap(),500);
                assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM catalog_products WHERE id=$1").bind(ids[500]).fetch_one(&pool).await.unwrap(),"draft");
            }
            for bad in [format!("{selected}&ids={}",ids[500]),format!("&ids={}&ids={}",ids[0],ids[0]),"&ids=0".into(),"&ids=-1".into(),"&ids=9223372036854775808".into()] {
                let response=request(&app,"POST","/admin/catalog/products/bulk",&cookie,&format!("csrf={}{bad}",session.csrf),false).await;
                assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
                let body=html(response).await;
                assert!(body.contains(if locale=="it" {"Seleziona da 1 a 500"} else {"Select 1 to 500"}));
            }
        }
    }).await;
}
