use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn fiscal_configuration_rules_exact_preview_and_persistence() {
    use talocart_commerce::{
        pricing::PriceMode,
        taxes::{Kind, Preferences, Preview, TaxError, Taxes},
    };
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let taxes = Taxes::new(pool.clone(), identity.clone());
        let initial = taxes.settings(&session).await.unwrap();
        assert_eq!(initial.mode, PriceMode::Inclusive);
        assert_eq!(initial.default_class_id, None);
        assert!(
            taxes
                .choices(&session, Kind::Rate)
                .await
                .unwrap()
                .is_empty()
        );
        let rate = taxes
            .save(
                &session,
                &session.csrf,
                Kind::Rate,
                None,
                &tax_values("Demo 50", "50", None),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            taxes
                .save(
                    &session,
                    &session.csrf,
                    Kind::Rate,
                    None,
                    &tax_values("demo 50", "50", None),
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::Duplicate)
        ));
        let class = taxes
            .save(
                &session,
                &session.csrf,
                Kind::Class,
                None,
                &tax_values("Demo class", "", Some(rate)),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let prefs = Preferences {
            currency: "EUR".into(),
            country: String::new(),
            mode: "inclusive".into(),
            default_class_id: class.to_string(),
        };
        taxes
            .save_preferences(
                &session,
                &session.csrf,
                &prefs,
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let quote = taxes
            .preview(
                &session,
                &session.csrf,
                &Preview {
                    country: "store".into(),
                    amount: "0,05".into(),
                    quantity: "1".into(),
                    class_id: class.to_string(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            (
                quote.totals.net().cents(),
                quote.totals.tax().cents(),
                quote.totals.gross().cents()
            ),
            (3, 2, 5)
        );
        let mut rate_values = tax_values("Demo 50", "50", None);
        rate_values.active = false;
        assert!(matches!(
            taxes
                .save(
                    &session,
                    &session.csrf,
                    Kind::Rate,
                    Some(rate),
                    &rate_values,
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::ActiveClasses)
        ));
        let mut class_values = tax_values("Demo class", "", Some(rate));
        class_values.active = false;
        assert!(matches!(
            taxes
                .save(
                    &session,
                    &session.csrf,
                    Kind::Class,
                    Some(class),
                    &class_values,
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::DefaultInUse)
        ));
        for (kind, id) in [(Kind::Rate, rate), (Kind::Class, class)] {
            assert!(matches!(
                taxes
                    .delete(
                        &session,
                        &session.csrf,
                        kind,
                        id,
                        taxes.revision().await.unwrap()
                    )
                    .await,
                Err(TaxError::InUse)
            ));
        }
        taxes
            .save_preferences(
                &session,
                &session.csrf,
                &Preferences {
                    currency: "EUR".into(),
                    country: String::new(),
                    mode: "exclusive".into(),
                    default_class_id: String::new(),
                },
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let quote = taxes
            .preview(
                &session,
                &session.csrf,
                &Preview {
                    country: "store".into(),
                    amount: "0.01".into(),
                    quantity: "3".into(),
                    class_id: class.to_string(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            (
                quote.totals.net().cents(),
                quote.totals.tax().cents(),
                quote.totals.gross().cents()
            ),
            (3, 2, 5)
        );
        for input in [
            Preview {
                country: "store".into(),
                amount: "0.001".into(),
                quantity: "1".into(),
                class_id: class.to_string(),
            },
            Preview {
                country: "store".into(),
                amount: "1".into(),
                quantity: "0".into(),
                class_id: class.to_string(),
            },
            Preview {
                country: "store".into(),
                amount: "1".into(),
                quantity: "1".into(),
                class_id: "999999".into(),
            },
        ] {
            assert!(matches!(
                taxes.preview(&session, &session.csrf, &input).await,
                Err(TaxError::Invalid(_))
            ));
        }
        assert!(matches!(
            taxes
                .preview(
                    &session,
                    &session.csrf,
                    &Preview {
                        country: "store".into(),
                        amount: "92233720368547758.07".into(),
                        quantity: "2".into(),
                        class_id: class.to_string()
                    }
                )
                .await,
            Err(TaxError::Overflow)
        ));
        taxes
            .save(
                &session,
                &session.csrf,
                Kind::Class,
                Some(class),
                &class_values,
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            taxes
                .save_preferences(
                    &session,
                    &session.csrf,
                    &prefs,
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::InvalidDefault)
        ));
        taxes
            .save(
                &session,
                &session.csrf,
                Kind::Rate,
                Some(rate),
                &rate_values,
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        class_values.active = true;
        assert!(matches!(
            taxes
                .save(
                    &session,
                    &session.csrf,
                    Kind::Class,
                    Some(class),
                    &class_values,
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::InactiveRate)
        ));
        let restarted = Taxes::new(pool.clone(), Identity::new(pool.clone()));
        assert_eq!(
            restarted.settings(&session).await.unwrap().mode,
            PriceMode::Exclusive
        );
        assert!(
            sqlx::query("DELETE FROM pricing_settings")
                .execute(&pool)
                .await
                .is_err()
        );
        taxes
            .delete(
                &session,
                &session.csrf,
                Kind::Class,
                class,
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        taxes
            .delete(
                &session,
                &session.csrf,
                Kind::Rate,
                rate,
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn fiscal_http_validation_csrf_filters_pagination_and_escaping() {
    use talocart_commerce::taxes::{Kind, Taxes};
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let raw=authenticated(&identity).await;let session=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let app=router(test_app(pool.clone(),"Shop".into()).await);let taxes=Taxes::new(pool.clone(),identity.clone());
        for path in ["/admin/settings/taxes","/admin/settings/taxes/rates","/admin/settings/taxes/classes"] {
            assert_eq!(fiscal_request(&app,"GET",path,"","",false).await.status(),StatusCode::SEE_OTHER);
            assert_eq!(fiscal_request(&app,"GET",path,&cookie,"",true).await.status(),StatusCode::OK);
        }
        for path in ["/admin/settings/taxes/rates/new","/admin/settings/taxes/classes/new","/admin/settings/taxes","/admin/settings/taxes/preview"] {
            let body="csrf=wrong&name=Demo&percentage=50&rate_id=1&mode=inclusive&amount=1&quantity=1&class_id=1";
            assert_eq!(fiscal_request(&app,"POST",path,&cookie,body,true).await.status(),StatusCode::FORBIDDEN);
        }
        let response=fiscal_request(&app,"POST","/admin/settings/taxes/rates/new",&cookie,&format!("csrf={}&name=Demo&description=Keep&percentage=100.01&active=true",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let body=html(response).await;assert!(body.contains("percentage-error"));assert!(body.contains("value=\"100.01\""));assert!(body.contains("Keep"));
        let response=fiscal_request(&app,"POST","/admin/settings/taxes/rates/new",&cookie,&format!("csrf={}&name=Demo&percentage=7.50&active=true",session.csrf),false).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);
        let rate=taxes.list(&session,Kind::Rate,"Demo",None,1).await.unwrap()[0].id;
        let response=fiscal_request(&app,"POST","/admin/settings/taxes/classes/new",&cookie,&format!("csrf={}&name=Class&rate_id={rate}&active=true",session.csrf),false).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);
        let class=taxes.choices(&session,Kind::Class).await.unwrap()[0].id;
        assert_eq!(fiscal_request(&app,"GET",&format!("/admin/settings/taxes/classes/{class}"),&cookie,"",false).await.status(),StatusCode::OK);
        assert_eq!(fiscal_request(&app,"GET",&format!("/admin/settings/taxes/classes/{class}/delete"),&cookie,"",false).await.status(),StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(fiscal_request(&app,"POST","/admin/settings/taxes",&cookie,&format!("csrf={}&mode=invalid&default_class_id=missing",session.csrf),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(fiscal_request(&app,"POST","/admin/settings/taxes",&cookie,&format!("csrf={}&mode=exclusive&default_class_id={class}",session.csrf),false).await.status(),StatusCode::SEE_OTHER);
        let response=fiscal_request(&app,"POST","/admin/settings/taxes/preview",&cookie,&format!("csrf={}&amount=1.00&quantity=2&class_id={class}",session.csrf),false).await;assert_eq!(response.status(),StatusCode::OK);let body=html(response).await;assert!(body.contains("2,15 €"));assert!(body.contains("0,15 €"));
        let response=fiscal_request(&app,"POST",&format!("/admin/settings/taxes/rates/{rate}/delete"),&cookie,&format!("csrf={}&confirm=true",session.csrf),false).await;assert_eq!(response.status(),StatusCode::CONFLICT);assert!(html(response).await.contains("collegata"));
        for n in 0..22 {taxes.save(&session,&session.csrf,Kind::Rate,None,&tax_values(&format!("Nome {n:02}"),"0",None), taxes.revision().await.unwrap()).await.unwrap();}
        let mut values=tax_values("<script>alert(1)</script>","0",None);values.active=false;let escaped=taxes.save(&session,&session.csrf,Kind::Rate,None,&values, taxes.revision().await.unwrap()).await.unwrap();
        let body=html(fiscal_request(&app,"GET",&format!("/admin/settings/taxes/rates/{escaped}"),&cookie,"",false).await).await;assert!(body.contains("&#60;script&#62;"));assert!(!body.contains("<script>alert"));
        for (query,count) in [("",20),("?page=2",4),("?q=Nome%2021",1),("?q=%25",0),("?status=inactive",1)] {
            let body=html(fiscal_request(&app,"GET",&format!("/admin/settings/taxes/rates{query}"),&cookie,"",false).await).await;assert_eq!(body.matches("role=\"listitem\"").count(),count, "{query}");
        }
        for query in ["?page=0","?status=bad","?q=%00"] {assert_eq!(fiscal_request(&app,"GET",&format!("/admin/settings/taxes/rates{query}"),&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);}
        assert_eq!(fiscal_request(&app,"GET","/admin/settings/taxes/rates/999999",&cookie,"",false).await.status(),StatusCode::NOT_FOUND);
        assert_eq!(fiscal_request(&app,"POST","/admin/settings/taxes/rates/999999",&cookie,&format!("csrf={}&revision={}&name=Lost&percentage=0",session.csrf,taxes.revision().await.unwrap()),false).await.status(),StatusCode::NOT_FOUND);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn fiscal_permissions_read_only_landing_normalization_and_revocation() {
    use talocart_commerce::taxes::{Kind, TaxError, Taxes};
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let owner = taxonomy_session(&identity).await;
        let taxes = Taxes::new(pool.clone(), identity.clone());
        let rate = taxes
            .save(
                &owner,
                &owner.csrf,
                Kind::Rate,
                None,
                &tax_values("Demo", "0", None),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Fiscal reader",
                &[Permission::TaxesRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "fiscal@example.test",
                "correct horse reader".into(),
                Role::Custom(role.clone()),
            )
            .await
            .unwrap();
        let login = |identity: Identity| async move {
            let (_, anon) = identity.anonymous().await.unwrap();
            identity
                .login(
                    &anon,
                    &anon.csrf,
                    "fiscal@example.test",
                    "correct horse reader".into(),
                )
                .await
                .unwrap()
        };
        let raw = login(identity.clone()).await;
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let login_page = fiscal_request(&app, "GET", "/admin/login", "", "", false).await;
        let anonymous_cookie = self::cookie(&login_page);
        let login_csrf = csrf(&html(login_page).await);
        let response = fiscal_request(
            &app,
            "POST",
            "/admin/login",
            &anonymous_cookie,
            &format!("csrf={login_csrf}&email=fiscal%40example.test&password=correct+horse+reader"),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/admin/settings/taxes"
        );
        let body =
            html(fiscal_request(&app, "GET", "/admin/settings/taxes", &cookie, "", false).await)
                .await;
        assert!(!body.contains("Salva preferenze"));
        assert!(body.contains("Calcola"));
        let body = html(
            fiscal_request(
                &app,
                "GET",
                &format!("/admin/settings/taxes/rates/{rate}"),
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(body.contains("readonly"));
        assert!(!body.contains("Salva aliquota"));
        assert_eq!(
            fiscal_request(
                &app,
                "GET",
                "/admin/settings/taxes/rates/new",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fiscal_request(
                &app,
                "POST",
                "/admin/settings/taxes/rates/new",
                &cookie,
                &format!("csrf={}&name=Forbidden&percentage=0", reader.csrf),
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fiscal_request(&app, "GET", "/admin/settings", &cookie, "", false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        identity
            .update_role(
                &owner,
                &owner.csrf,
                &role,
                "Fiscal writer",
                &[Permission::TaxesWrite],
            )
            .await
            .unwrap();
        let raw = login(identity.clone()).await;
        let writer = identity.session(&raw).await.unwrap().unwrap();
        assert!(writer.allows(Permission::TaxesRead));
        taxes
            .save(
                &writer,
                &writer.csrf,
                Kind::Rate,
                None,
                &tax_values("Allowed", "0", None),
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
        identity
            .update_role(
                &owner,
                &owner.csrf,
                &role,
                "Fiscal reader",
                &[Permission::TaxesRead],
            )
            .await
            .unwrap();
        assert!(matches!(
            taxes
                .save(
                    &writer,
                    &writer.csrf,
                    Kind::Rate,
                    None,
                    &tax_values("Revoked", "0", None),
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert_eq!(
            fiscal_request(
                &app,
                "GET",
                "/admin/settings/taxes/rates/new",
                &format!("talocart_session={raw}"),
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
async fn fiscal_database_guards_concurrent_changes_and_limits() {
    isolated(|pool,_|async move {
        let rate:i64=sqlx::query_scalar("INSERT INTO tax_rates(name,basis_points) VALUES('Concurrent',0) RETURNING id").fetch_one(&pool).await.unwrap();
        // Direct independent connections bypass the application lock: database invariants must still hold.
        let (a,b)=tokio::join!(
            sqlx::query("UPDATE tax_rates SET active=false WHERE id=$1").bind(rate).execute(&pool),
            sqlx::query("INSERT INTO tax_classes(name,rate_id) VALUES('Concurrent',$1)").bind(rate).execute(&pool)
        );
        assert!(a.is_ok() || b.is_ok());assert!(!(a.is_ok() && b.is_ok()));
        let broken:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tax_classes c JOIN tax_rates r ON r.id=c.rate_id WHERE c.active AND NOT r.active)").fetch_one(&pool).await.unwrap();assert!(!broken);
        sqlx::query("UPDATE tax_rates SET active=true WHERE id=$1").bind(rate).execute(&pool).await.unwrap();
        let class:i64=sqlx::query_scalar("INSERT INTO tax_classes(name,rate_id) VALUES('Default race',$1) RETURNING id").bind(rate).fetch_one(&pool).await.unwrap();
        let(a,b)=tokio::join!(
            sqlx::query("UPDATE tax_classes SET active=false WHERE id=$1").bind(class).execute(&pool),
            sqlx::query("UPDATE pricing_settings SET default_class_id=$1 WHERE id=1").bind(class).execute(&pool)
        );assert!(a.is_ok() || b.is_ok());assert!(!(a.is_ok() && b.is_ok()));
        let broken:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pricing_settings p JOIN tax_classes c ON c.id=p.default_class_id WHERE NOT c.active)").fetch_one(&pool).await.unwrap();assert!(!broken);
        // Unique names and the final capacity slot also remain protected under simultaneous inserts.
        let(a,b)=tokio::join!(sqlx::query("INSERT INTO tax_rates(name,basis_points) VALUES('Unique',0)").execute(&pool),sqlx::query("INSERT INTO tax_rates(name,basis_points) VALUES('unique',0)").execute(&pool));assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        let existing:i64=sqlx::query_scalar("SELECT count(*) FROM tax_rates").fetch_one(&pool).await.unwrap();
        for n in existing..99 {sqlx::query("INSERT INTO tax_rates(name,basis_points) VALUES($1,0)").bind(format!("Capacity {n}")).execute(&pool).await.unwrap();}
        let(a,b)=tokio::join!(sqlx::query("INSERT INTO tax_rates(name,basis_points) VALUES('Last A',0)").execute(&pool),sqlx::query("INSERT INTO tax_rates(name,basis_points) VALUES('Last B',0)").execute(&pool));assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM tax_rates").fetch_one(&pool).await.unwrap(),100);
        for n in 0..(99-sqlx::query_scalar::<_,i64>("SELECT count(*) FROM tax_classes").fetch_one(&pool).await.unwrap()) {sqlx::query("INSERT INTO tax_classes(name,rate_id) VALUES($1,$2)").bind(format!("Capacity class {n}")).bind(rate).execute(&pool).await.unwrap();}
        let(a,b)=tokio::join!(sqlx::query("INSERT INTO tax_classes(name,rate_id) VALUES('Last class A',$1)").bind(rate).execute(&pool),sqlx::query("INSERT INTO tax_classes(name,rate_id) VALUES('Last class B',$1)").bind(rate).execute(&pool));assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM tax_classes").fetch_one(&pool).await.unwrap(),100);
    }).await;
}
