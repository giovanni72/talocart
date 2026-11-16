use super::*;
use talocart_commerce::products::{BulkAction, ProductError, ProductSelection};

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn optional_category_single_http_publication_and_public_routes() {
    isolated(|pool, _| async move {
        let (identity, _, products, _, model, _, _) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let refs = products.entry(&session,model).await.unwrap();
        let class = refs.tax_class_id.unwrap();
        let category = refs.category_id.unwrap();
        let slug: String = sqlx::query_scalar("SELECT slug FROM catalog_categories WHERE id=$1").bind(category).fetch_one(&pool).await.unwrap();
        let app = router(test_app(pool.clone(),"Shop".into()).await);
        let cookie = format!("talocart_session={raw}");
        let form = format!("csrf={}&name=Without+category&model=NO-CATEGORY&slug=without-category&price=12.34&stock_revision=0&stock=3&minimum_quantity=1&packs=1&status=published&price_mode=inclusive&category_id=&tax_class_id={class}",session.csrf);
        let response = request(&app,"POST","/admin/catalog/products/new",&cookie,&form,false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER,"A product can be published without category");
        let id: i64 = sqlx::query_scalar("SELECT id FROM catalog_products WHERE model='NO-CATEGORY'").fetch_one(&pool).await.unwrap();
        assert_eq!(products.entry(&session,id).await.unwrap().category_id,None);
        assert_eq!(request(&app,"GET","/products/without-category","","",false).await.status(),StatusCode::OK);
        for path in ["/search/NO-CATEGORY","/","/sitemap-products.xml?page=1"] {
            let body=html(request(&app,"GET",path,"","",false).await).await;
            assert!(body.contains("without-category"),"Published uncategorized product appears at {path}");
        }
        let body=html(request(&app,"GET",&format!("/catalog/{slug}"),"","",false).await).await;
        assert!(!body.contains("/products/without-category"));
        let update = format!("{form}&brand_id={}",refs.brand_id.unwrap());
        assert_eq!(request(&app,"POST",&format!("/admin/catalog/products/{id}"),&cookie,&update,false).await.status(),StatusCode::SEE_OTHER);
        // A selected inactive category still blocks publication; no taxonomy assignment is forced.
        sqlx::query("UPDATE catalog_categories SET active=false WHERE id=$1").bind(category).execute(&pool).await.unwrap();
        let mut values=products.entry(&session,id).await.unwrap().values(); values.category_id=category.to_string();
        assert!(matches!(products.save(&session,&session.csrf,Some(id),&values).await,Err(ProductError::Publication)));
        assert!(products.public_entry("without-category").await.is_ok());
        values.category_id.clear(); values.tax_class_id.clear();
        assert!(matches!(products.save(&session,&session.csrf,Some(id),&values).await,Err(ProductError::Invalid(_))));
        sqlx::query("UPDATE tax_classes SET active=false WHERE id=$1").bind(class).execute(&pool).await.unwrap();
        let response=request(&app,"POST",&format!("/admin/catalog/products/{id}"),&cookie,&form,false).await;
        assert_eq!(response.status(),StatusCode::CONFLICT);
        let body=html(response).await;
        assert!(!body.contains(r#"id="category-error""#));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn optional_category_database_bulk_variants_and_tax_constraints() {
    isolated(|pool, _| async move {
        let (identity, session, products, variants, first, options, values) =
            variant_fixture(&pool).await;
        // Exercise the database constraint directly, independently of Rust validation.
        let result = sqlx::query("UPDATE catalog_products SET category_id=NULL WHERE id=$1")
            .bind(first)
            .execute(&pool)
            .await;
        assert!(
            result.is_ok(),
            "SQL publication also allows a missing category"
        );
        let entry = products.public_entry("model").await.unwrap();
        assert_eq!(entry.category_id, None);
        assert_eq!(entry.category_name, None);
        variants
            .configure(&session, &session.csrf, first, &options)
            .await
            .unwrap();
        assert!(
            matches!(
                products.public_entry("model").await,
                Err(ProductError::NotFound)
            ),
            "An incomplete variant product remains hidden"
        );
        variants
            .save(
                &session,
                &session.csrf,
                first,
                None,
                &variant_values("UNCAT-V", &[values[0], values[2]], "10", "2"),
            )
            .await
            .unwrap();
        assert!(products.public_entry("model").await.unwrap().has_variants);
        let raw = authenticated(&identity).await;
        let http_session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        assert_eq!(
            request(
                &app,
                "GET",
                &format!(
                    "/products/model?value_1={}&value_2={}",
                    values[0], values[2]
                ),
                "",
                "",
                false
            )
            .await
            .status(),
            StatusCode::OK
        );
        let mut other = products.entry(&session, first).await.unwrap().values();
        other.model = "SECOND-UNCAT".into();
        other.slug = "second-uncat".into();
        other.status = "draft".into();
        let second = products
            .save(&session, &session.csrf, None, &other)
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
        let form = format!(
            "csrf={}&ids={first}&ids={second}&action=publish&confirm=true",
            http_session.csrf
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/catalog/products/bulk/apply",
                &cookie,
                &form,
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        for slug in ["model", "second-uncat"] {
            assert!(products.public_entry(slug).await.is_ok());
        }
        assert!(
            sqlx::query("UPDATE catalog_products SET tax_class_id=NULL WHERE id=$1")
                .bind(second)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE catalog_products SET category_id=-1 WHERE id=$1")
                .bind(second)
                .execute(&pool)
                .await
                .is_err()
        );
        assert_eq!(
            products.entry(&session, second).await.unwrap().category_id,
            None
        );
    })
    .await;
}
