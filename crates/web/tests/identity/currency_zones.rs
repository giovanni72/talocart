include!("currency_zones_support.rs");

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn currency_selection_persists_in_empty_catalog() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let token = authenticated(&identity).await;
        let session = identity.session(&token).await.unwrap().unwrap();
        let app = router(test_app(pool.clone(), "Currency".into()).await);
        let response = fiscal_request(
            &app,
            "POST",
            "/admin/settings/taxes",
            &format!("talocart_session={token}"),
            &format!(
                "csrf={}&mode=inclusive&default_class_id=&currency=USD&country=IT",
                session.csrf
            ),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(identity.settings().await.unwrap().unwrap().currency, "USD");
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn currency_zones_catalog_preview_variants_fallback_and_protected_references() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let taxes = Taxes::new(pool.clone(), identity.clone());
        let base = class(&taxes, &s, "Base 50", "50").await;
        let local = class(&taxes, &s, "Zone 20", "20").await;
        let zone_id = taxes
            .save_zone(
                &s,
                &s.csrf,
                None,
                &zone_for("Italy", base, local, "IT,SM"),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        taxes
            .save_preferences(
                &s,
                &s.csrf,
                &preferences("USD", "IT"),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let products = Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let mut values = product_values("CURRENCY", "currency");
        values.currency = "USD".into();
        values.price_mode = "exclusive".into();
        values.price = "100".into();
        values.tax_class_id = base.to_string();
        values.status = "published".into();
        let id = products.save(&s, &s.csrf, None, &values).await.unwrap();
        let entry = products.public_entry("currency").await.unwrap();
        assert_eq!(entry.quote().unwrap().gross().cents(), 12000);
        assert_eq!(entry.currency, "USD");
        assert_eq!(entry.tax_class_id, Some(base));
        assert_eq!(entry.tax_name.as_deref(), Some("Base 50"));
        let mut preview = Preview {
            country: "store".into(),
            amount: "100".into(),
            quantity: "2".into(),
            class_id: base.to_string(),
        };
        let result = taxes.preview(&s, &s.csrf, &preview).await.unwrap();
        assert_eq!(result.totals.gross().cents(), 24000);
        assert_eq!(result.country, "IT");
        assert_eq!(result.currency, "USD");
        assert_eq!(result.zone_name.as_deref(), Some("Italy"));
        preview.country = "FR".into();
        let result = taxes.preview(&s, &s.csrf, &preview).await.unwrap();
        assert_eq!(result.totals.gross().cents(), 30000);
        assert!(result.zone_name.is_none());
        preview.country = "ZZ".into();
        assert!(matches!(
            taxes.preview(&s, &s.csrf, &preview).await,
            Err(TaxError::Invalid(_))
        ));
        let mut inactive = tax_values("Base 50", "", Some(1));
        inactive.active = false;
        assert!(matches!(
            taxes
                .save(
                    &s,
                    &s.csrf,
                    Kind::Class,
                    Some(base),
                    &inactive,
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::ActiveZones)
        ));
        assert!(matches!(
            taxes
                .delete(
                    &s,
                    &s.csrf,
                    Kind::Class,
                    base,
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::InUse)
        ));
        assert!(matches!(
            taxes
                .save_preferences(
                    &s,
                    &s.csrf,
                    &preferences("GBP", "FR"),
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::CurrencyInUse)
        ));
        assert_eq!(
            taxes.settings(&s).await.unwrap().country,
            "IT",
            "failed save rolls back every preference"
        );
        assert!(
            sqlx::query("UPDATE store_settings SET currency='GBP'")
                .execute(&pool)
                .await
                .is_err()
        );
        let mut inactive_zone = zone_for("Italy", base, local, "IT,SM");
        inactive_zone.active = false;
        taxes
            .save_zone(
                &s,
                &s.csrf,
                Some(zone_id),
                &inactive_zone,
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            products
                .public_entry("currency")
                .await
                .unwrap()
                .quote()
                .unwrap()
                .gross()
                .cents(),
            15000
        );
        taxes
            .delete_zone(&s, &s.csrf, zone_id, taxes.revision().await.unwrap())
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tax_zone_countries")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(products.entry(&s, id).await.unwrap().price_cents, 10000);
        // Currency changes do not become possible merely by unpublishing the product.
        values.status = "draft".into();
        products.save(&s, &s.csrf, Some(id), &values).await.unwrap();
        assert!(matches!(
            taxes
                .save_preferences(
                    &s,
                    &s.csrf,
                    &preferences("GBP", ""),
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::CurrencyInUse)
        ));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn currency_zones_validation_country_uniqueness_atomic_update_and_pagination() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let taxes = Taxes::new(pool.clone(), identity);
        let class_id = class(&taxes, &s, "Zero", "0").await;
        let a = taxes
            .save_zone(
                &s,
                &s.csrf,
                None,
                &zone("A", class_id, "IT"),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let b = taxes
            .save_zone(
                &s,
                &s.csrf,
                None,
                &zone("B", class_id, "FR"),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            taxes
                .save_zone(
                    &s,
                    &s.csrf,
                    Some(b),
                    &zone("Changed", class_id, "IT,DE"),
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::CountryInUse)
        ));
        let stored = taxes.zone(&s, b).await.unwrap();
        assert_eq!(stored.name, "B");
        assert_eq!(stored.countries, "FR");
        for countries in ["", "ZZ", "IT,IT"] {
            assert!(matches!(
                taxes
                    .save_zone(
                        &s,
                        &s.csrf,
                        None,
                        &zone("Invalid", class_id, countries),
                        taxes.revision().await.unwrap()
                    )
                    .await,
                Err(TaxError::Invalid(_))
            ));
        }
        for currency in ["JPY", "USD<script>", "usd", ""] {
            assert!(matches!(
                taxes
                    .save_preferences(
                        &s,
                        &s.csrf,
                        &preferences(currency, ""),
                        taxes.revision().await.unwrap()
                    )
                    .await,
                Err(TaxError::Invalid(_))
            ));
        }
        assert!(matches!(
            taxes
                .save_preferences(
                    &s,
                    &s.csrf,
                    &preferences("USD", "ZZ"),
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::Invalid(_))
        ));
        assert_eq!(taxes.settings(&s).await.unwrap().currency, "EUR");
        assert!(
            sqlx::query("INSERT INTO tax_zones(name) SELECT 'Empty' WHERE $1::bigint>0")
                .bind(class_id)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM tax_zone_countries WHERE zone_id=$1")
                .bind(a)
                .execute(&pool)
                .await
                .is_err()
        );
        for (index, code) in talocart_platform::geography::COUNTRIES
            .iter()
            .filter(|c| !["IT", "FR"].contains(c))
            .take(98)
            .enumerate()
        {
            taxes
                .save_zone(
                    &s,
                    &s.csrf,
                    None,
                    &zone(&format!("Zone {index:03}"), class_id, code),
                    taxes.revision().await.unwrap(),
                )
                .await
                .unwrap();
        }
        assert!(matches!(
            taxes
                .save_zone(
                    &s,
                    &s.csrf,
                    None,
                    &zone("Overflow", class_id, "ZW"),
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::Limit)
        ));
        let page = taxes.zones(&s, "", None, 999).await.unwrap();
        assert_eq!(page.total, 100);
        assert_eq!(page.page, 5);
        assert_eq!(page.entries.len(), 20);
        let filtered = taxes.zones(&s, "Zone 0", Some(true), 1).await.unwrap();
        assert_eq!(filtered.total, 98);
        assert_eq!(filtered.entries.len(), 20);
        assert!(matches!(
            taxes.zones(&s, "", None, 0).await,
            Err(TaxError::Invalid(_))
        ));
        assert!(matches!(
            taxes.zone(&s, 999999).await,
            Err(TaxError::NotFound)
        ));
        for code in talocart_platform::geography::CURRENCIES {
            taxes
                .save_preferences(
                    &s,
                    &s.csrf,
                    &preferences(code, ""),
                    taxes.revision().await.unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(taxes.settings(&s).await.unwrap().currency, *code);
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM countries")
                .fetch_one(&pool)
                .await
                .unwrap(),
            249
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn currency_zones_concurrent_first_product_and_stale_form_currency() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let taxes = Taxes::new(pool.clone(), identity.clone());
        let products = Products::new(pool.clone(), identity, test_media(&pool).await);
        let mut values = product_values("RACE", "race");
        values.price_mode = "exclusive".into();
        taxes
            .save_preferences(
                &s,
                &s.csrf,
                &preferences("EUR", ""),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let gbp = preferences("GBP", "");
        let (product, change) = tokio::join!(
            products.save(&s, &s.csrf, None, &values),
            taxes.save_preferences(&s, &s.csrf, &gbp, taxes.revision().await.unwrap())
        );
        assert_ne!(
            product.is_ok(),
            change.is_ok(),
            "exactly one incompatible operation can commit"
        );
        if product.is_ok() {
            assert!(matches!(change, Err(TaxError::CurrencyInUse)));
        } else {
            assert!(matches!(product, Err(ProductError::ModeChanged)));
            assert!(matches!(
                products.save(&s, &s.csrf, None, &values).await,
                Err(ProductError::ModeChanged)
            ));
        }
        let currency = taxes.settings(&s).await.unwrap().currency;
        let count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_products")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!((currency == "EUR" && count == 1) || (currency == "GBP" && count == 0));
        if count == 1 {
            products
                .delete(&s, &s.csrf, product.unwrap())
                .await
                .unwrap();
        }
        // Two zones racing for one country: one complete zone survives, no empty orphan.
        let c = class(&taxes, &s, "Race rate", "20").await;
        let one = zone("One", c, "IT");
        let two = zone("Two", c, "IT");
        let (one, two) = tokio::join!(
            taxes.save_zone(&s, &s.csrf, None, &one, taxes.revision().await.unwrap()),
            taxes.save_zone(&s, &s.csrf, None, &two, taxes.revision().await.unwrap())
        );
        assert_ne!(one.is_ok(), two.is_ok());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tax_zones")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn currency_zones_http_bilingual_forms_errors_csrf_permissions_and_public_prices() {
    isolated(|pool,_|async move {
        use talocart_platform::identity::{Permission,Role};
        let identity=installed(&pool).await;let raw=authenticated(&identity).await;let s=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let taxes=Taxes::new(pool.clone(),identity.clone());let class_id=class(&taxes,&s,"VAT","20").await;
        taxes.save_preferences(&s,&s.csrf,&preferences("USD","IT"), taxes.revision().await.unwrap()).await.unwrap();
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let values=format!("csrf={}&name=Europe&description=Preserve&rule_{class_id}={class_id}&countries=IT&countries=FR&active=true",s.csrf);
        let created=fiscal_request(&app,"POST","/admin/settings/taxes/zones/new",&cookie,&values,true).await;assert_eq!(created.status(),StatusCode::SEE_OTHER);
        let zone_id=taxes.zones(&s,"Europe",None,1).await.unwrap().entries[0].id;
        assert_eq!(taxes.zone(&s,zone_id).await.unwrap().countries,"FR, IT");
        for (locale,title,save) in [("it","Zone fiscali","Salva zona fiscale"),("en","Tax zones","Save tax zone")] {
            identity.update_locale(&s,&s.csrf,locale).await.unwrap();
            for path in ["/admin/settings/taxes/zones".into(),format!("/admin/settings/taxes/zones/{zone_id}")] {
                let r=fiscal_request(&app,"GET",&path,&cookie,"",true).await;assert_eq!(r.headers()[header::CONTENT_LANGUAGE],locale);let body=html(r).await;assert!(body.contains(title));assert!(!body.contains("admin-tax-zones"));
            }
            let invalid=values.replace("name=Europe","name=Failed").replace("rule_", "rule_999").replace("countries=FR","countries=ZZ");
            let r=fiscal_request(&app,"POST","/admin/settings/taxes/zones/new",&cookie,&invalid,false).await;assert_eq!(r.status(),StatusCode::UNPROCESSABLE_ENTITY);let body=html(r).await;assert!(body.contains("Preserve"));assert!(body.contains("Failed"));assert!(body.contains("ZZ"));assert!(body.contains(save));
            let r=fiscal_request(&app,"POST","/admin/settings/taxes/zones/new",&cookie,&values.replace(&s.csrf,"wrong"),true).await;assert_eq!(r.status(),StatusCode::FORBIDDEN);
            let r=fiscal_request(&app,"POST","/admin/settings/taxes/preview",&cookie,&format!("csrf={}&amount=100&quantity=1&class_id={class_id}&country=IT",s.csrf),true).await;assert_eq!(r.status(),StatusCode::OK);let body=html(r).await;assert!(body.contains(if locale=="it" {"120,00 USD"} else {"120.00 USD"}));assert!(body.contains("Europe (IT)"));assert!(!body.contains("€"));if locale=="en" {assert!(body.contains("20.00%"));}
        }
        let role=identity.create_role(&s,&s.csrf,"Fiscal reader",&[Permission::TaxesRead]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"reader@example.test","correct horse reader".into(),Role::Custom(role)).await.unwrap();
        let (_,anon)=identity.anonymous().await.unwrap();let reader_raw=identity.login(&anon,&anon.csrf,"reader@example.test","correct horse reader".into()).await.unwrap();let reader=identity.session(&reader_raw).await.unwrap().unwrap();let reader_cookie=format!("talocart_session={reader_raw}");
        let r=fiscal_request(&app,"GET",&format!("/admin/settings/taxes/zones/{zone_id}"),&reader_cookie,"",false).await;assert_eq!(r.status(),StatusCode::OK);assert!(!html(r).await.contains("Salva zona fiscale"));
        assert_eq!(fiscal_request(&app,"POST",&format!("/admin/settings/taxes/zones/{zone_id}/delete"),&reader_cookie,&format!("csrf={}&confirm=true",reader.csrf),true).await.status(),StatusCode::FORBIDDEN);
        assert!(matches!(taxes.save_zone(&reader,&reader.csrf,None,&zone("Reader",class_id,"DE"), taxes.revision().await.unwrap()).await,Err(TaxError::Access(talocart_platform::technical::AccessError::Forbidden))));
        assert_eq!(fiscal_request(&app,"GET","/admin/settings/taxes/zones","","",false).await.status(),StatusCode::SEE_OTHER);
        assert_eq!(fiscal_request(&app,"POST",&format!("/admin/settings/taxes/zones/{zone_id}/delete"),&cookie,&format!("csrf={}",s.csrf),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert!(taxes.zone(&s,zone_id).await.is_ok());
        // Public list, detail and HTMX variant projections share the same saved currency.
        let products=Products::new(pool.clone(),identity.clone(),test_media(&pool).await);
        let mut product=product_values("USD","usd");product.currency="USD".into();product.price_mode="exclusive".into();product.price="100".into();product.tax_class_id=class_id.to_string();product.status="published".into();
        products.save(&s,&s.csrf,None,&product).await.unwrap();
        for (path,amount) in [("/catalog","120,00 USD"),("/en/catalog","120.00 USD"),("/products/usd","120,00 USD"),("/en/products/usd","120.00 USD"),("/admin/catalog/products","100.00 USD")] {
            let body=html(fiscal_request(&app,"GET",path,&cookie,"",false).await).await;assert!(body.contains(amount),"{path}");assert!(!body.contains("€"),"{path}");
        }
        let quote=taxes.preview(&s,&s.csrf,&Preview{country:"IT".into(),amount:"0.01".into(),quantity:"3".into(),class_id:class_id.to_string()}).await.unwrap();assert_eq!(quote.mode,PriceMode::Exclusive);assert_eq!(quote.totals.tax().cents(),1);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn currency_zones_inclusive_rounding_and_explicit_empty_country() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let taxes = Taxes::new(pool, identity);
        let base = class(&taxes, &s, "Base 50", "50").await;
        let local = class(&taxes, &s, "Zone 20", "20").await;
        taxes
            .save_zone(
                &s,
                &s.csrf,
                None,
                &zone_for("Italy", base, local, "IT"),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let mut config = preferences("CHF", "IT");
        config.mode = "inclusive".into();
        taxes
            .save_preferences(&s, &s.csrf, &config, taxes.revision().await.unwrap())
            .await
            .unwrap();
        let mut input = Preview {
            country: "store".into(),
            amount: "100".into(),
            quantity: "1".into(),
            class_id: base.to_string(),
        };
        let quote = taxes.preview(&s, &s.csrf, &input).await.unwrap();
        assert_eq!(quote.mode, PriceMode::Inclusive);
        assert_eq!(
            (
                quote.totals.net().cents(),
                quote.totals.tax().cents(),
                quote.totals.gross().cents()
            ),
            (8333, 1667, 10000)
        );
        input.country = String::new();
        let quote = taxes.preview(&s, &s.csrf, &input).await.unwrap();
        assert!(quote.zone_name.is_none());
        assert_eq!(quote.totals.net().cents(), 6667);
        input.country = "store".into();
        input.amount = "0.01".into();
        input.quantity = "3".into();
        let quote = taxes.preview(&s, &s.csrf, &input).await.unwrap();
        assert_eq!(
            (
                quote.totals.net().cents(),
                quote.totals.tax().cents(),
                quote.totals.gross().cents()
            ),
            (3, 0, 3)
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn fiscal_rules_preserve_product_classes_zero_fallback_and_shared_resolution() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let taxes = Taxes::new(pool.clone(), identity.clone());
        let standard = class(&taxes, &s, "Standard", "22").await;
        let reduced = class(&taxes, &s, "Reduced", "10").await;
        let zero = class(&taxes, &s, "Zero", "0").await;
        let foreign = taxes
            .save(
                &s,
                &s.csrf,
                Kind::Rate,
                None,
                &tax_values("Destination", "20", None),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let mut z = zone_for("Destination", standard, foreign, "IT");
        z.rules.insert(zero, zero);
        taxes
            .save_zone(&s, &s.csrf, None, &z, taxes.revision().await.unwrap())
            .await
            .unwrap();
        taxes
            .save_preferences(
                &s,
                &s.csrf,
                &preferences("EUR", "IT"),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let products = Products::new(pool.clone(), identity, test_media(&pool).await);
        for (class_id, name, gross, explicit) in [
            (standard, "standard", 12000, true),
            (reduced, "reduced", 11000, false),
            (zero, "zero", 10000, true),
        ] {
            let mut v = product_values(name, name);
            v.tax_class_id = class_id.to_string();
            v.status = "published".into();
            v.price_mode = "exclusive".into();
            v.price = "100".into();
            products.save(&s, &s.csrf, None, &v).await.unwrap();
            let product = products.public_entry(name).await.unwrap();
            assert_eq!(product.tax_class_id, Some(class_id));
            assert_eq!(product.quote().unwrap().gross().cents(), gross);
            let quote = taxes
                .preview(
                    &s,
                    &s.csrf,
                    &Preview {
                        country: "IT".into(),
                        amount: "100".into(),
                        quantity: "1".into(),
                        class_id: class_id.to_string(),
                    },
                )
                .await
                .unwrap();
            assert_eq!(quote.totals.gross().cents(), gross);
            assert_eq!(quote.zone_name.is_some(), explicit);
        }
        let countries = talocart_commerce::countries::CheckoutCountries::new(
            pool.clone(),
            Identity::new(pool.clone()),
        );
        countries
            .save(
                &s,
                &s.csrf,
                &["IT".into(), "FR".into()],
                countries.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let coverage = taxes.coverage(&s).await.unwrap();
        let it = coverage.iter().find(|c| c.country == "IT").unwrap();
        assert_eq!((it.classes, it.explicit, it.enabled), (3, 2, true));
        let fr = coverage.iter().find(|c| c.country == "FR").unwrap();
        assert_eq!((fr.explicit, fr.enabled), (0, true));
        assert!(fr.zone_id.is_none());
        let before = taxes.revision().await.unwrap();
        let mut invalid = z.clone();
        invalid.rules.insert(99999, foreign);
        assert!(
            taxes
                .save_zone(&s, &s.csrf, Some(1), &invalid, before)
                .await
                .is_err()
        );
        assert_eq!(taxes.revision().await.unwrap(), before);
        assert_eq!(taxes.zone(&s, 1).await.unwrap().values().rules, z.rules);
        let mut inactive = tax_values("Destination", "20", None);
        inactive.active = false;
        assert!(matches!(
            taxes
                .save(&s, &s.csrf, Kind::Rate, Some(foreign), &inactive, before)
                .await,
            Err(TaxError::ActiveZones)
        ));
        z.active = false;
        taxes
            .save_zone(&s, &s.csrf, Some(1), &z, before)
            .await
            .unwrap();
        assert_eq!(
            products
                .public_entry("standard")
                .await
                .unwrap()
                .quote()
                .unwrap()
                .gross()
                .cents(),
            12200
        );
        assert_eq!(
            taxes
                .coverage(&s)
                .await
                .unwrap()
                .iter()
                .find(|c| c.country == "IT")
                .unwrap()
                .explicit,
            0
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn fiscal_stale_revisions_reject_writes_deletes_and_concurrent_overwrites() {
    use talocart_commerce::countries::{CheckoutCountries, CountryError};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let taxes = Taxes::new(pool.clone(), identity.clone());
        let countries = CheckoutCountries::new(pool, identity);
        let class_id = class(&taxes, &s, "Base", "22").await;
        let v = zone("Zone", class_id, "IT");
        let id = taxes
            .save_zone(&s, &s.csrf, None, &v, taxes.revision().await.unwrap())
            .await
            .unwrap();
        let stale = taxes.revision().await.unwrap();
        countries
            .save(&s, &s.csrf, &["IT".into()], stale)
            .await
            .unwrap();
        assert!(matches!(
            taxes
                .save_preferences(&s, &s.csrf, &preferences("USD", "FR"), stale)
                .await,
            Err(TaxError::Stale)
        ));
        assert!(matches!(
            taxes.save_zone(&s, &s.csrf, Some(id), &v, stale).await,
            Err(TaxError::Stale)
        ));
        assert!(matches!(
            taxes.delete_zone(&s, &s.csrf, id, stale).await,
            Err(TaxError::Stale)
        ));
        assert!(matches!(
            taxes
                .save(
                    &s,
                    &s.csrf,
                    Kind::Rate,
                    Some(1),
                    &tax_values("Changed", "10", None),
                    stale
                )
                .await,
            Err(TaxError::Stale)
        ));
        assert!(matches!(
            taxes
                .delete(&s, &s.csrf, Kind::Class, class_id, stale)
                .await,
            Err(TaxError::Stale)
        ));
        assert!(matches!(
            countries.save(&s, &s.csrf, &[], stale).await,
            Err(CountryError::Stale)
        ));
        assert_eq!(countries.destinations().await.unwrap(), ["IT"]);
        assert_eq!(taxes.settings(&s).await.unwrap().currency, "EUR");
        let rev = taxes.revision().await.unwrap();
        let a = vec!["FR".into()];
        let b = vec!["DE".into()];
        let (one, two) = tokio::join!(
            countries.save(&s, &s.csrf, &a, rev),
            countries.save(&s, &s.csrf, &b, rev)
        );
        assert_ne!(one.is_ok(), two.is_ok());
        assert!(matches!(one, Err(CountryError::Stale)) || matches!(two, Err(CountryError::Stale)));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn fiscal_http_coverage_and_stale_forms_bilingual_with_no_implicit_revision() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let token = authenticated(&identity).await;
        let s = identity.session(&token).await.unwrap().unwrap();
        let cookie = format!("talocart_session={token}");
        let taxes = Taxes::new(pool.clone(), identity.clone());
        let class_id = class(&taxes, &s, "Base", "22").await;
        let app = router(test_app(pool.clone(), "Audit".into()).await);
        assert_eq!(
            request(&app, "GET", "/admin/settings/taxes/coverage", "", "", false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/settings/taxes/coverage?page=10001",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let empty = html(
            request(
                &app,
                "GET",
                "/admin/settings/taxes/coverage",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(empty.contains("ui-empty"));
        let all = html(
            request(
                &app,
                "GET",
                "/admin/settings/taxes/coverage?all=true&page=999",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert_eq!(
            all.matches("data-country=").count(),
            9,
            "249 countries: last page contains nine rows"
        );
        for (language, label, message) in [
            (
                "it",
                "Copertura fiscale per paese",
                "La configurazione fiscale è cambiata",
            ),
            ("en", "Tax coverage by country", "Tax configuration changed"),
        ] {
            identity.update_locale(&s, &s.csrf, language).await.unwrap();
            let response = request(
                &app,
                "GET",
                "/admin/settings/taxes/coverage?all=true&q=IT",
                &cookie,
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = html(response).await;
            assert!(body.contains(label));
            assert!(body.contains("data-country=\"IT\""));
            let revision = taxes.revision().await.unwrap();
            taxes
                .save(
                    &s,
                    &s.csrf,
                    Kind::Rate,
                    Some(1),
                    &tax_values("Base", "22", None),
                    revision,
                )
                .await
                .unwrap();
            for (path, fields) in [
                (
                    "/admin/settings/taxes",
                    format!("mode=exclusive&currency=USD&country=IT&default_class_id={class_id}"),
                ),
                ("/admin/settings/countries", "country=FR".into()),
                (
                    "/admin/settings/taxes/zones/new",
                    format!("name=Keep+me&countries=IT&active=true&rule_{class_id}=1"),
                ),
                (
                    "/admin/settings/taxes/rates/new",
                    "name=Keep+me&percentage=10&active=true".into(),
                ),
            ] {
                let response = request(
                    &app,
                    "POST",
                    path,
                    &cookie,
                    &format!("csrf={}&revision={revision}&{fields}", s.csrf),
                    false,
                )
                .await;
                assert_eq!(response.status(), StatusCode::CONFLICT, "{path}");
                let body = html(response).await;
                assert!(body.contains(message), "{path}");
                assert!(
                    body.contains(&format!("name=\"revision\" value=\"{revision}\"")),
                    "submitted revision stays stale"
                );
            }
            let missing = request(
                &app,
                "POST",
                "/admin/settings/countries",
                &cookie,
                &format!("csrf={}&country=IT", s.csrf),
                false,
            )
            .await;
            assert_eq!(missing.status(), StatusCode::CONFLICT);
        }
        assert_eq!(taxes.zones(&s, "", None, 1).await.unwrap().total, 0);
        assert!(
            sqlx::query_scalar::<_, String>("SELECT country_code FROM checkout_countries")
                .fetch_all(&pool)
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await;
}
