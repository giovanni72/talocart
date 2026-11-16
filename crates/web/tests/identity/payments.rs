use super::*;
use talocart_commerce::{
    payments::{
        BankDetailsInput, BankTransferInput, CommonInput, MethodCode, PaymentError, Payments,
    },
    pricing::Money,
};
fn input() -> BankTransferInput {
    BankTransferInput {
        common: CommonInput {
            name_it: "Alla consegna".into(),
            name_en: "On delivery".into(),
            active: true,
            sort_order: "1".into(),
            fee_kind: "fixed".into(),
            fee_value: "2.50".into(),
            minimum: "10".into(),
            currency: "EUR".into(),
            price_mode: "inclusive".into(),
            initial_order_status_id: "1".into(),
            tax_class_id: "1".into(),
            ..Default::default()
        },
        bank: BankDetailsInput {
            beneficiary: "Example".into(),
            iban: "IT60X0542811101000000123456".into(),
            bic: String::new(),
        },
    }
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn payments_persistence_thresholds_constraints_and_currency() {
    isolated(|pool, options| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let s = taxonomy_session(&identity).await;
        let p = Payments::new(pool.clone(), identity);
        let talocart_commerce::payments::Snapshot {
            revision: rev,
            methods,
            ..
        } = p.snapshot(&s).await.unwrap();
        assert_eq!(methods.len(), 2);
        assert!(!methods[0].active);
        let id = p
            .save_bank_transfer(&s, &s.csrf, 1, rev, &input())
            .await
            .unwrap();
        let restarted = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let p2 = Payments::new(restarted.clone(), Identity::new(restarted.clone()));
        assert_eq!(p2.snapshot(&s).await.unwrap().methods.len(), 2);
        restarted.close().await;
        let mut tx = pool.begin().await.unwrap();
        assert!(matches!(
            Payments::require_method(&mut tx, id, Money::from_cents(999).unwrap(), "IT").await,
            Err(PaymentError::Unavailable)
        ));
        let (_, quote) =
            Payments::require_method(&mut tx, id, Money::from_cents(1000).unwrap(), "IT")
                .await
                .unwrap();
        assert_eq!(quote.total.cents(), 1250);
        tx.commit().await.unwrap();
        for sql in [
            "UPDATE payment_methods SET minimum_cents=-1",
            "UPDATE payment_methods SET fee_kind='percent',fee_value=10001",
            "UPDATE payment_methods SET code='stripe'",
            "UPDATE store_settings SET currency='USD'",
        ] {
            assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
        }
        let rev = p.snapshot(&s).await.unwrap().revision;
        let mut i = input();
        i.common.minimum = String::new();
        i.common.fee_kind = "percent".into();
        i.common.fee_value = "2.5".into();
        p.save_bank_transfer(&s, &s.csrf, id, rev, &i)
            .await
            .unwrap();
        sqlx::query("UPDATE store_settings SET currency='USD'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            p.save_bank_transfer(&s, &s.csrf, id, p.snapshot(&s).await.unwrap().revision, &i)
                .await,
            Err(PaymentError::Stale)
        ));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn payments_concurrent_save_and_order_lock() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let s = taxonomy_session(&identity).await;
        let p = Payments::new(pool.clone(), identity);
        let revision = p.snapshot(&s).await.unwrap().revision;
        let i = input();
        let (a, b) = tokio::join!(
            p.save_bank_transfer(&s, &s.csrf, 1, revision, &i),
            p.save_bank_transfer(&s, &s.csrf, 1, revision, &i)
        );
        assert_ne!(a.is_ok(), b.is_ok());
        let id = match (a, b) {
            (Ok(id), Err(PaymentError::Stale)) | (Err(PaymentError::Stale), Ok(id)) => id,
            other => panic!("unexpected {other:?}"),
        };
        let rev = p.snapshot(&s).await.unwrap().revision;
        let mut order = pool.begin().await.unwrap();
        Payments::require_method(&mut order, id, Money::from_cents(1000).unwrap(), "IT")
            .await
            .unwrap();
        let writer = p.clone();
        let session = s.clone();
        let mut update = tokio::spawn(async move {
            let mut i = input();
            i.common.active = false;
            writer
                .save_bank_transfer(&session, &session.csrf, id, rev, &i)
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(150), &mut update)
                .await
                .is_err()
        );
        order.commit().await.unwrap();
        update.await.unwrap().unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(matches!(
            Payments::require_method(&mut tx, id, Money::from_cents(1000).unwrap(), "IT").await,
            Err(PaymentError::Unavailable)
        ));
    })
    .await;
}
pub(super) fn body(csrf: &str, rev: i64) -> String {
    format!(
        "csrf={csrf}&revision={rev}&name_it=Pagamento&name_en=Payment&instructions_it=%3Cscript%3E&instructions_en=Notes&active=true&sort_order=2&beneficiary=Example&iban=IT60X0542811101000000123456&bic=&fee_kind=percent&fee_value=2.5&minimum=10&price_mode=inclusive&currency=EUR&initial_order_status_id=1&tax_class_id=1"
    )
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn payments_http_bilingual_validation_permissions_csrf() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let p = Payments::new(pool.clone(), identity.clone());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (lang, title) in [("it", "Metodi di pagamento"), ("en", "Payment methods")] {
            identity.update_locale(&s, &s.csrf, lang).await.unwrap();
            let r = request(&app, "GET", "/admin/settings/payments", &cookie, "", true).await;
            assert_eq!(r.status(), StatusCode::OK);
            assert_eq!(r.headers()[header::CONTENT_LANGUAGE], lang);
            assert!(html(r).await.contains(title));
            let rev = p.snapshot(&s).await.unwrap().revision;
            for iban in ["ZZ8112345678901", "IT5812345678901"] {
                let invalid = body(&s.csrf, rev).replace("IT60X0542811101000000123456", iban);
                let r = request(
                    &app,
                    "POST",
                    "/admin/settings/payments/1",
                    &cookie,
                    &invalid,
                    false,
                )
                .await;
                assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let h = html(r).await;
                assert!(h.contains("aria-describedby=\"payment-iban-error\""));
                assert!(h.contains(iban));
                assert_eq!(p.snapshot(&s).await.unwrap().revision, rev);
            }
            let invalid = body(&s.csrf, rev).replace("minimum=10", "minimum=-1");
            let r = request(
                &app,
                "POST",
                "/admin/settings/payments/1",
                &cookie,
                &invalid,
                true,
            )
            .await;
            assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let h = html(r).await;
            assert!(h.contains("&#60;script&#62;"));
            assert!(!h.contains("<script>"));
            assert!(h.contains("aria-invalid=\"true\""));
            assert!(h.contains("value=\"-1\""));
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/admin/settings/payments/1",
                    &cookie,
                    &body(&s.csrf, rev),
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
                    "/admin/settings/payments/1",
                    &cookie,
                    &body(&s.csrf, rev),
                    false
                )
                .await
                .status(),
                StatusCode::CONFLICT
            );
        }
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/payments/1",
                &cookie,
                &body("wrong", 0),
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
                "/admin/settings/payments/999999",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(&app, "GET", "/admin/settings/payments", "", "", false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        let h = html(
            request(
                &app,
                "GET",
                "/admin/settings/payments?page=9999",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(!h.contains("rel=\"prev\""));
        assert!(!h.contains("rel=\"next\""));
        use talocart_platform::identity::{Permission, Role};
        let role = identity
            .create_role(&s, &s.csrf, "Payments reader", &[Permission::SettingsRead])
            .await
            .unwrap();
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "reader@example.test",
                "correct horse battery".into(),
                Role::Custom(role),
            )
            .await
            .unwrap();
        let raw = login_as(&identity, "reader@example.test").await;
        let rs = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/settings/payments/1",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/payments/1",
                &cookie,
                &body(&rs.csrf, p.snapshot(&s).await.unwrap().revision),
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn payments_only_compiled_methods_are_editable_and_creation_is_removed() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let p = Payments::new(pool.clone(), identity);
        let missing_id = 999999;
        let talocart_commerce::payments::Snapshot {
            revision: rev,
            methods,
            ..
        } = p.snapshot(&session).await.unwrap();
        assert_eq!(methods.len(), 2);
        assert_eq!(methods[0].id, 1);
        for path in ["/admin/settings/payments/new", "/admin/settings/payments"] {
            assert!(
                !request(
                    &app,
                    "POST",
                    path,
                    &cookie,
                    &body(&session.csrf, rev),
                    false
                )
                .await
                .status()
                .is_success()
            );
        }
        assert_eq!(
            request(
                &app,
                "GET",
                "/admin/settings/payments/new",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        for lang in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(lang)
                .execute(&pool)
                .await
                .unwrap();
            let h =
                html(request(&app, "GET", "/admin/settings/payments", &cookie, "", false).await)
                    .await;
            assert!(!h.contains("/payments/new"));
            let h = html(
                request(
                    &app,
                    "GET",
                    "/admin/settings/payments/1",
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(!h.contains("name=\"kind\""));
            assert!(!h.contains("payment-kind"));
            assert!(h.contains("payment-iban"));
        }
        let forged = format!("{}&kind=manual", body(&session.csrf, rev));
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/payments/1",
                &cookie,
                &forged,
                false
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(matches!(
            p.save_bank_transfer(&session, &session.csrf, missing_id, rev, &input())
                .await,
            Err(PaymentError::NotFound)
        ));
        let mut tx = pool.begin().await.unwrap();
        assert!(matches!(
            Payments::require_method(&mut tx, missing_id, Money::from_cents(1000).unwrap(), "IT")
                .await,
            Err(PaymentError::NotFound)
        ));
        tx.rollback().await.unwrap();
        assert_eq!(p.snapshot(&session).await.unwrap().revision, rev);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_methods")
                .fetch_one(&pool)
                .await
                .unwrap(),
            3
        );
        sqlx::query("UPDATE store_settings SET currency='USD'")
            .execute(&pool)
            .await
            .unwrap();
    })
    .await;
}

fn cod_input() -> talocart_commerce::payments::CashOnDeliveryInput {
    talocart_commerce::payments::CashOnDeliveryInput {
        common: CommonInput {
            name_it: "Contrassegno".into(),
            name_en: "Cash on delivery".into(),
            active: true,
            sort_order: "10".into(),
            fee_kind: "fixed".into(),
            fee_value: "3.50".into(),
            minimum: "10".into(),
            currency: "EUR".into(),
            price_mode: "inclusive".into(),
            initial_order_status_id: "1".into(),
            tax_class_id: "1".into(),
            ..Default::default()
        },
    }
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn payments_cash_on_delivery_defaults_quote_isolation_and_concurrency() {
    isolated(|pool, options| async move {
        let identity = installed(&pool).await;
        zero_charge_class(&pool).await;
        let s = taxonomy_session(&identity).await;
        let p = Payments::new(pool.clone(), identity);
        let talocart_commerce::payments::Snapshot {
            revision: rev,
            methods,
            ..
        } = p.snapshot(&s).await.unwrap();
        let cod = methods
            .iter()
            .find(|m| m.code == MethodCode::CashOnDelivery)
            .unwrap();
        let id = cod.id;
        assert!(!cod.active);
        assert_eq!(cod.minimum_cents, 0);
        assert_eq!(cod.fee_value, 0);
        assert!(cod.iban.is_empty());
        assert_eq!(cod.instructions_it, "Paga alla consegna.");
        let bank_before: String =
            sqlx::query_scalar("SELECT to_jsonb(m)::text FROM payment_methods m WHERE id=1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let input = cod_input();
        p.save_cash_on_delivery(&s, &s.csrf, id, rev, &input)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(matches!(
            Payments::require_method(&mut tx, id, Money::from_cents(999).unwrap(), "IT").await,
            Err(PaymentError::Unavailable)
        ));
        let (m, q) = Payments::require_method(&mut tx, id, Money::from_cents(1000).unwrap(), "IT")
            .await
            .unwrap();
        assert_eq!(q.fee.cents(), 350);
        assert_eq!(q.total.cents(), 1350);
        assert!(m.beneficiary.is_empty());
        tx.commit().await.unwrap();
        let revision = p.snapshot(&s).await.unwrap().revision;
        let mut percent = input.clone();
        percent.common.fee_kind = "percent".into();
        percent.common.fee_value = "2.5".into();
        percent.common.minimum = String::new();
        let (a, b) = tokio::join!(
            p.save_cash_on_delivery(&s, &s.csrf, id, revision, &percent),
            p.save_cash_on_delivery(&s, &s.csrf, id, revision, &percent)
        );
        assert_ne!(a.is_ok(), b.is_ok());
        assert!(matches!(a, Err(PaymentError::Stale)) || matches!(b, Err(PaymentError::Stale)));
        let mut tx = pool.begin().await.unwrap();
        let (_, q) = Payments::require_method(&mut tx, id, Money::from_cents(20).unwrap(), "IT")
            .await
            .unwrap();
        assert_eq!(q.fee.cents(), 1);
        tx.commit().await.unwrap();
        let revision = p.snapshot(&s).await.unwrap().revision;
        assert!(matches!(
            p.save_cash_on_delivery(&s, &s.csrf, 1, revision, &percent)
                .await,
            Err(PaymentError::NotFound)
        ));
        assert!(matches!(
            p.save_bank_transfer(&s, &s.csrf, id, revision, &super::payments::input())
                .await,
            Err(PaymentError::NotFound)
        ));
        percent.common.minimum = "-1".into();
        assert!(matches!(
            p.save_cash_on_delivery(&s, &s.csrf, id, revision, &percent)
                .await,
            Err(PaymentError::Invalid(_))
        ));
        assert_eq!(
            bank_before,
            sqlx::query_scalar::<_, String>(
                "SELECT to_jsonb(m)::text FROM payment_methods m WHERE id=1"
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        );
        let restarted = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        let p2 = Payments::new(restarted.clone(), Identity::new(restarted.clone()));
        assert_eq!(
            p2.snapshot(&s)
                .await
                .unwrap()
                .methods
                .iter()
                .find(|m| m.code == MethodCode::CashOnDelivery)
                .unwrap()
                .fee_value,
            250
        );
        restarted.close().await;
        let mut disabled = cod_input();
        disabled.common.active = false;
        let revision = p.snapshot(&s).await.unwrap().revision;
        p.save_cash_on_delivery(&s, &s.csrf, id, revision, &disabled)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(matches!(
            Payments::require_method(&mut tx, id, Money::from_cents(1000).unwrap(), "IT").await,
            Err(PaymentError::Unavailable)
        ));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn payments_cash_on_delivery_http_bilingual_fields_errors_csrf_and_reader() {
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;zero_charge_class(&pool).await;let raw=authenticated(&identity).await;let s=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let p=Payments::new(pool.clone(),identity.clone());let id=p.snapshot(&s).await.unwrap().methods.into_iter().find(|m|m.code==MethodCode::CashOnDelivery).unwrap().id;
        let path=format!("/admin/settings/payments/{id}");let app=router(test_app(pool.clone(),"Shop".into()).await);
        let payload=|csrf:&str,rev:i64|format!("csrf={csrf}&revision={rev}&name_it=Contrassegno&name_en=Cash+on+delivery&instructions_it=%3Cscript%3E&instructions_en=Pay+on+delivery&active=true&sort_order=10&fee_kind=percent&fee_value=2.5&minimum=10&price_mode=inclusive&currency=EUR&initial_order_status_id=1&tax_class_id=1");
        for (lang,label) in [("it","Contrassegno"),("en","Cash on delivery")] {
            identity.update_locale(&s,&s.csrf,lang).await.unwrap();let r=request(&app,"GET",&path,&cookie,"",true).await;assert_eq!(r.status(),StatusCode::OK);let h=html(r).await;assert!(h.contains(label));
            for field in ["beneficiary","iban","bic","kind"] {assert!(!h.contains(&format!("name=\"{field}\"")));}
            let rev=p.snapshot(&s).await.unwrap().revision;
            let invalid=payload(&s.csrf,rev).replace("minimum=10","minimum=-1");let r=request(&app,"POST",&path,&cookie,&invalid,true).await;assert_eq!(r.status(),StatusCode::UNPROCESSABLE_ENTITY);let h=html(r).await;assert!(h.contains("&#60;script&#62;"));assert!(h.contains("value=\"-1\""));assert!(h.contains("aria-invalid=\"true\""));
            let injected=format!("{}&iban=INVALID",payload(&s.csrf,rev));assert_eq!(request(&app,"POST",&path,&cookie,&injected,false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(request(&app,"POST",&path,&cookie,&payload(&s.csrf,rev),false).await.status(),StatusCode::SEE_OTHER);
            assert_eq!(request(&app,"POST",&path,&cookie,&payload(&s.csrf,rev),false).await.status(),StatusCode::CONFLICT);
        }
        assert_eq!(request(&app,"POST",&path,&cookie,&payload("wrong",0),true).await.status(),StatusCode::FORBIDDEN);
        use talocart_platform::identity::{Permission,Role};let role=identity.create_role(&s,&s.csrf,"COD reader",&[Permission::SettingsRead]).await.unwrap();identity.create_administrator(&s,&s.csrf,"cod-reader@example.test","correct horse battery".into(),Role::Custom(role)).await.unwrap();
        let raw=login_as(&identity,"cod-reader@example.test").await;let rs=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let r=request(&app,"GET",&path,&cookie,"",false).await;assert_eq!(r.status(),StatusCode::OK);assert!(html(r).await.contains(" disabled"));
        assert_eq!(request(&app,"POST",&path,&cookie,&payload(&rs.csrf,p.snapshot(&s).await.unwrap().revision),true).await.status(),StatusCode::FORBIDDEN);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn payments_identity_is_unique_immutable_and_enforces_bank_fields() {
    isolated(|pool, _| async move {
        installed(&pool).await;
        for sql in [
            "INSERT INTO payment_methods(code,name_it,name_en) VALUES('manual','Manual','Manual')",
            "INSERT INTO payment_methods(code,name_it,name_en) VALUES('bank_transfer','Duplicate','Duplicate')",
            "UPDATE payment_methods SET code='manual',active=true WHERE code='bank_transfer'",
            "UPDATE payment_methods SET active=true WHERE code='bank_transfer'",
            "UPDATE payment_methods SET iban='INVALID' WHERE code='cash_on_delivery'",
        ] { assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}"); }
        // Remove the other row inside a rolled-back transaction to prove immutability,
        // independently of the unique constraint.
        let mut tx=pool.begin().await.unwrap();
        sqlx::query("DELETE FROM payment_methods WHERE code='cash_on_delivery'").execute(&mut *tx).await.unwrap();
        assert!(sqlx::query("UPDATE payment_methods SET code='cash_on_delivery' WHERE code='bank_transfer'").execute(&mut *tx).await.is_err());
        tx.rollback().await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM payment_methods").fetch_one(&pool).await.unwrap(),3);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn payments_base_hides_and_rejects_optional_online_method_without_changing_legacy_data() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let online_id: i64 = sqlx::query_scalar(
            "UPDATE payment_methods SET active=true WHERE code='online_checkout' RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let service = Payments::new(pool.clone(), identity);
        let snapshot = service.snapshot(&session).await.unwrap();
        assert_eq!(snapshot.methods.len(), 2);
        assert!(snapshot.methods.iter().all(|method| matches!(
            method.code,
            MethodCode::BankTransfer | MethodCode::CashOnDelivery
        )));
        assert!(matches!(
            service
                .save_online(
                    &session,
                    &session.csrf,
                    online_id,
                    snapshot.revision,
                    &CommonInput::default(),
                    true
                )
                .await,
            Err(PaymentError::NotFound)
        ));
        let app = router(test_app(pool.clone(), "Base".into()).await);
        let cookie = format!("talocart_session={raw}");
        for locale in ["en", "it"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let listing =
                html(request(&app, "GET", "/admin/settings/payments", &cookie, "", false).await)
                    .await;
            assert!(!listing.contains("Stripe"));
            assert_eq!(
                request(
                    &app,
                    "GET",
                    &format!("/admin/settings/payments/{online_id}"),
                    &cookie,
                    "",
                    false
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("/admin/settings/payments/{online_id}"),
                    &cookie,
                    &format!(
                        "csrf={}&revision={}&active=true",
                        session.csrf, snapshot.revision
                    ),
                    false
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            request(&app, "GET", "/assets/checkout-stripe.js", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(&app, "POST", "/payments/stripe/webhook", "", "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT active FROM payment_methods WHERE id=$1")
                .bind(online_id)
                .fetch_one(&pool)
                .await
                .unwrap()
        );
        assert_eq!(
            service.snapshot(&session).await.unwrap().revision,
            snapshot.revision
        );
    })
    .await;
}
