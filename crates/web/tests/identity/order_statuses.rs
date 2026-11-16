use super::*;
use talocart_commerce::{
    order_statuses::{OrderStatuses, StatusError, StatusInput},
    payments::{CashOnDeliveryInput, PaymentError, Payments},
    pricing::Money,
};
fn input() -> StatusInput {
    StatusInput {
        name_it: "Pronto al ritiro".into(),
        name_en: "Ready for collection".into(),
        color: "#12ABef".into(),
        sort_order: "15".into(),
    }
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn statuses_crud_validation_duplicate_stale_and_reference_protection() {
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let s=taxonomy_session(&identity).await;
        let statuses=OrderStatuses::new(pool.clone(),identity.clone());let payments=Payments::new(pool.clone(),identity);
        let (rev,_)=statuses.editor(&s,None).await.unwrap();
        let id=statuses.save(&s,&s.csrf,None,rev,&input()).await.unwrap();
        let saved=statuses.editor(&s,Some(id)).await.unwrap().1.unwrap();assert_eq!(saved.color,"#12abef");assert_eq!(saved.name("it"),"Pronto al ritiro");assert_eq!(saved.name("en"),"Ready for collection");
        assert!(matches!(statuses.save(&s,&s.csrf,Some(id),rev,&input()).await,Err(StatusError::Stale)));
        let revision=statuses.editor(&s,None).await.unwrap().0;
        let mut duplicate=input();duplicate.name_it=" PRONTO AL RITIRO ".into();duplicate.name_en="Unique".into();
        assert!(matches!(statuses.save(&s,&s.csrf,None,revision,&duplicate).await,Err(StatusError::Duplicate)));
        assert_eq!(statuses.editor(&s,None).await.unwrap().0,revision);
        let mut invalid=input();invalid.color="#ffffff;display:none".into();
        assert!(matches!(statuses.save(&s,&s.csrf,Some(id),revision,&invalid).await,Err(StatusError::Invalid(_))));
        let snapshot=payments.snapshot(&s).await.unwrap();let cod=snapshot.methods.iter().find(|m|m.code==talocart_commerce::payments::MethodCode::CashOnDelivery).unwrap();let mut common=cod.common_input(snapshot.currency,snapshot.price_mode);common.initial_order_status_id=id.to_string();common.active=true;
        payments.save_cash_on_delivery(&s,&s.csrf,cod.id,snapshot.revision,&CashOnDeliveryInput{common:common.clone()}).await.unwrap();
        let mut tx=pool.begin().await.unwrap();let (method,_)=Payments::require_method(&mut tx,cod.id,Money::from_cents(100).unwrap(), "IT").await.unwrap();assert_eq!(method.initial_order_status_id,id);assert_eq!(OrderStatuses::require(&mut tx,id).await.unwrap().name_en,"Ready for collection");tx.commit().await.unwrap();
        let rev=payments.snapshot(&s).await.unwrap().revision;
        assert!(matches!(statuses.delete(&s,&s.csrf,id,rev).await,Err(StatusError::InUse)));
        common.initial_order_status_id="999999".into();assert!(matches!(payments.save_cash_on_delivery(&s,&s.csrf,cod.id,rev,&CashOnDeliveryInput{common:common.clone()}).await,Err(PaymentError::Invalid(fields)) if fields.contains(&"initial_order_status_id")));
        assert_eq!(payments.snapshot(&s).await.unwrap().revision,rev);
        common.initial_order_status_id="1".into();payments.save_cash_on_delivery(&s,&s.csrf,cod.id,rev,&CashOnDeliveryInput{common}).await.unwrap();
        statuses.delete(&s,&s.csrf,id,payments.snapshot(&s).await.unwrap().revision).await.unwrap();assert!(matches!(statuses.editor(&s,Some(id)).await,Err(StatusError::NotFound)));
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn statuses_and_payments_serialize_competing_configuration_edits() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let statuses = OrderStatuses::new(pool.clone(), identity.clone());
        let payments = Payments::new(pool.clone(), identity);
        let snap = payments.snapshot(&s).await.unwrap();
        let cod = snap
            .methods
            .iter()
            .find(|m| m.code == talocart_commerce::payments::MethodCode::CashOnDelivery)
            .unwrap();
        let payment = CashOnDeliveryInput {
            common: cod.common_input(snap.currency, snap.price_mode),
        };
        let status = input();
        let (a, b) = tokio::join!(
            statuses.save(&s, &s.csrf, None, snap.revision, &status),
            payments.save_cash_on_delivery(&s, &s.csrf, cod.id, snap.revision, &payment)
        );
        assert!(
            matches!(
                (&a, &b),
                (Ok(_), Err(PaymentError::Stale)) | (Err(StatusError::Stale), Ok(_))
            ),
            "Competing status/payment edits: {a:?}, {b:?}"
        );
        // Built-in statuses have protected role references. Both competing
        // operations must be valid independently for this revision-lock test.
        let revision = payments.snapshot(&s).await.unwrap().revision;
        let editable = statuses
            .save(
                &s,
                &s.csrf,
                None,
                revision,
                &StatusInput {
                    name_it: "Da modificare".into(),
                    name_en: "Editable".into(),
                    ..input()
                },
            )
            .await
            .unwrap();
        let revision = payments.snapshot(&s).await.unwrap().revision;
        let deletable = statuses
            .save(
                &s,
                &s.csrf,
                None,
                revision,
                &StatusInput {
                    name_it: "Da eliminare".into(),
                    name_en: "Deletable".into(),
                    ..input()
                },
            )
            .await
            .unwrap();
        let revision = payments.snapshot(&s).await.unwrap().revision;
        let mut changed = status.clone();
        changed.name_it = "Aggiornato".into();
        changed.name_en = "Updated".into();
        let (a, b) = tokio::join!(
            statuses.save(&s, &s.csrf, Some(editable), revision, &changed),
            statuses.delete(&s, &s.csrf, deletable, revision)
        );
        assert!(
            matches!(
                (&a, &b),
                (Ok(_), Err(StatusError::Stale)) | (Err(StatusError::Stale), Ok(_))
            ),
            "Competing status edits: {a:?}, {b:?}"
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn statuses_database_constraints_capacity_and_pagination() {
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let s=taxonomy_session(&identity).await;let statuses=OrderStatuses::new(pool.clone(),identity);
        for sql in ["UPDATE order_statuses SET color='red' WHERE id=1","UPDATE order_statuses SET sort_order=-1 WHERE id=1","UPDATE order_statuses SET name_en='' WHERE id=1","UPDATE order_statuses SET name_en='Paid' WHERE id=1","UPDATE payment_methods SET initial_order_status_id=999999","DELETE FROM order_statuses WHERE id=1"] {assert!(sqlx::query(sql).execute(&pool).await.is_err(),"{sql}");}
        sqlx::query("INSERT INTO order_statuses(name_it,name_en,color,sort_order) SELECT 'Stato '||i,'Status '||i,'#ffffff',i FROM generate_series(1,993) i").execute(&pool).await.unwrap();
        let page=statuses.list(&s,999).await.unwrap();assert_eq!(page.total,1000);assert_eq!(page.page,50);assert_eq!(page.entries.len(),20);
        assert!(matches!(statuses.list(&s,0).await,Err(StatusError::Invalid(_))));
        let revision=statuses.editor(&s,None).await.unwrap().0;assert!(matches!(statuses.save(&s,&s.csrf,None,revision,&input()).await,Err(StatusError::Capacity)));
        assert_eq!(statuses.editor(&s,None).await.unwrap().0,revision);
    }).await;
}
fn body(csrf: &str, revision: i64) -> String {
    format!(
        "csrf={csrf}&revision={revision}&name_it=Pronto&name_en=Ready&color=%23123456&sort_order=25"
    )
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn statuses_http_bilingual_errors_escaping_csrf_permissions_and_payment_selection() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let statuses = OrderStatuses::new(pool.clone(), identity.clone());
        let payments = Payments::new(pool.clone(), identity.clone());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = "/admin/settings/order-statuses";
        for (lang, title, waiting) in [
            ("it", "Stati d’ordine", "In attesa di pagamento"),
            ("en", "Order statuses", "Awaiting payment"),
        ] {
            identity.update_locale(&s, &s.csrf, lang).await.unwrap();
            let r = request(&app, "GET", path, &cookie, "", true).await;
            assert_eq!(r.status(), StatusCode::OK);
            assert_eq!(r.headers()[header::CONTENT_LANGUAGE], lang);
            let h = html(r).await;
            assert!(h.contains(title));
            assert!(h.contains(waiting));
            assert!(h.contains("ui-status-badge"));
            let revision = statuses.editor(&s, None).await.unwrap().0;
            let invalid = body(&s.csrf, revision)
                .replace("name_it=Pronto", "name_it=%3Cscript%3E")
                .replace("%23123456", "red");
            let r = request(
                &app,
                "POST",
                &format!("{path}/new"),
                &cookie,
                &invalid,
                true,
            )
            .await;
            assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let h = html(r).await;
            assert!(h.contains("&#60;script&#62;"));
            assert!(!h.contains("<script>"));
            assert!(h.contains("value=\"red\""));
            assert!(h.contains("status-color-error"));
            for extra in ["&color=%23000000", "&unknown=1"] {
                assert_eq!(
                    request(
                        &app,
                        "POST",
                        &format!("{path}/new"),
                        &cookie,
                        &format!("{}{extra}", body(&s.csrf, revision)),
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
                    &format!("{path}/new"),
                    &cookie,
                    &body("wrong", revision),
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
                    &format!("{path}/1/delete"),
                    &cookie,
                    &format!("csrf={}&revision={revision}&confirm=true", s.csrf),
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
                    &format!("{path}/2/delete"),
                    &cookie,
                    &format!("csrf={}&revision={revision}", s.csrf),
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
                    &format!("{path}/new"),
                    &cookie,
                    &body(&s.csrf, revision),
                    false
                )
                .await
                .status(),
                StatusCode::SEE_OTHER
            );
            let id =
                sqlx::query_scalar::<_, i64>("SELECT id FROM order_statuses WHERE name_en='Ready'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("{path}/{id}"),
                    &cookie,
                    &body(&s.csrf, revision),
                    true
                )
                .await
                .status(),
                StatusCode::CONFLICT
            );
            let r = request(&app, "GET", "/admin/settings/payments/1", &cookie, "", true).await;
            let h = html(r).await;
            assert!(h.contains("name=\"initial_order_status_id\""));
            assert!(h.contains(if lang == "it" { "Pronto" } else { "Ready" }));
            let revision = payments.snapshot(&s).await.unwrap().revision;
            let invalid = super::payments::body(&s.csrf, revision).replace(
                "initial_order_status_id=1",
                "initial_order_status_id=999999",
            );
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
            assert!(html(r).await.contains("payment-order-status-error"));
            statuses.delete(&s, &s.csrf, id, revision).await.unwrap();
        }
        use talocart_platform::identity::{Permission, Role};
        let role = identity
            .create_role(&s, &s.csrf, "Status reader", &[Permission::SettingsRead])
            .await
            .unwrap();
        identity
            .create_administrator(
                &s,
                &s.csrf,
                "status-reader@example.test",
                "correct horse battery".into(),
                Role::Custom(role),
            )
            .await
            .unwrap();
        let raw = login_as(&identity, "status-reader@example.test").await;
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let r = request(&app, "GET", &format!("{path}/1"), &cookie, "", false).await;
        assert_eq!(r.status(), StatusCode::OK);
        let h = html(r).await;
        assert!(h.contains(" disabled"));
        assert!(!h.contains("/1/delete"));
        assert_eq!(
            request(&app, "GET", &format!("{path}/new"), &cookie, "", false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        for suffix in ["/new", "/1", "/1/delete"] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("{path}{suffix}"),
                    &cookie,
                    &body(&reader.csrf, 0),
                    true
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert!(matches!(
            statuses
                .save(&reader, &reader.csrf, None, 0, &input())
                .await,
            Err(StatusError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert_eq!(
            request(&app, "GET", path, "", "", false).await.status(),
            StatusCode::SEE_OTHER
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn statuses_order_snapshot_blocks_edits_until_commit() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let statuses = OrderStatuses::new(pool.clone(), identity);
        let revision = statuses.editor(&s, Some(2)).await.unwrap().0;
        let mut order = pool.begin().await.unwrap();
        let current = OrderStatuses::require(&mut order, 2).await.unwrap();
        assert_eq!(current.name_en, "Paid");
        let mut update = tokio::spawn(async move {
            statuses
                .save(&s, &s.csrf, Some(2), revision, &input())
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(150), &mut update)
                .await
                .is_err()
        );
        order.commit().await.unwrap();
        update.await.unwrap().unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT name_en FROM order_statuses WHERE id=2")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "Ready for collection"
        );
    })
    .await;
}
