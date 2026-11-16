use super::*;
use talocart_commerce::{
    countries::CheckoutCountries,
    pricing::Money,
    shipping::{Input, MethodCode, Shipping, ShippingError},
};
fn input(free: bool) -> Input {
    Input {
        name_it: if free {
            "Spedizione gratuita"
        } else {
            "Corriere espresso nazionale"
        }
        .into(),
        name_en: if free {
            "Free shipping"
        } else {
            "National express courier"
        }
        .into(),
        active: true,
        sort_order: "0".into(),
        fee_kind: "fixed".into(),
        fee_value: if free { "0" } else { "5.90" }.into(),
        free_from: if free { "" } else { "100" }.into(),
        delivery_days: "3".into(),
        currency: "EUR".into(),
        price_mode: "inclusive".into(),
        tax_class_id: if free { "" } else { "1" }.into(),
        countries: vec!["IT".into(), "FR".into()],
        ..Default::default()
    }
}
async fn enable(
    pool: &PgPool,
    identity: &Identity,
    s: &talocart_platform::identity::Session,
    codes: &[&str],
) {
    let c = CheckoutCountries::new(pool.clone(), identity.clone());
    c.save(
        s,
        &s.csrf,
        &codes.iter().map(|c| (*c).into()).collect::<Vec<_>>(),
        c.revision().await.unwrap(),
    )
    .await
    .unwrap();
}
pub(super) fn body(csrf: &str, rev: i64, free: bool) -> String {
    format!(
        "csrf={csrf}&revision={rev}&name_it=Spedizione&name_en=Shipping&instructions_it=%3Cscript%3E&instructions_en=Instructions&active=true&sort_order=0&fee_kind=fixed&fee_value={}&free_from={}&delivery_days=3&price_mode=inclusive&currency=EUR&countries=IT&countries=FR&tax_class_id={}",
        if free { "0" } else { "5.90" },
        if free { "0" } else { "100" },
        if free { "" } else { "1" }
    )
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn shipping_quotes_countries_removal_retention_and_restart() {
    isolated(|pool, options| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let s = taxonomy_session(&identity).await;
        let shipping = Shipping::new(pool.clone(), identity.clone());
        enable(&pool, &identity, &s, &["IT", "FR", "DE"]).await;
        for free in [false, true] {
            let id = if free { 2 } else { 1 };
            let rev = shipping.snapshot(&s).await.unwrap().revision;
            shipping
                .save(&s, &s.csrf, id, rev, &input(free))
                .await
                .unwrap();
            for destination in ["IT", "FR"] {
                for (base, cost) in [(9999, if free { 0 } else { 590 }), (10000, 0), (10001, 0)] {
                    let mut tx = pool.begin().await.unwrap();
                    let (m, q) = Shipping::require_method(
                        &mut tx,
                        id,
                        Money::from_cents(base).unwrap(),
                        destination,
                    )
                    .await
                    .unwrap();
                    assert_eq!(m.delivery_days, Some(3));
                    assert_eq!(q.cost.cents(), cost);
                    tx.commit().await.unwrap();
                }
            }
            for dest in ["DE", "US", ""] {
                let mut tx = pool.begin().await.unwrap();
                assert!(matches!(
                    Shipping::require_method(&mut tx, id, Money::from_cents(10000).unwrap(), dest)
                        .await,
                    Err(ShippingError::Unavailable)
                ));
            }
        }
        enable(&pool, &identity, &s, &["IT", "FR", "DE"]).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM shipping_method_countries")
                .fetch_one(&pool)
                .await
                .unwrap(),
            4
        );
        let rev = shipping.snapshot(&s).await.unwrap().revision;
        enable(&pool, &identity, &s, &["IT", "DE"]).await;
        assert!(shipping.snapshot(&s).await.unwrap().revision > rev);
        for method in shipping.snapshot(&s).await.unwrap().methods {
            assert_eq!(method.countries, ["IT"]);
        }
        let mut tx = pool.begin().await.unwrap();
        assert!(
            Shipping::require_method(&mut tx, 1, Money::from_cents(10000).unwrap(), "FR")
                .await
                .is_err()
        );
        tx.rollback().await.unwrap();
        enable(&pool, &identity, &s, &["IT", "FR"]).await;
        assert_eq!(
            shipping.snapshot(&s).await.unwrap().methods[0].countries,
            ["IT"]
        );
        let restarted = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let p = Shipping::new(restarted.clone(), Identity::new(restarted.clone()));
        assert_eq!(
            p.snapshot(&s).await.unwrap().methods[0].delivery_days,
            Some(3)
        );
        restarted.close().await;
        enable(&pool, &identity, &s, &[]).await;
        let methods = shipping.snapshot(&s).await.unwrap().methods;
        assert!(methods.iter().all(|m| m.countries.is_empty()));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn shipping_validation_currency_concurrency_and_rollback() {
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;zero_charge_class(&pool).await;let s=taxonomy_session(&identity).await;let p=Shipping::new(pool.clone(),identity.clone());enable(&pool,&identity,&s,&["IT","FR"]).await;
        let revision=p.snapshot(&s).await.unwrap().revision;let i=input(false);
        let (a,b)=tokio::join!(p.save(&s,&s.csrf,1,revision,&i),p.save(&s,&s.csrf,1,revision,&i));assert_ne!(a.is_ok(),b.is_ok());assert!(matches!(a,Err(ShippingError::Stale))||matches!(b,Err(ShippingError::Stale)));
        assert!(sqlx::query("UPDATE store_settings SET currency='USD'").execute(&pool).await.is_err());
        let rev=p.snapshot(&s).await.unwrap().revision;
        for countries in [vec![],vec!["IT".into(),"IT".into()],vec!["US".into()],vec!["it".into()]] {let mut i=input(false);i.countries=countries;assert!(matches!(p.save(&s,&s.csrf,1,rev,&i).await,Err(ShippingError::Invalid(_))));assert_eq!(p.snapshot(&s).await.unwrap().revision,rev);}
        for bad in ["-1","1.5","366","999999999999"] {let mut i=input(false);i.delivery_days=bad.into();assert!(matches!(p.save(&s,&s.csrf,1,rev,&i).await,Err(ShippingError::Invalid(f)) if f.contains(&"delivery_days")));}
        let mut i=input(false);i.currency="USD".into();assert!(matches!(p.save(&s,&s.csrf,1,rev,&i).await,Err(ShippingError::Stale)));
        i=input(false);i.free_from.clear();i.fee_kind="percent".into();i.fee_value="2.50".into();i.delivery_days.clear();p.save(&s,&s.csrf,1,rev,&i).await.unwrap();
        let mut tx=pool.begin().await.unwrap();let(m,q)=Shipping::require_method(&mut tx,1,Money::from_cents(20).unwrap(),"IT").await.unwrap();assert_eq!(q.cost.cents(),1);assert_eq!(m.delivery_days,None);tx.commit().await.unwrap();
        sqlx::query("UPDATE store_settings SET currency='USD'").execute(&pool).await.unwrap();
        let rev=p.snapshot(&s).await.unwrap().revision;assert!(matches!(p.save(&s,&s.csrf,1,rev,&i).await,Err(ShippingError::Stale)));
        i.currency="USD".into();i.active=false;p.save(&s,&s.csrf,1,rev,&i).await.unwrap();let mut tx=pool.begin().await.unwrap();assert!(matches!(Shipping::require_method(&mut tx,1,Money::from_cents(10000).unwrap(),"IT").await,Err(ShippingError::Unavailable)));tx.rollback().await.unwrap();
        let rev=p.snapshot(&s).await.unwrap().revision;assert!(matches!(p.save(&s,&s.csrf,99999,rev,&i).await,Err(ShippingError::NotFound)));assert_eq!(p.snapshot(&s).await.unwrap().revision,rev);
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn shipping_quotes_hold_country_and_method_until_transaction_ends() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let s = taxonomy_session(&identity).await;
        enable(&pool, &identity, &s, &["IT", "FR"]).await;
        let p = Shipping::new(pool.clone(), identity.clone());
        p.save(
            &s,
            &s.csrf,
            1,
            p.snapshot(&s).await.unwrap().revision,
            &input(false),
        )
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        Shipping::require_method(&mut tx, 1, Money::from_cents(9999).unwrap(), "IT")
            .await
            .unwrap();
        let c = CheckoutCountries::new(pool.clone(), identity.clone());
        let revision = c.revision().await.unwrap();
        let rev = p.snapshot(&s).await.unwrap().revision;
        let mut i = input(false);
        i.active = false;
        let mut save = Box::pin(p.save(&s, &s.csrf, 1, rev, &i));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut save)
                .await
                .is_err()
        );
        tx.commit().await.unwrap();
        save.await.unwrap();
        let mut i = input(false);
        i.active = true;
        p.save(&s, &s.csrf, 1, p.snapshot(&s).await.unwrap().revision, &i)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        Shipping::require_method(&mut tx, 1, Money::from_cents(0).unwrap(), "IT")
            .await
            .unwrap();
        let empty = vec![];
        let mut removal = Box::pin(c.save(&s, &s.csrf, &empty, revision));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut removal)
                .await
                .is_err()
        );
        tx.commit().await.unwrap();
        removal.await.unwrap();
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn shipping_http_bilingual_errors_csrf_permissions_and_forgery() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        enable(&pool, &identity, &s, &["IT", "FR"]).await;
        let p = Shipping::new(pool.clone(), identity.clone());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (lang, title) in [("it", "Metodi di spedizione"), ("en", "Shipping methods")] {
            identity.update_locale(&s, &s.csrf, lang).await.unwrap();
            let r = request(&app, "GET", "/admin/settings/shipping", &cookie, "", true).await;
            assert_eq!(r.headers()[header::CONTENT_LANGUAGE], lang);
            assert!(html(r).await.contains(title));
            for free in [false, true] {
                let path = format!("/admin/settings/shipping/{}", if free { 2 } else { 1 });
                let h = html(request(&app, "GET", &path, &cookie, "", true).await).await;
                assert!(h.contains("shipping-delivery_days"));
                assert!(h.contains("shipping-country-IT"));
                assert!(h.contains("shipping-country-FR"));
                assert!(!h.contains("shipping-country-US"));
                assert_eq!(h.contains("id=\"shipping-fee-kind\""), !free);
                let rev = p.snapshot(&s).await.unwrap().revision;
                let invalid =
                    body(&s.csrf, rev, free).replace("delivery_days=3", "delivery_days=-1");
                let r = request(&app, "POST", &path, &cookie, &invalid, true).await;
                assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let h = html(r).await;
                assert!(h.contains("&#60;script&#62;"));
                assert!(h.contains("value=\"-1\""));
                assert!(h.contains("shipping-delivery_days-error"));
                for extra in [
                    "&kind=other",
                    "&name_en=Duplicate",
                    "&countries=FR",
                    "&countries=US",
                ] {
                    assert_eq!(
                        request(
                            &app,
                            "POST",
                            &path,
                            &cookie,
                            &format!("{}{extra}", body(&s.csrf, rev, free)),
                            false
                        )
                        .await
                        .status(),
                        StatusCode::UNPROCESSABLE_ENTITY
                    );
                }
                if free {
                    assert_eq!(
                        request(
                            &app,
                            "POST",
                            &path,
                            &cookie,
                            &body(&s.csrf, rev, true).replace("fee_value=0", "fee_value=5"),
                            false
                        )
                        .await
                        .status(),
                        StatusCode::UNPROCESSABLE_ENTITY
                    );
                }
                assert_eq!(
                    request(
                        &app,
                        "POST",
                        &path,
                        &cookie,
                        &body(&s.csrf, rev, free),
                        false
                    )
                    .await
                    .status(),
                    StatusCode::SEE_OTHER
                );
                assert_eq!(
                    request(
                        &app,
                        "POST",
                        &path,
                        &cookie,
                        &body(&s.csrf, rev, free),
                        false
                    )
                    .await
                    .status(),
                    StatusCode::CONFLICT
                );
                assert_eq!(
                    request(
                        &app,
                        "POST",
                        &path,
                        &cookie,
                        &body("wrong", rev, free),
                        true
                    )
                    .await
                    .status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
        assert_eq!(
            request(&app, "GET", "/admin/settings/shipping", "", "", false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/settings/shipping/99999",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        use talocart_platform::identity::{Permission, Role};
        let role = identity
            .create_role(&s, &s.csrf, "Shipping reader", &[Permission::SettingsRead])
            .await
            .unwrap();
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "shipping-reader@example.test",
                "correct horse battery".into(),
                Role::Custom(role),
            )
            .await
            .unwrap();
        let raw = login_as(&identity, "shipping-reader@example.test").await;
        let rs = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let h = html(
            request(
                &app,
                "GET",
                "/admin/settings/shipping/1",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(h.contains(" disabled"));
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/shipping/1",
                &cookie,
                &body(&rs.csrf, p.snapshot(&s).await.unwrap().revision, false),
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            p.snapshot(&s).await.unwrap().methods[1].code,
            MethodCode::FreeShipping
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn shipping_currency_block_explains_shipping_in_both_languages() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        sqlx::query("UPDATE shipping_methods SET fee_value=590 WHERE code='national_express'")
            .execute(&pool)
            .await
            .unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (locale, phrase) in [
            ("it", "metodi di pagamento o di spedizione"),
            ("en", "payment or shipping methods"),
        ] {
            identity.update_locale(&s, &s.csrf, locale).await.unwrap();
            let revision: i64 =
                sqlx::query_scalar("SELECT revision FROM tax_configuration WHERE id=1")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let r = request(
                &app,
                "POST",
                "/admin/settings/taxes",
                &cookie,
                &format!(
                    "csrf={}&revision={revision}&currency=USD&mode=inclusive",
                    s.csrf
                ),
                true,
            )
            .await;
            let h = html(r).await;
            assert!(
                h.matches(phrase).count() >= 2,
                "Both help and error must explain shipping currency lock in {locale}"
            );
            assert_eq!(
                sqlx::query_scalar::<_, String>("SELECT currency FROM store_settings WHERE id=1")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                "EUR"
            );
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn shipping_edits_preserve_retained_country_rows() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let s = taxonomy_session(&identity).await;
        enable(&pool, &identity, &s, &["IT", "FR", "DE"]).await;
        let shipping = Shipping::new(pool.clone(), identity.clone());
        let mut i = input(false);
        shipping.save(&s, &s.csrf, 1, shipping.snapshot(&s).await.unwrap().revision, &i).await.unwrap();
        let retained: String = sqlx::query_scalar("SELECT xmin::text FROM shipping_method_countries WHERE method_id=1 AND country_code='IT'").fetch_one(&pool).await.unwrap();
        let revision = shipping.snapshot(&s).await.unwrap().revision;
        i.name_it = "Corriere aggiornato".into();
        shipping.save(&s, &s.csrf, 1, revision, &i).await.unwrap();
        assert_eq!(shipping.snapshot(&s).await.unwrap().revision, revision + 1);
        i.countries = vec!["IT".into(), "DE".into()];
        shipping.save(&s, &s.csrf, 1, revision + 1, &i).await.unwrap();
        let snapshot = shipping.snapshot(&s).await.unwrap();
        assert_eq!(snapshot.methods[0].countries, ["DE", "IT"]);
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT xmin::text FROM shipping_method_countries WHERE method_id=1 AND country_code='IT'").fetch_one(&pool).await.unwrap(), retained);
        assert!(matches!(shipping.save(&s, &s.csrf, 1, revision + 1, &i).await, Err(ShippingError::Stale)));
        enable(&pool, &identity, &s, &["IT", "FR"]).await;
        assert!(shipping.snapshot(&s).await.unwrap().revision > snapshot.revision);
        assert_eq!(shipping.snapshot(&s).await.unwrap().methods[0].countries, ["IT"]);
    }).await;
}
