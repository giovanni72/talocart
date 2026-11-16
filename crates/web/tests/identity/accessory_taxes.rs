use super::*;
use talocart_commerce::{
    countries::CheckoutCountries,
    payments::{BankDetailsInput, BankTransferInput, CashOnDeliveryInput, PaymentError, Payments},
    pricing::{Money, PriceError},
    shipping::{Shipping, ShippingError},
};

async fn fixture(pool: &PgPool) -> (Identity, talocart_platform::identity::Session) {
    let identity = installed(pool).await;
    zero_charge_class(pool).await;
    sqlx::query("UPDATE tax_rates SET basis_points=2200 WHERE id=1")
        .execute(pool)
        .await
        .unwrap();
    let s = taxonomy_session(&identity).await;
    let countries = CheckoutCountries::new(pool.clone(), identity.clone());
    countries
        .save(
            &s,
            &s.csrf,
            &["IT".into(), "FR".into()],
            countries.revision().await.unwrap(),
        )
        .await
        .unwrap();
    (identity, s)
}
async fn configure(
    pool: &PgPool,
    identity: &Identity,
    s: &talocart_platform::identity::Session,
    percent: bool,
) {
    let shipping = Shipping::new(pool.clone(), identity.clone());
    let snap = shipping.snapshot(s).await.unwrap();
    let mut input = snap
        .methods
        .iter()
        .find(|m| m.id == 1)
        .unwrap()
        .input(snap.currency, snap.price_mode);
    input.active = true;
    input.countries = vec!["IT".into(), "FR".into()];
    input.tax_class_id = "1".into();
    input.fee_value = "10".into();
    input.fee_kind = if percent { "percent" } else { "fixed" }.into();
    shipping
        .save(s, &s.csrf, 1, snap.revision, &input)
        .await
        .unwrap();
    let payments = Payments::new(pool.clone(), identity.clone());
    for id in [1, 2] {
        let snap = payments.snapshot(s).await.unwrap();
        let mut common = snap
            .methods
            .iter()
            .find(|m| m.id == id)
            .unwrap()
            .common_input(snap.currency, snap.price_mode);
        common.active = true;
        common.tax_class_id = "1".into();
        common.fee_kind = if percent { "percent" } else { "fixed" }.into();
        common.fee_value = "10".into();
        if id == 1 {
            payments
                .save_bank_transfer(
                    s,
                    &s.csrf,
                    id,
                    snap.revision,
                    &BankTransferInput {
                        common,
                        bank: BankDetailsInput {
                            beneficiary: "Synthetic beneficiary".into(),
                            iban: "IT60X0542811101000000123456".into(),
                            bic: String::new(),
                        },
                    },
                )
                .await
                .unwrap();
        } else {
            payments
                .save_cash_on_delivery(
                    s,
                    &s.csrf,
                    id,
                    snap.revision,
                    &CashOnDeliveryInput { common },
                )
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn accessory_taxes_fixed_quotes_resolve_destination_mode_and_snapshot() {
    isolated(|pool, _| async move {
        let (identity, s) = fixture(&pool).await;
        sqlx::query("INSERT INTO tax_rates(name,basis_points) VALUES('Destination ten',1000)")
            .execute(&pool)
            .await
            .unwrap();
        let mut setup = pool.begin().await.unwrap();
        sqlx::query("INSERT INTO tax_zones(name) VALUES('France rule')")
            .execute(&mut *setup)
            .await
            .unwrap();
        sqlx::query("INSERT INTO tax_zone_countries(country_code,zone_id) VALUES('FR',1)")
            .execute(&mut *setup)
            .await
            .unwrap();
        sqlx::query("INSERT INTO tax_zone_rules(zone_id,class_id,rate_id) VALUES(1,1,2)")
            .execute(&mut *setup)
            .await
            .unwrap();
        setup.commit().await.unwrap();
        for (mode, country, net, tax, gross, rate) in [
            ("inclusive", "IT", 820, 180, 1000, 2200),
            ("exclusive", "IT", 1000, 220, 1220, 2200),
            ("inclusive", "FR", 909, 91, 1000, 1000),
            ("exclusive", "FR", 1000, 100, 1100, 1000),
        ] {
            sqlx::query("UPDATE pricing_settings SET price_mode=$1,country_code='IT'")
                .bind(mode)
                .execute(&pool)
                .await
                .unwrap();
            configure(&pool, &identity, &s, false).await;
            let mut tx = pool.begin().await.unwrap();
            let (_, q) =
                Shipping::require_method(&mut tx, 1, Money::from_cents(10000).unwrap(), country)
                    .await
                    .unwrap();
            assert_eq!(
                (
                    q.breakdown.totals.net().cents(),
                    q.breakdown.totals.tax().cents(),
                    q.cost.cents(),
                    q.total.cents()
                ),
                (net, tax, gross, 10000 + gross)
            );
            assert_eq!(q.breakdown.tax.as_ref().unwrap().basis_points, rate);
            assert_eq!(q.breakdown.destination, country);
            assert_eq!(q.breakdown.currency, "EUR");
            assert_eq!(q.breakdown.price_mode.key(), mode);
            assert_eq!(
                q.breakdown.tax.as_ref().unwrap().zone_name.as_deref(),
                if country == "FR" {
                    Some("France rule")
                } else {
                    None
                }
            );
            for id in [1, 2] {
                let (_, p) = Payments::require_method(&mut tx, id, q.total, country)
                    .await
                    .unwrap();
                assert_eq!(
                    (
                        p.breakdown.totals.net().cents(),
                        p.breakdown.totals.tax().cents(),
                        p.fee.cents(),
                        p.total.cents()
                    ),
                    (net, tax, gross, 10000 + gross * 2)
                );
            }
            tx.commit().await.unwrap();
        }
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn accessory_taxes_percentage_bases_thresholds_zero_and_overflow() {
    isolated(|pool, _| async move {
        let (identity, s) = fixture(&pool).await;
        sqlx::query("UPDATE pricing_settings SET price_mode='exclusive'")
            .execute(&pool)
            .await
            .unwrap();
        configure(&pool, &identity, &s, true).await;
        let mut tx = pool.begin().await.unwrap();
        let (_, shipping) =
            Shipping::require_method(&mut tx, 1, Money::from_cents(10000).unwrap(), "IT")
                .await
                .unwrap();
        assert_eq!(
            (
                shipping.breakdown.entered.cents(),
                shipping.cost.cents(),
                shipping.total.cents()
            ),
            (1000, 1220, 11220)
        );
        let (_, payment) = Payments::require_method(&mut tx, 2, shipping.total, "IT")
            .await
            .unwrap();
        assert_eq!(
            (
                payment.breakdown.entered.cents(),
                payment.breakdown.totals.tax().cents(),
                payment.fee.cents(),
                payment.total.cents()
            ),
            (1122, 247, 1369, 12589)
        );
        tx.commit().await.unwrap();
        sqlx::query("UPDATE shipping_methods SET free_from_cents=10000 WHERE id=1")
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let (_, free) =
            Shipping::require_method(&mut tx, 1, Money::from_cents(10000).unwrap(), "IT")
                .await
                .unwrap();
        assert!(free.free);
        assert_eq!(
            (free.cost.cents(), free.breakdown.totals.tax().cents()),
            (0, 0)
        );
        assert!(matches!(
            Payments::require_method(&mut tx, 2, Money::from_cents(i64::MAX).unwrap(), "IT").await,
            Err(PaymentError::Price(PriceError::Overflow))
        ));
        tx.rollback().await.unwrap();
        sqlx::query("UPDATE tax_rates SET basis_points=0 WHERE id=1")
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let (_, q) = Payments::require_method(&mut tx, 2, Money::from_cents(20).unwrap(), "IT")
            .await
            .unwrap();
        assert_eq!((q.fee.cents(), q.breakdown.totals.tax().cents()), (2, 0));
        assert_eq!(q.breakdown.tax.unwrap().basis_points, 0);
        tx.commit().await.unwrap();
        sqlx::query(
            "UPDATE payment_methods SET fee_kind='none',fee_value=0,tax_class_id=NULL WHERE id=2",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let (_, q) = Payments::require_method(&mut tx, 2, Money::from_cents(20).unwrap(), "IT")
            .await
            .unwrap();
        assert!(q.breakdown.tax.is_none());
        assert_eq!(q.fee.cents(), 0);
        tx.commit().await.unwrap();
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn accessory_taxes_validation_fk_inactive_class_and_stale_mode() {
    isolated(|pool,_|async move {
        let(identity,s)=fixture(&pool).await;configure(&pool,&identity,&s,false).await;
        let shipping=Shipping::new(pool.clone(),identity.clone());let payments=Payments::new(pool.clone(),identity.clone());
        let snap=shipping.snapshot(&s).await.unwrap();let original=snap.methods[0].input(snap.currency,snap.price_mode);
        let pay=payments.snapshot(&s).await.unwrap();let common=pay.methods.iter().find(|m|m.id==2).unwrap().common_input(pay.currency,pay.price_mode);
        for bad in ["","0","-1","999999","bad"] {
            let mut i=original.clone();i.tax_class_id=bad.into();
            assert!(matches!(shipping.save(&s,&s.csrf,1,snap.revision,&i).await,Err(ShippingError::Invalid(f)) if f.contains(&"tax_class_id")));
            let mut c=common.clone();c.tax_class_id=bad.into();
            assert!(matches!(payments.save_cash_on_delivery(&s,&s.csrf,2,pay.revision,&CashOnDeliveryInput{common:c}).await,Err(PaymentError::Invalid(f)) if f.contains(&"tax_class_id")));
        }
        assert_eq!(shipping.snapshot(&s).await.unwrap().revision,snap.revision);assert_eq!(payments.snapshot(&s).await.unwrap().revision,pay.revision);
        assert!(sqlx::query("DELETE FROM tax_classes WHERE id=1").execute(&pool).await.is_err());
        assert!(sqlx::query("UPDATE payment_methods SET tax_class_id=NULL WHERE id=2").execute(&pool).await.is_err());
        assert!(sqlx::query("UPDATE shipping_methods SET tax_class_id=999999 WHERE id=1").execute(&pool).await.is_err());
        assert!(sqlx::query("UPDATE shipping_methods SET tax_class_id=1 WHERE id=2").execute(&pool).await.is_err());
        sqlx::query("UPDATE tax_classes SET active=false WHERE id=1").execute(&pool).await.unwrap();
        let mut tx=pool.begin().await.unwrap();assert!(matches!(Payments::require_method(&mut tx,2,Money::from_cents(100).unwrap(),"IT").await,Err(PaymentError::Unavailable)));tx.rollback().await.unwrap();
        assert!(matches!(shipping.save(&s,&s.csrf,1,snap.revision,&original).await,Err(ShippingError::Invalid(f)) if f.contains(&"tax_class_id")));
        sqlx::query("UPDATE tax_classes SET active=true WHERE id=1").execute(&pool).await.unwrap();
        sqlx::query("UPDATE pricing_settings SET price_mode='exclusive'").execute(&pool).await.unwrap();
        assert!(matches!(shipping.save(&s,&s.csrf,1,snap.revision,&original).await,Err(ShippingError::Stale)));
        assert!(matches!(payments.save_cash_on_delivery(&s,&s.csrf,2,pay.revision,&CashOnDeliveryInput{common}).await,Err(PaymentError::Stale)));
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn accessory_taxes_quote_holds_rates_mode_and_zone_until_commit() {
    isolated(|pool, _| async move {
        let (identity, s) = fixture(&pool).await;
        configure(&pool, &identity, &s, false).await;
        for sql in [
            "UPDATE tax_rates SET basis_points=1000 WHERE id=1",
            "UPDATE pricing_settings SET price_mode='exclusive'",
            "UPDATE tax_classes SET active=false WHERE id=1",
        ] {
            let mut tx = pool.begin().await.unwrap();
            let (_, q) =
                Shipping::require_method(&mut tx, 1, Money::from_cents(10000).unwrap(), "IT")
                    .await
                    .unwrap();
            let other = pool.clone();
            let mut writer = tokio::spawn(async move { sqlx::query(sql).execute(&other).await });
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), &mut writer)
                    .await
                    .is_err()
            );
            tx.commit().await.unwrap();
            writer.await.unwrap().unwrap();
            assert_eq!(q.breakdown.tax.as_ref().unwrap().basis_points, 2200);
            sqlx::query("UPDATE tax_rates SET basis_points=2200 WHERE id=1")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE pricing_settings SET price_mode='inclusive'")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE tax_classes SET active=true WHERE id=1")
                .execute(&pool)
                .await
                .unwrap();
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn accessory_taxes_http_bilingual_missing_unknown_and_duplicate_class() {
    isolated(|pool, _| async move {
        let (identity, _) = fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        sqlx::query("UPDATE tax_classes SET name='<script>Tax</script>' WHERE id=1")
            .execute(&pool)
            .await
            .unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for lang in ["it", "en"] {
            identity.update_locale(&s, &s.csrf, lang).await.unwrap();
            for payment in [true, false] {
                let (path, revision, payload, prefix) = if payment {
                    let revision = Payments::new(pool.clone(), identity.clone())
                        .snapshot(&s)
                        .await
                        .unwrap()
                        .revision;
                    (
                        "/admin/settings/payments/1",
                        revision,
                        payments::body(&s.csrf, revision),
                        "payment",
                    )
                } else {
                    let revision = Shipping::new(pool.clone(), identity.clone())
                        .snapshot(&s)
                        .await
                        .unwrap()
                        .revision;
                    (
                        "/admin/settings/shipping/1",
                        revision,
                        shipping::body(&s.csrf, revision, false),
                        "shipping",
                    )
                };
                for value in ["", "999999", "bad"] {
                    let invalid =
                        payload.replace("tax_class_id=1", &format!("tax_class_id={value}"));
                    let response = request(&app, "POST", path, &cookie, &invalid, true).await;
                    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
                    assert_eq!(response.headers()[header::CONTENT_LANGUAGE], lang);
                    let page = html(response).await;
                    assert!(page.contains(&format!("id=\"{prefix}-tax-error\"")));
                    assert!(page.contains("&#60;script&#62;Tax&#60;/script&#62;"));
                    assert!(!page.contains("<script>Tax</script>"));
                    if !value.is_empty() {
                        assert!(page.contains(&format!("value=\"{value}\" selected")));
                    }
                    assert!(page.contains(if lang == "it" {
                        "Classe fiscale del costo"
                    } else {
                        "Cost or fee tax class"
                    }));
                }
                assert_eq!(
                    request(
                        &app,
                        "POST",
                        path,
                        &cookie,
                        &format!("{payload}&tax_class_id=1"),
                        false
                    )
                    .await
                    .status(),
                    StatusCode::UNPROCESSABLE_ENTITY
                );
                assert_eq!(
                    request(
                        &app,
                        "POST",
                        path,
                        &cookie,
                        &payload.replace(&s.csrf, "wrong"),
                        false
                    )
                    .await
                    .status(),
                    StatusCode::FORBIDDEN
                );
                let current = if payment {
                    Payments::new(pool.clone(), identity.clone())
                        .snapshot(&s)
                        .await
                        .unwrap()
                        .revision
                } else {
                    Shipping::new(pool.clone(), identity.clone())
                        .snapshot(&s)
                        .await
                        .unwrap()
                        .revision
                };
                assert_eq!(current, revision);
            }
        }
    })
    .await;
}
