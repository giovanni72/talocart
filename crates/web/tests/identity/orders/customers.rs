use super::*;
use talocart_commerce::customer_addresses::{AddressInput, CustomerAddresses};
use talocart_platform::identity::{
    CustomerError, CustomerProfile, CustomerSession, Customers, IdentityError,
};
const PASSWORD: &str = "synthetic customer password";
fn profile(email: &str) -> CustomerProfile {
    CustomerProfile {
        email: email.into(),
        first_name: "Alex".into(),
        last_name: "Example".into(),
        customer_type: "private".into(),
    }
}
async fn customer(service: &Customers, email: &str) -> (String, CustomerSession) {
    let (_, s) = service.anonymous().await.expect("Anonymous customer");
    let raw = service
        .register(&s, &s.csrf, &profile(email), PASSWORD.into(), None)
        .await
        .expect("Register customer")
        .expect("Immediate activation");
    let session = service
        .session(&raw)
        .await
        .expect("Session query")
        .expect("Registered session");
    (raw, session)
}
fn address() -> AddressInput {
    AddressInput {
        label: "Home".into(),
        first_name: "Alex".into(),
        last_name: "Example".into(),
        phone: "+39 3331234567".into(),
        country: "IT".into(),
        street: "Via Example".into(),
        number: "1".into(),
        postal: "00100".into(),
        city: "Roma".into(),
        region: "RM".into(),
        ..Default::default()
    }
}
fn input<'a>(g: &'a Guest, review: &'a str) -> Confirmation<'a> {
    Confirmation {
        guest: g,
        csrf: &g.csrf,
        revision: g.revision,
        data: data(),
        selection: selection(1),
        review,
        language: lang(),
        online_available: false,
    }
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_registration_rotation_normalization_and_concurrent_uniqueness() {
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let service=Customers::new(pool.clone(),identity.clone());
        let (anonymous,s)=service.anonymous().await.unwrap();
        assert!(matches!(service.register(&s,"bad",&profile("a@example.test"),PASSWORD.into(), None).await,Err(CustomerError::Access(talocart_platform::technical::AccessError::Forbidden))));
        let p1=profile(" A@Example.Test ");let p2=profile("a@example.test");
        let (a,b)=tokio::join!(service.register(&s,&s.csrf,&p1,PASSWORD.into(), None),service.register(&s,&s.csrf,&p2,PASSWORD.into(), None));
        assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        let raw=match (a,b) {(Ok(a),_)|(_,Ok(a))=>a,_=>panic!("One registration must succeed")};
        let raw=raw.expect("Immediate activation");
        let authenticated=service.session(&raw).await.unwrap().unwrap();
        assert_ne!(authenticated.csrf,s.csrf);assert!(service.session(&anonymous).await.unwrap().is_none());
        assert_eq!(service.account(&authenticated).await.unwrap().email,"a@example.test");
        assert!(identity.session(&raw).await.unwrap().is_none());
        let admin=taxonomy_session(&identity).await;assert!(service.session(&admin.token_hash).await.unwrap().is_none());
        let stored:(String,String)=sqlx::query_as("SELECT password_hash,token_hash FROM customers c JOIN customer_sessions s ON s.customer_id=c.id").fetch_one(&pool).await.unwrap();
        assert!(stored.0.starts_with("$argon2id$"));assert_ne!(stored.1,raw);
        let (_,other)=service.anonymous().await.unwrap();
        assert!(matches!(service.register(&other,&other.csrf,&profile("a@example.test"),PASSWORD.into(), None).await,Err(CustomerError::Duplicate)));
        assert!(matches!(service.register(&other,&other.csrf,&profile("invalid"),"short".into(), None).await,Err(CustomerError::Fields(_))));
        service.logout(&authenticated,&authenticated.csrf).await.unwrap();assert!(service.session(&raw).await.unwrap().is_none());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_login_limits_expiry_revocation_and_recovery_are_isolated() {
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let service=Customers::new(pool.clone(),identity.clone());let (raw,c)=customer(&service,"buyer@example.test").await;
        let (_,s)=service.anonymous().await.unwrap();
        for _ in 0..5 {assert!(matches!(service.login(&s,&s.csrf,"buyer@example.test","incorrect".into()).await,Err(CustomerError::Access(talocart_platform::technical::AccessError::Unauthorized))));}
        assert!(matches!(service.login(&s,&s.csrf,"buyer@example.test",PASSWORD.into()).await,Err(CustomerError::Identity(IdentityError::Limited))));
        sqlx::query("DELETE FROM auth_limits WHERE bucket LIKE 'customer-login%'").execute(&pool).await.unwrap();
        let second=service.login(&s,&s.csrf,"buyer@example.test",PASSWORD.into()).await.unwrap();assert!(service.session(&second).await.unwrap().is_some());
        let token=service.issue_reset("buyer@example.test").await.unwrap();let replaced=service.issue_reset("buyer@example.test").await.unwrap();
        let (_,reset)=service.anonymous().await.unwrap();
        assert!(service.reset(&reset,&reset.csrf,&token,"new customer password".into()).await.is_err());
        sqlx::query("UPDATE customer_password_resets SET expires_at=now()-interval '1 second'").execute(&pool).await.unwrap();
        assert!(service.reset(&reset,&reset.csrf,&replaced,"new customer password".into()).await.is_err());
        let fresh=service.issue_reset("buyer@example.test").await.unwrap();
        service.reset(&reset,&reset.csrf,&fresh,"new customer password".into()).await.unwrap();
        assert!(service.session(&raw).await.unwrap().is_none());assert!(service.session(&second).await.unwrap().is_none());
        assert!(service.reset(&reset,&reset.csrf,&fresh,"another customer password".into()).await.is_err());
        let (_,anon)=service.anonymous().await.unwrap();let raw=service.login(&anon,&anon.csrf,"buyer@example.test","new customer password".into()).await.unwrap();
        let signed=service.session(&raw).await.unwrap().unwrap();
        sqlx::query("UPDATE customer_sessions SET expires_at=now()-interval '1 second' WHERE customer_id=$1").bind(signed.customer_id()).execute(&pool).await.unwrap();
        assert!(service.session(&raw).await.unwrap().is_none());assert!(service.account(&signed).await.is_err());assert!(service.account(&c).await.is_err());
        let (_,admin)=identity.anonymous().await.unwrap();assert!(identity.login(&admin,&admin.csrf,"owner@example.com","correct horse battery".into()).await.is_ok());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_profile_revisions_password_checks_and_email_change_revoke_sessions() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        let (raw, c) = customer(&service, "buyer@example.test").await;
        let mut p = profile("buyer@example.test");
        p.first_name = "Updated".into();
        let (a, b) = tokio::join!(
            service.update(&c, &c.csrf, 0, &p, String::new()),
            service.update(&c, &c.csrf, 0, &p, String::new())
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert_eq!(service.account(&c).await.unwrap().revision, 1);
        p.email = "changed@example.test".into();
        assert!(matches!(
            service.update(&c, &c.csrf, 1, &p, "wrong".into()).await,
            Err(CustomerError::Identity(IdentityError::CurrentPassword))
        ));
        assert!(service.session(&raw).await.unwrap().is_some());
        let reset = service.issue_reset("buyer@example.test").await.unwrap();
        assert!(!reset.is_empty());
        assert!(
            service
                .update(&c, &c.csrf, 1, &p, PASSWORD.into())
                .await
                .unwrap()
        );
        assert!(service.session(&raw).await.unwrap().is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM customer_password_resets")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        let (_, anon) = service.anonymous().await.unwrap();
        let raw = service
            .login(&anon, &anon.csrf, &p.email, PASSWORD.into())
            .await
            .unwrap();
        let c = service.session(&raw).await.unwrap().unwrap();
        assert!(
            service
                .change_password(&c, "bad", PASSWORD.into(), "new customer password".into())
                .await
                .is_err()
        );
        assert!(
            service
                .change_password(&c, &c.csrf, "wrong".into(), "new customer password".into())
                .await
                .is_err()
        );
        service
            .change_password(&c, &c.csrf, PASSWORD.into(), "new customer password".into())
            .await
            .unwrap();
        assert!(service.session(&raw).await.unwrap().is_none());
        let (_, anon) = service.anonymous().await.unwrap();
        assert!(
            service
                .login(&anon, &anon.csrf, &p.email, PASSWORD.into())
                .await
                .is_err()
        );
        assert!(
            service
                .login(&anon, &anon.csrf, &p.email, "new customer password".into())
                .await
                .is_ok()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_default_address_switch_concurrency_prefill_and_draft_precedence() {
    isolated(|pool, _| async move {
        let (_, identity, _) = configured(&pool).await;
        let customers = Customers::new(pool.clone(), identity.clone());
        let (_, other) = customer(&customers, "other-default@example.test").await;
        let (raw_customer, customer) = customer(&customers, "default@example.test").await;
        let addresses = CustomerAddresses::new(pool.clone());
        let billing = talocart_commerce::customer_billing::CustomerBilling::new(pool.clone(), identity);
        let mut input = address();
        let first = addresses.save(&customer, &customer.csrf, None, &input).await.unwrap();
        assert!(billing.initial(&customer).await.unwrap().shipping_street.is_empty(), "A sole untagged address is not selected implicitly");
        input.is_default = true;
        input.street = "Default street".into();
        let second = addresses.save(&customer, &customer.csrf, None, &input).await.unwrap();
        assert_eq!(addresses.list(&customer).await.unwrap()[0].id, second);
        let d = billing.initial(&customer).await.unwrap();
        assert_eq!(d.shipping_street, "Default street");
        assert_eq!(d.phone, input.phone);
        assert_eq!(d.email, "default@example.test");
        assert!(billing.initial(&other).await.unwrap().shipping_street.is_empty());
        assert!(addresses.save(&other, &other.csrf, Some((second, 0)), &input).await.is_err());
        assert!(addresses.save(&customer, "bad", Some((first, 0)), &input).await.is_err());
        let carts = Carts::new(pool.clone());
        let (raw_cart, guest) = carts.create().await.unwrap();
        carts.merge_customer(&customer, &customer.csrf, &guest, &guest.csrf, guest.revision).await.unwrap();
        let guest = carts.customer_cart(&customer).await.unwrap().unwrap();
        let app = router(test_app(pool.clone(), "Default address".into()).await);
        let cookie = format!("talocart_customer={raw_customer}; talocart_cart={raw_cart}");
        for path in ["/checkout", "/it/checkout"] {
            let h = html(request(&app, "GET", path, &cookie, "", false).await).await;
            assert!(h.contains("value=\"Default street\""));
            assert!(h.contains(&format!("value=\"{second}\" selected")));
        }
        assert!(carts.details(&guest).await.unwrap().is_none(), "GET only fills the form");
        let mut draft = data(); draft.shipping_street = "Draft street".into();
        carts.save_details(&guest, &guest.csrf, guest.revision, draft).await.unwrap();
        // Both requests are serialized by the customer lock; at most one default remains.
        let (a, b) = tokio::join!(
            addresses.save(&customer, &customer.csrf, Some((first, 0)), &input),
            addresses.save(&customer, &customer.csrf, Some((second, 0)), &input)
        );
        assert!(a.is_ok() || b.is_ok());
        let rows = addresses.list(&customer).await.unwrap();
        assert_eq!(rows.iter().filter(|a| a.is_default).count(), 1);
        let selected = rows.iter().find(|a| a.is_default).unwrap();
        let unselected = rows.iter().find(|a| !a.is_default).unwrap();
        // The database constraint protects the invariant even outside the service.
        let error = sqlx::query("UPDATE customer_addresses SET is_default=true,revision=revision+1 WHERE id=$1")
            .bind(unselected.id).execute(&pool).await.unwrap_err();
        assert_eq!(error.as_database_error().unwrap().code().as_deref(), Some("23505"));
        assert!(matches!(addresses.save(&customer, &customer.csrf, Some((second, 0)), &input).await, Err(CustomerError::Stale)));
        sqlx::query("ALTER TABLE customer_addresses ADD CONSTRAINT synthetic_default_failure CHECK(label<>'Fail default')").execute(&pool).await.unwrap();
        let mut invalid = input.clone(); invalid.label = "Fail default".into();
        assert!(addresses.save(&customer, &customer.csrf, None, &invalid).await.is_err());
        let current = addresses.get(&customer, selected.id).await.unwrap();
        assert!(current.is_default);
        assert_eq!(current.revision, selected.revision, "Failed inserts roll back demotion and revision changes");
        for path in ["/checkout", "/it/checkout"] {
            let h = html(request(&app, "GET", path, &cookie, "", false).await).await;
            assert!(h.contains("value=\"Draft street\""), "The existing draft takes precedence over the default");
        }
        let mut unset = current.input(); unset.is_default = false;
        addresses.save(&customer, &customer.csrf, Some((current.id, current.revision)), &unset).await.unwrap();
        assert!(addresses.list(&customer).await.unwrap().iter().all(|a| !a.is_default));
        let current = addresses.get(&customer, current.id).await.unwrap();
        addresses.save(&customer, &customer.csrf, Some((current.id, current.revision)), &input).await.unwrap();
        let current = addresses.get(&customer, current.id).await.unwrap();
        addresses.delete(&customer, &customer.csrf, current.id, current.revision).await.unwrap();
        assert!(addresses.list(&customer).await.unwrap().iter().all(|a| !a.is_default), "Deleting the default does not promote another address");
        database::migrate(&pool).await.unwrap();
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_addresses_ownership_revisions_checkout_copy_and_no_stock_effect() {
    isolated(|pool, _| async move {
        let (_, identity, _) = configured(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        let (_, a) = customer(&service, "a@example.test").await;
        let (_, b) = customer(&service, "b@example.test").await;
        let addresses = CustomerAddresses::new(pool.clone());
        let id = addresses.save(&a, &a.csrf, None, &address()).await.unwrap();
        assert!(matches!(
            addresses.get(&b, id).await,
            Err(CustomerError::NotFound)
        ));
        assert!(addresses.list(&b).await.unwrap().is_empty());
        assert!(matches!(
            addresses.save(&b, &b.csrf, Some((id, 0)), &address()).await,
            Err(CustomerError::NotFound)
        ));
        assert!(addresses.delete(&b, &b.csrf, id, 0).await.is_err());
        assert!(addresses.delete(&a, "bad", id, 0).await.is_err());
        let mut updated = address();
        updated.street = "New street".into();
        addresses
            .save(&a, &a.csrf, Some((id, 0)), &updated)
            .await
            .unwrap();
        assert!(matches!(
            addresses.delete(&a, &a.csrf, id, 0).await,
            Err(CustomerError::Stale)
        ));
        let carts = Carts::new(pool.clone());
        let (raw, g) = carts.create().await.unwrap();
        assert!(
            addresses
                .use_in_checkout(&b, &b.csrf, id, (&g, &g.csrf, 0))
                .await
                .is_err()
        );
        addresses
            .use_in_checkout(&a, &a.csrf, id, (&g, &g.csrf, 0))
            .await
            .unwrap();
        let g = carts.session(&raw).await.unwrap().unwrap();
        let d = carts.details(&g).await.unwrap().unwrap();
        assert_eq!(d.email, "a@example.test");
        assert_eq!(d.shipping_street, "New street");
        assert_eq!(g.revision, 1);
        assert!(matches!(
            addresses
                .use_in_checkout(&a, &a.csrf, id, (&g, &g.csrf, 0))
                .await,
            Err(CustomerError::Stale)
        ));
        sqlx::query("DELETE FROM checkout_countries WHERE country_code='IT'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(addresses.get(&a, id).await.is_ok());
        assert!(matches!(
            addresses
                .use_in_checkout(&a, &a.csrf, id, (&g, &g.csrf, 1))
                .await,
            Err(CustomerError::Fields(_))
        ));
        assert_eq!(count(&pool, "orders").await, 0);
        assert_eq!(count(&pool, "stock").await, 0);
        assert_eq!(count(&pool, "mail").await, 0);
        addresses.delete(&a, &a.csrf, id, 1).await.unwrap();
        assert!(addresses.list(&a).await.unwrap().is_empty());
    })
    .await;
}

async fn address_checkout_database_failure(
    setup: &'static str,
    cleanup: &'static str,
    sqlstate: &'static str,
) {
    isolated(move |pool, _| async move {
        let (_, identity, _) = configured(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        let (_, customer) = customer(&service, "address-failure@example.test").await;
        let addresses = CustomerAddresses::new(pool.clone());
        let id = addresses
            .save(&customer, &customer.csrf, None, &address())
            .await
            .unwrap();
        let carts = Carts::new(pool.clone());
        let (_, guest) = carts.create().await.unwrap();

        sqlx::raw_sql(setup).execute(&pool).await.unwrap();
        let result = addresses
            .use_in_checkout(&customer, &customer.csrf, id, (&guest, &guest.csrf, 0))
            .await;
        sqlx::raw_sql(cleanup).execute(&pool).await.unwrap();

        match result {
            Err(CustomerError::Technical(
                talocart_platform::technical::TechnicalError::Database(error),
            )) => assert_eq!(error.sqlstate(), Some(sqlstate)),
            other => panic!("Expected original database cause {sqlstate}, got {other:?}"),
        }
        assert!(carts.details(&guest).await.unwrap().is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT revision FROM guest_carts WHERE id=$1")
                .bind(guest.id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        for table in ["orders", "stock", "mail"] {
            assert_eq!(count(&pool, table).await, 0);
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_checkout_address_preserves_destination_database_failure() {
    address_checkout_database_failure(
        "ALTER TABLE checkout_countries RENAME TO checkout_countries_unavailable",
        "ALTER TABLE checkout_countries_unavailable RENAME TO checkout_countries",
        "42P01",
    )
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_checkout_address_preserves_cart_lock_database_failure() {
    address_checkout_database_failure(
        "ALTER TABLE guest_carts RENAME TO guest_carts_unavailable",
        "ALTER TABLE guest_carts_unavailable RENAME TO guest_carts",
        "42P01",
    )
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_checkout_address_preserves_details_database_failure() {
    address_checkout_database_failure(
        "CREATE FUNCTION fail_checkout_address() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'synthetic address persistence failure' USING ERRCODE='40001'; END $$; \
         CREATE TRIGGER fail_checkout_address BEFORE INSERT ON checkout_details \
         FOR EACH ROW EXECUTE FUNCTION fail_checkout_address()",
        "DROP TRIGGER fail_checkout_address ON checkout_details; DROP FUNCTION fail_checkout_address()",
        "40001",
    )
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_checkout_address_preserves_revision_database_failure() {
    address_checkout_database_failure(
        "CREATE FUNCTION fail_checkout_revision() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'synthetic cart revision failure' USING ERRCODE='40P01'; END $$; \
         CREATE TRIGGER fail_checkout_revision BEFORE UPDATE ON guest_carts \
         FOR EACH ROW EXECUTE FUNCTION fail_checkout_revision()",
        "DROP TRIGGER fail_checkout_revision ON guest_carts; DROP FUNCTION fail_checkout_revision()",
        "40P01",
    )
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_address_limit_is_atomic_and_revoked_sessions_cannot_write() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        let (_, c) = customer(&service, "buyer@example.test").await;
        let addresses = CustomerAddresses::new(pool.clone());
        for _ in 0..19 {
            addresses.save(&c, &c.csrf, None, &address()).await.unwrap();
        }
        let a1 = address();
        let a2 = address();
        let (a, b) = tokio::join!(
            addresses.save(&c, &c.csrf, None, &a1),
            addresses.save(&c, &c.csrf, None, &a2)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert_eq!(addresses.list(&c).await.unwrap().len(), 20);
        service.logout(&c, &c.csrf).await.unwrap();
        assert!(addresses.save(&c, &c.csrf, None, &address()).await.is_err());
        assert!(addresses.list(&c).await.is_err());
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_order_attribution_replay_isolation_restart_and_immutable_snapshots() {
    isolated(|pool, options| async move {
        let (p, identity, admin) = configured(&pool).await;
        let service = Customers::new(pool.clone(), identity.clone());
        let (_, a) = customer(&service, "buyer@example.test").await;
        let (_, b) = customer(&service, "b@example.test").await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let (_, old, review) = prepare(&carts, (p, 0), 1, 1).await;
        let previous = orders.confirm(input(&old, &review)).await.unwrap();
        assert!(
            orders
                .customer_history(&a, 1)
                .await
                .unwrap()
                .entries
                .is_empty()
        );
        assert!(matches!(
            orders.for_customer(&a, &previous.reference).await,
            Err(OrderError::NotFound)
        ));
        assert!(
            orders
                .confirm_for_customer(input(&old, &review), &a)
                .await
                .is_err(),
            "Guest replay cannot be claimed by email or login"
        );
        let (_, g, review) = prepare(&carts, (p, 0), 1, 1).await;
        let (one, two) = tokio::join!(
            orders.confirm_for_customer(input(&g, &review), &a),
            orders.confirm_for_customer(input(&g, &review), &a)
        );
        let o = one.unwrap();
        assert_eq!(two.unwrap().id, o.id);
        assert!(matches!(
            orders.confirm_for_customer(input(&g, &review), &b).await,
            Err(OrderError::NotFound)
        ));
        assert!(orders.for_customer(&b, &o.reference).await.is_err());
        assert!(
            orders
                .customer_history(&b, 1)
                .await
                .unwrap()
                .entries
                .is_empty()
        );
        let history = orders.customer_history(&a, i64::MAX).await.unwrap();
        assert_eq!(
            (history.total, history.page, history.entries.len()),
            (1, 1, 1)
        );
        let addresses = CustomerAddresses::new(pool.clone());
        let id = addresses.save(&a, &a.csrf, None, &address()).await.unwrap();
        let mut changed = address();
        changed.street = "Changed after order".into();
        addresses
            .save(&a, &a.csrf, Some((id, 0)), &changed)
            .await
            .unwrap();
        let original = orders.for_customer(&a, &o.reference).await.unwrap();
        assert_eq!(
            original.frozen.customer.shipping_street,
            o.frozen.customer.shipping_street
        );
        orders
            .cancel(&admin, &admin.csrf, o.id, o.revision)
            .await
            .unwrap();
        assert_eq!(
            orders.for_customer(&a, &o.reference).await.unwrap().state,
            "cancelled"
        );
        assert!(
            sqlx::query("UPDATE customer_orders SET customer_id=$1 WHERE order_id=$2")
                .bind(b.customer_id())
                .bind(o.id)
                .execute(&pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM customer_orders WHERE order_id=$1")
                .bind(o.id)
                .execute(&pool)
                .await
                .is_err()
        );
        let reopened = PgPoolOptions::new().connect_with(options).await.unwrap();
        let persisted = Orders::new(reopened.clone(), Identity::new(reopened.clone()));
        assert_eq!(
            persisted.for_customer(&a, &o.reference).await.unwrap().id,
            o.id
        );
        reopened.close().await;
        sqlx::query("UPDATE guest_carts SET expires_at=now()-interval '1 second'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            orders.for_customer(&a, &o.reference).await.unwrap().id,
            o.id
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_order_link_rolls_back_with_stock_or_email_failure() {
    isolated(|pool,_|async move {
        let(p,identity,_)=configured(&pool).await;let service=Customers::new(pool.clone(),identity.clone());let(_,c)=customer(&service,"buyer@example.test").await;let carts=Carts::new(pool.clone());let orders=Orders::new(pool.clone(),identity);let(_,g,review)=prepare(&carts,(p,0),1,1).await;
        sqlx::raw_sql("CREATE FUNCTION fail_customer_mail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic'; END; $$; CREATE TRIGGER fail_customer_mail BEFORE INSERT ON email_outbox FOR EACH ROW EXECUTE FUNCTION fail_customer_mail();").execute(&pool).await.unwrap();
        assert!(orders.confirm_for_customer(input(&g,&review),&c).await.is_err());assert_eq!(count(&pool,"orders").await,0);assert_eq!(count(&pool,"stock").await,0);assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM customer_orders").fetch_one(&pool).await.unwrap(),0);
        sqlx::query("DROP TRIGGER fail_customer_mail ON email_outbox").execute(&pool).await.unwrap();assert!(orders.confirm_for_customer(input(&g,&review),&c).await.is_ok());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_checkout_blocks_account_changes_between_review_and_confirmation() {
    isolated(|pool,_|async move {
        let(p,identity,_)=configured(&pool).await;let service=Customers::new(pool.clone(),identity);let(raw_a,a)=customer(&service,"a@example.test").await;let(raw_b,b)=customer(&service,"b@example.test").await;
        let carts=Carts::new(pool.clone());let(raw,g,_)=prepare(&carts,(p,0),1,1).await;carts.save_checkout(&g,&g.csrf,g.revision,data(),selection(1),lang()).await.unwrap();let g=carts.session(&raw).await.unwrap().unwrap();carts.merge_customer(&a,&a.csrf,&g,&g.csrf,g.revision).await.unwrap();let g=carts.customer_cart(&a).await.unwrap().unwrap();carts.save_checkout(&g,&g.csrf,g.revision,data(),selection(1),lang()).await.unwrap();let g=carts.customer_cart(&a).await.unwrap().unwrap();let q=carts.checkout_quote(&g,lang(),"IT",None).await.unwrap();
        let body=format!("csrf={}&revision={}&action=confirm&shipping_method_id=1&payment_method_id=1&review={}&customer_id={}",g.csrf,g.revision,q.review,a.customer_id().unwrap());
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        for cookie in [format!("talocart_cart={raw}; talocart_customer={raw_b}"),format!("talocart_cart={raw}")] {
            assert_eq!(request(&app,"POST","/checkout/methods",&cookie,&body,false).await.status(),StatusCode::FORBIDDEN);
        }
        assert_eq!(count(&pool,"orders").await,0);
        let cookie=format!("talocart_cart={raw}; talocart_customer={raw_a}");
        let page=html(request(&app,"GET","/checkout/methods",&cookie,"",false).await).await;assert!(page.contains(&format!("name=\"customer_id\" value=\"{}\"",a.customer_id().unwrap())));
        assert_eq!(request(&app,"POST","/checkout/methods",&cookie,&body,false).await.status(),StatusCode::SEE_OTHER);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT customer_id FROM customer_orders").fetch_one(&pool).await.unwrap(),a.customer_id().unwrap());
        let forged=body.replace(&format!("customer_id={}",a.customer_id().unwrap()),&format!("customer_id={}",b.customer_id().unwrap()));
        assert_ne!(request(&app,"POST","/checkout/methods",&format!("talocart_cart={raw}; talocart_customer={raw_b}"),&forged,false).await.status(),StatusCode::SEE_OTHER);
        assert_eq!(count(&pool,"orders").await,1);assert_eq!(count(&pool,"stock").await,1);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_history_is_bounded_and_disabled_accounts_lose_access() {
    isolated(|pool, _| async move {
        let (p, identity, _) = configured(&pool).await;
        sqlx::query("UPDATE catalog_products SET stock=100 WHERE id=$1")
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        let service = Customers::new(pool.clone(), identity.clone());
        let (raw, c) = customer(&service, "buyer@example.test").await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity);
        let mut first = String::new();
        for i in 0..21 {
            let (_, g, review) = prepare(&carts, (p, 0), 1, 1).await;
            let order = orders
                .confirm_for_customer(input(&g, &review), &c)
                .await
                .unwrap();
            if i == 0 {
                first = order.reference;
            }
        }
        let one = orders.customer_history(&c, 0).await.unwrap();
        assert_eq!((one.total, one.page, one.entries.len()), (21, 1, 20));
        let two = orders.customer_history(&c, i64::MAX).await.unwrap();
        assert_eq!((two.total, two.page, two.entries.len()), (21, 2, 1));
        assert_eq!(two.entries[0].reference, first);
        sqlx::query("UPDATE customers SET active=false WHERE id=$1")
            .bind(c.customer_id())
            .execute(&pool)
            .await
            .unwrap();
        assert!(service.session(&raw).await.unwrap().is_none());
        assert!(orders.customer_history(&c, 1).await.is_err());
        assert!(orders.for_customer(&c, &first).await.is_err());
        let (_, anon) = service.anonymous().await.unwrap();
        assert!(
            service
                .login(&anon, &anon.csrf, "buyer@example.test", PASSWORD.into())
                .await
                .is_err()
        );
        assert_eq!(count(&pool, "orders").await, 21);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_http_bilingual_forms_private_csrf_cookie_roles_and_signout() {
    isolated(|pool,_|async move {
        let (_,identity,_)=configured(&pool).await;let app=router(test_app_config(pool.clone(),"Shop".into(), talocart_web::AppStateConfig { secure_cookies: true, ..Default::default() }).await);
        for prefix in ["","/it"] {
            let route=format!("{prefix}/account/register");let head=request(&app,"HEAD",&route,"","",false).await;assert!(head.headers().get(header::SET_COOKIE).is_none());
            let response=request(&app,"GET",&route,"","",false).await;assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");assert_eq!(response.headers()[header::CONTENT_LANGUAGE],if prefix.is_empty(){"en"}else{"it"});
            let raw=response.headers()[header::SET_COOKIE].to_str().unwrap();assert!(raw.contains("HttpOnly")&&raw.contains("Secure")&&raw.contains("SameSite=Lax"));let anon=cookie(&response);let body=html(response).await;let csrf_token=csrf(&body);
            let address=if prefix.is_empty(){"en@example.test"}else{"it@example.test"};
            let f=format!("csrf={csrf_token}&email={address}&first_name=Alex&last_name=Example&password=synthetic+customer+password&password_confirmation=synthetic+customer+password");
            assert_eq!(request(&app,"POST",&route,&anon,&f.replace(&csrf_token,"wrong"),false).await.status(),StatusCode::FORBIDDEN);
            let response=request(&app,"POST",&route,&anon,&f,false).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);let authenticated=cookie(&response);assert_ne!(anon,authenticated);
            let response=request(&app,"GET",&format!("{prefix}/account"),&authenticated,"",false).await;let body=html(response).await;assert!(body.contains(address));assert!(!body.contains(PASSWORD));
            assert_eq!(request(&app,"GET","/admin",&authenticated,"",false).await.status(),StatusCode::SEE_OTHER);
            assert_eq!(request(&app,"GET",&format!("{prefix}/account/orders/ORD-2026-999"),&authenticated,"",true).await.status(),StatusCode::NOT_FOUND);
            let response=request(&app,"POST",&format!("{prefix}/account/logout"),&authenticated,&format!("csrf={}",csrf(&body)),false).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);assert_eq!(response.headers().get_all(header::SET_COOKIE).iter().count(),2);
            let response=request(&app,"GET",&format!("{prefix}/account"),&authenticated,"",true).await;assert_eq!(response.status(),StatusCode::UNAUTHORIZED);assert_eq!(response.headers()["hx-redirect"],format!("{prefix}/account/login"));
        }
        let (_,admin)=identity.anonymous().await.unwrap();let raw=identity.login(&admin,&admin.csrf,"owner@example.com","correct horse battery".into()).await.unwrap();
        let response=request(&app,"GET","/account/orders",&format!("talocart_customer={raw}"),"",false).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL, npm ci and Playwright Chromium"]
async fn customers_browser_bilingual_desktop_mobile_and_no_js() {
    for chosen_theme in ["default", "essential"] {
        isolated(move |pool, _| async move {
            let (_, _, _) = configured(&pool).await;
            sqlx::query("UPDATE customer_registration_settings SET allow_guest_checkout=false")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE catalog_products SET stock=100 WHERE model='METHODS-1'")
                .execute(&pool)
                .await
                .unwrap();
            let pending_profiles = carts::browser_pending(&pool).await;
            sqlx::query("UPDATE storefront_settings SET theme_id=$1 WHERE id=1")
                .bind(chosen_theme)
                .execute(&pool)
                .await
                .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let state = test_app_config(
                pool.clone(),
                "Customer test".into(),
                talocart_web::AppStateConfig {
                    public_origin: origin.clone(),
                    ..Default::default()
                },
            )
            .await;
            let task = tokio::spawn(async move {
                axum::serve(
                    listener,
                    router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .await
                .unwrap();
            });
            let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../scripts/browser-customers.cjs");
            let mut child = tokio::process::Command::new("node")
                .arg(script)
                .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use tokio::io::AsyncWriteExt;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(
                    serde_json::json!({"origin":origin,"pendingProfiles":pending_profiles})
                        .to_string()
                        .as_bytes(),
                )
                .await
                .unwrap();
            let result = child.wait_with_output().await.unwrap();
            task.abort();
            let _ = task.await;
            assert!(
                result.status.success(),
                "Customer browser failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM customer_orders")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                8
            );
            assert_eq!(count(&pool, "orders").await, 8);
            assert_eq!(count(&pool, "stock").await, 8);
            assert_eq!(count(&pool, "mail").await, 16);
        })
        .await;
    }
}

#[tokio::test]
#[ignore = "Audit probe requires dedicated PostgreSQL"]
async fn customers_login_rechecks_email_after_concurrent_change() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        let (_, c) = customer(&service, "before@example.test").await;
        let (_, anon) = service.anonymous().await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *tx).await.unwrap();
        sqlx::query("SELECT id FROM customers WHERE id=$1 FOR UPDATE").bind(c.customer_id()).execute(&mut *tx).await.unwrap();
        let login_service = service.clone();
        let login = tokio::spawn(async move { login_service.login(&anon, &anon.csrf, "before@example.test", PASSWORD.into()).await });
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&pool).await.unwrap();
                if blocked { break; }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.expect("Login reached transactional recheck");
        // Same locked mutation and revocation as Customers::update for an email change.
        sqlx::query("UPDATE customers SET email='after@example.test',revision=revision+1 WHERE id=$1").bind(c.customer_id()).execute(&mut *tx).await.unwrap();
        sqlx::query("DELETE FROM customer_password_resets WHERE customer_id=$1").bind(c.customer_id()).execute(&mut *tx).await.unwrap();
        sqlx::query("DELETE FROM customer_sessions WHERE customer_id=$1").bind(c.customer_id()).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let result = login.await.unwrap();
        assert!(result.is_err(), "Old-email login created a session AFTER email change and session revocation");
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_limits_do_not_lock_out_unrelated_sessions() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        customer(&service, "healthy@example.test").await;
        let (_, noisy) = service.anonymous().await.unwrap();
        for _ in 0..100 {
            let _ = service.login(&noisy, &noisy.csrf, "invalid", PASSWORD.into()).await;
        }
        assert!(matches!(service.login(&noisy, &noisy.csrf, "invalid", PASSWORD.into()).await, Err(CustomerError::Identity(IdentityError::Limited))));
        let (_, healthy) = service.anonymous().await.unwrap();
        assert!(service.login(&healthy, &healthy.csrf, "healthy@example.test", PASSWORD.into()).await.is_ok(), "Other sessions must not inherit the noisy session budget");
        // Saturated legacy shared buckets must not prevent new unrelated users.
        sqlx::query("INSERT INTO auth_limits(bucket,attempts,expires_at) VALUES('customer-register',20,now()+interval '15 minutes'),('customer-reset',30,now()+interval '15 minutes'),('customer-anonymous',200,now()+interval '15 minutes') ON CONFLICT(bucket) DO UPDATE SET attempts=EXCLUDED.attempts").execute(&pool).await.unwrap();
        let (_, fresh) = service.anonymous().await.expect("No global anonymous lockout");
        assert!(service.register(&fresh, &fresh.csrf, &profile("fresh@example.test"), PASSWORD.into(), None).await.is_ok());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_origin_limits_ignore_forwarded_headers_and_isolate_peers() {
    isolated(|pool, _| async move {
        let (_, identity, _) = configured(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        let noisy = "192.0.2.1".parse().unwrap();
        let healthy = "192.0.2.2".parse().unwrap();
        use talocart_platform::identity::CustomerAuthAction as Action;
        for (action, maximum) in [
            (Action::Anonymous, 200),
            (Action::Login, 100),
            (Action::Register, 60),
            (Action::Reset, 30),
        ] {
            for _ in 0..maximum {
                service.limit_origin(noisy, action).await.unwrap();
            }
            assert!(matches!(
                service.limit_origin(noisy, action).await,
                Err(CustomerError::Identity(IdentityError::Limited))
            ));
            service.limit_origin(healthy, action).await.unwrap();
        }
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (peer, expected) in [
            ("192.0.2.1:1234", StatusCode::TOO_MANY_REQUESTS),
            ("[::ffff:192.0.2.1]:1234", StatusCode::TOO_MANY_REQUESTS),
            ("192.0.2.2:1234", StatusCode::OK),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/account/login")
                        .extension(axum::extract::ConnectInfo(
                            peer.parse::<std::net::SocketAddr>().unwrap(),
                        ))
                        .header("x-forwarded-for", "192.0.2.250")
                        .header("forwarded", "for=192.0.2.250")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            if expected == StatusCode::TOO_MANY_REQUESTS {
                assert!(response.headers().contains_key("retry-after"));
            }
        }
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_proxy_origin_limits_require_https_and_preserve_session_limits() {
    isolated(|pool, _| async move {
        let (_, identity, _) = configured(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        use talocart_platform::identity::CustomerAuthAction as Action;
        for origin in ["127.0.0.1", "::1", "192.0.2.1"] {
            for _ in 0..200 {
                service
                    .limit_origin(origin.parse().unwrap(), Action::Anonymous)
                    .await
                    .unwrap();
            }
        }
        for (https, secure) in [(false, false), (true, false), (false, true), (true, true)] {
            let app = router(
                test_app_config(
                    pool.clone(),
                    "Shop".into(),
                    talocart_web::AppStateConfig {
                        public_origin: if https {
                            "https://shop.example.test"
                        } else {
                            "http://127.0.0.1:3000"
                        }
                        .into(),
                        secure_cookies: secure,
                        ..Default::default()
                    },
                )
                .await,
            );
            for peer in [
                "127.0.0.1:1234",
                "[::ffff:127.0.0.1]:1234",
                "[::1]:1234",
                "192.0.2.1:1234",
            ] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri("/account/login")
                            .extension(axum::extract::ConnectInfo(
                                peer.parse::<std::net::SocketAddr>().unwrap(),
                            ))
                            .header("x-forwarded-for", "192.0.2.250")
                            .header("x-forwarded-proto", "https")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    if https && secure && !peer.starts_with("192.") {
                        StatusCode::OK
                    } else {
                        StatusCode::TOO_MANY_REQUESTS
                    },
                    "https={https}, secure={secure}, peer={peer}"
                );
            }
        }
        // Proxy delegation changes only transport limits: service-owned limits survive.
        let (_, session) = service.anonymous().await.unwrap();
        for _ in 0..100 {
            let result = service
                .login(
                    &session,
                    &session.csrf,
                    "invalid-email",
                    "bad password".into(),
                )
                .await;
            assert!(
                matches!(
                    result,
                    Err(CustomerError::Access(
                        talocart_platform::technical::AccessError::Unauthorized
                    ))
                ),
                "Expected rejected credentials before exhausting the session budget: {result:?}"
            );
        }
        assert!(matches!(
            service
                .login(
                    &session,
                    &session.csrf,
                    "invalid-email",
                    "bad password".into()
                )
                .await,
            Err(CustomerError::Identity(IdentityError::Limited))
        ));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_http_return_targets_and_separate_profile_email_forms() {
    isolated(|pool, _| async move {
        let (_, identity, _) = configured(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        customer(&service, "buyer@example.test").await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for prefix in ["", "/it"] {
            let response = request(&app,"GET",&format!("{prefix}/account/orders?page=2"),"","",true).await;
            assert_eq!(response.status(),StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()["hx-redirect"],format!("{prefix}/account/login?next=%2Faccount%2Forders%3Fpage%3D2"));
            for (next, target) in [("%2Faccount%2Forders%3Fpage%3D2", "/account/orders?page=2"), ("%2Fcheckout", "/checkout"), ("https%3A%2F%2Fevil.test", "/account"), ("%2F%2Fevil.test", "/account"), ("%2Faccount%2Flogout", "/account"), ("%2Fadmin", "/account"), ("%2Faccount%2F..%2Fadmin", "/account"), ("%2F%5Cevil.test", "/account"), ("%252F%252Fevil.test", "/account")] {
                let response=request(&app,"GET",&format!("{prefix}/account/login?next={next}"),"","",false).await;
                let anon=cookie(&response);let body=html(response).await;
                let form=format!("csrf={}&email=buyer@example.test&password=synthetic+customer+password&next={next}",csrf(&body));
                let response=request(&app,"POST",&format!("{prefix}/account/login"),&anon,&form,false).await;
                assert_eq!(response.status(),StatusCode::SEE_OTHER);
                assert_eq!(response.headers()[header::LOCATION],format!("{prefix}{target}"));
            }
        }
        let (_,session)=customer(&service,"profile@example.test").await;
        let (_,anon)=service.anonymous().await.unwrap();
        let raw=service.login(&anon,&anon.csrf,"profile@example.test",PASSWORD.into()).await.unwrap();
        let signed=format!("talocart_customer={raw}");
        let body=html(request(&app,"GET","/account",&signed,"",false).await).await;
        assert!(!body.contains("id=\"customer-email\""));
        assert!(!body.contains("id=\"customer-current_password\""));
        assert!(body.contains("href=\"/account/email\""));
        let f=format!("csrf={}&revision=0&first_name=Updated&last_name=Name&email=ignored@example.test",csrf(&body));
        let response=request(&app,"POST","/account",&signed,&f,false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION],"/account?saved=1");
        assert_eq!(service.account(&session).await.unwrap().email,"profile@example.test");
        let body=html(request(&app,"GET","/account/email",&signed,"",false).await).await;
        let f=format!("csrf={}&revision=1&email=updated@example.test&current_password=wrong",csrf(&body));
        assert_eq!(request(&app,"POST","/account/email",&signed,&f,false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        let response=request(&app,"POST","/account/email",&signed,&f.replace("current_password=wrong","current_password=synthetic+customer+password"),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION],"/account/login?changed=email");
        assert!(service.session(&raw).await.unwrap().is_none());
        let names:(String,String)=sqlx::query_as("SELECT first_name,last_name FROM customers WHERE email='updated@example.test'").fetch_one(&pool).await.unwrap();
        assert_eq!(names,("Updated".into(),"Name".into()));
    }).await;
}

#[path = "customers/carts.rs"]
mod carts;

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_master_changes_preserve_drafts_and_order_snapshots_and_bound_history() {
    use talocart_commerce::customer_billing::{BillingInput, CustomerBilling};
    use talocart_platform::identity::customer_admin::CustomerAdmin;
    isolated(|pool, _| async move {
        let (p, identity, owner) = configured(&pool).await;
        let customers = Customers::new(pool.clone(), identity.clone());
        let (_, session) = customer(&customers, "master@example.test").await;
        let id = session.customer_id().unwrap();
        let billing = CustomerBilling::new(pool.clone(), identity.clone());
        let b = BillingInput {
            invoice_requested: true,
            billing_first_name: "Alex".into(),
            billing_last_name: "Example".into(),
            billing_country: "IT".into(),
            billing_street: "Master Street".into(),
            tax_id: "RSSMRA80A01H501U".into(),
            billing_number: "5".into(),
            billing_postal: "00100".into(),
            billing_city: "Roma".into(),
            billing_region: "RM".into(),
            ..BillingInput::default()
        };
        billing.save(&owner, &owner.csrf, id, -1, &b).await.unwrap();
        let carts = Carts::new(pool.clone());
        let (_, draft) = carts.create().await.unwrap();
        carts
            .save_details(&draft, &draft.csrf, 0, data())
            .await
            .unwrap();
        let draft_before: String =
            sqlx::query_scalar("SELECT to_jsonb(d)::text FROM checkout_details d WHERE cart_id=$1")
                .bind(draft.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let (raw, g, review) = prepare(&carts, (p, 0), 1, 1).await;
        let order = Orders::new(pool.clone(), identity.clone())
            .confirm_for_customer(input(&g, &review), &session)
            .await
            .unwrap();
        let snapshot =
            sqlx::query_scalar::<_, String>("SELECT to_jsonb(o)::text FROM orders o WHERE id=$1")
                .bind(order.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let guest = carts.session(&raw).await.unwrap().unwrap();
        let stored_billing = billing.get(&owner, id).await.unwrap().unwrap();
        let mut changed = b.clone();
        changed.billing_street = "Changed master street".into();
        billing
            .save(&owner, &owner.csrf, id, stored_billing.revision, &changed)
            .await
            .unwrap();
        let admin = CustomerAdmin::new(pool.clone(), identity.clone());
        let mut profile = admin.get(&owner, id).await.unwrap().profile();
        profile.email = "changed-master@example.test".into();
        admin
            .save(&owner, &owner.csrf, id, 0, &profile)
            .await
            .unwrap();
        assert_eq!(
            snapshot,
            sqlx::query_scalar::<_, String>("SELECT to_jsonb(o)::text FROM orders o WHERE id=$1")
                .bind(order.id)
                .fetch_one(&pool)
                .await
                .unwrap()
        );
        assert_eq!(
            draft_before,
            sqlx::query_scalar::<_, String>(
                "SELECT to_jsonb(d)::text FROM checkout_details d WHERE cart_id=$1"
            )
            .bind(draft.id)
            .fetch_one(&pool)
            .await
            .unwrap()
        );
        assert_eq!(guest.id, g.id);
        let orders = Orders::new(pool.clone(), identity.clone());
        let history = orders
            .admin_customer_history(&owner, id, 999)
            .await
            .unwrap();
        assert_eq!(history.total, 1);
        assert_eq!(history.page, 1);
        assert_eq!(history.entries[0].id, order.id);
        let raw = authenticated(&identity).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let response = request(
            &app,
            "GET",
            &format!("/admin/customers/{id}"),
            &format!("talocart_session={raw}"),
            "",
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            html(response)
                .await
                .contains(&format!("href=\"/admin/orders/{}\"", order.id))
        );

        assert!(orders.admin_customer_history(&owner, id, 0).await.is_err());
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Customer only",
                &[Permission::CustomersRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "customer-only@example.test",
                "correct horse battery".into(),
                talocart_platform::identity::Role::Custom(role),
            )
            .await
            .unwrap();
        let raw = login_as(&identity, "customer-only@example.test").await;
        let reader = identity.session(&raw).await.unwrap().unwrap();
        assert!(matches!(
            orders.admin_customer_history(&reader, id, 1).await,
            Err(OrderError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
    })
    .await;
}

#[path = "customers/quantity_discounts.rs"]
mod quantity_discounts;

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customers_context_distinguishes_absent_anonymous_authenticated_expired_and_revoked() {
    isolated(|pool, _| async move {
        let (_, identity, _) = configured(&pool).await;
        let service = Customers::new(pool.clone(), identity);
        let app = router(test_app(pool.clone(), "Request context".into()).await);
        async fn sessions(pool: &PgPool) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM customer_sessions")
                .fetch_one(pool)
                .await
                .unwrap()
        }
        let before = sessions(&pool).await;
        for prefix in ["", "/it"] {
            assert_eq!(
                request(&app, "HEAD", &format!("{prefix}/account"), "", "", false)
                    .await
                    .status(),
                StatusCode::SEE_OTHER,
                "HEAD retains the native login redirect"
            );
            assert_eq!(
                request(&app, "GET", &format!("{prefix}/account"), "", "", false)
                    .await
                    .status(),
                StatusCode::SEE_OTHER
            );
            let head = request(
                &app,
                "HEAD",
                &format!("{prefix}/account/login"),
                "",
                "",
                false,
            )
            .await;
            assert_eq!(head.status(), StatusCode::OK);
            assert!(!head.headers().contains_key(header::SET_COOKIE));
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("{prefix}/account/login"),
                    "",
                    "csrf=synthetic",
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            // A captured authentication rejection must not overtake native Form parsing.
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("{prefix}/account"),
                    "",
                    "revision=invalid",
                    false
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        assert_eq!(sessions(&pool).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_carts")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        let (anonymous, _) = service.anonymous().await.unwrap();
        let anon_cookie = format!("talocart_customer={anonymous}");
        let before = sessions(&pool).await;
        for prefix in ["", "/it"] {
            let response = request(
                &app,
                "GET",
                &format!("{prefix}/account"),
                &anon_cookie,
                "",
                true,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                response.headers()["hx-redirect"],
                format!("{prefix}/account/login")
            );
            let response = request(
                &app,
                "GET",
                &format!("{prefix}/account/login"),
                &anon_cookie,
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!response.headers().contains_key(header::SET_COOKIE));
        }
        assert_eq!(sessions(&pool).await, before);
        let (raw, authenticated) = customer(&service, "context@example.test").await;
        let auth_cookie = format!("talocart_customer={raw}");
        let addresses = CustomerAddresses::new(pool.clone());
        addresses
            .save(&authenticated, &authenticated.csrf, None, &address())
            .await
            .unwrap();
        for prefix in ["", "/it"] {
            let response = request(
                &app,
                "GET",
                &format!("{prefix}/account"),
                &auth_cookie,
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let response = request(
                &app,
                "HEAD",
                &format!("{prefix}/account/addresses"),
                &auth_cookie,
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!response.headers().contains_key(header::SET_COOKIE));
        }
        let carts = talocart_commerce::cart::Carts::new(pool.clone());
        assert!(carts.customer_cart(&authenticated).await.unwrap().is_none());
        let response = request(&app, "GET", "/account/addresses", &auth_cookie, "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .any(|v| v.to_str().unwrap().starts_with("talocart_cart="))
        );
        let body = html(response).await;
        let cart = carts
            .customer_cart(&authenticated)
            .await
            .unwrap()
            .expect("GET establishes an owned cart");
        assert!(body.contains(&format!("name=\"cart_csrf\" value=\"{}\"", cart.csrf)));
        assert!(body.contains(&format!(
            "name=\"cart_revision\" value=\"{}\"",
            cart.revision
        )));
        sqlx::query(
            "UPDATE customer_sessions SET expires_at=now()-interval '1 second' WHERE token_hash=$1",
        )
        .bind(digest(&raw))
        .execute(&pool)
        .await
        .unwrap();
        let (revoked, session) = customer(&service, "revoked-context@example.test").await;
        service.logout(&session, &session.csrf).await.unwrap();
        let before = sessions(&pool).await;
        for raw in [raw, revoked] {
            let cookie = format!("talocart_customer={raw}");
            for prefix in ["", "/it"] {
                assert_eq!(
                    request(
                        &app,
                        "GET",
                        &format!("{prefix}/account"),
                        &cookie,
                        "",
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
                        &format!("{prefix}/account"),
                        &cookie,
                        "csrf=synthetic",
                        false
                    )
                    .await
                    .status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
        assert_eq!(
            sessions(&pool).await,
            before,
            "Invalid capabilities never create replacement sessions on POST"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn guest_checkout_policy_blocks_new_orders_and_preserves_customer_and_replay() {
    use talocart_platform::identity::customer_registration::RegistrationSettings;
    isolated(|pool, _| async move {
        let (product, identity, owner) = configured(&pool).await;
        let carts = Carts::new(pool.clone());
        let orders = Orders::new(pool.clone(), identity.clone());
        let (_, old_guest, old_review) = prepare(&carts, (product, 0), 1, 1).await;
        let old_order = confirm(&orders, &old_guest, &old_review, 1).await.unwrap();
        let (raw, guest, review) = prepare(&carts, (product, 0), 1, 1).await;
        let customers = Customers::new(pool.clone(), identity);
        let (customer_raw, signed) = customer(&customers, "buyer@example.test").await;
        let mut policy = RegistrationSettings::load(&pool).await.unwrap();
        assert!(policy.allow_guest_checkout);
        policy.allow_guest_checkout = false;
        policy.save(&pool, &owner, &owner.csrf).await.unwrap();
        let forbidden = |e| matches!(e, talocart_commerce::cart::CartError::Forbidden);
        assert!(forbidden(
            carts
                .checkout_quote(&guest, lang(), "IT", None)
                .await
                .err()
                .unwrap()
        ));
        assert!(forbidden(
            carts
                .save_details(&guest, &guest.csrf, guest.revision, data())
                .await
                .err()
                .unwrap()
        ));
        assert!(forbidden(
            carts
                .save_checkout(
                    &guest,
                    &guest.csrf,
                    guest.revision,
                    data(),
                    selection(1),
                    lang()
                )
                .await
                .err()
                .unwrap()
        ));
        assert!(forbidden(
            carts
                .update_checkout_choices(&guest, &guest.csrf, guest.revision, selection(1), lang())
                .await
                .err()
                .unwrap()
        ));
        assert!(matches!(
            confirm(&orders, &guest, &review, 1).await,
            Err(OrderError::Cart(
                talocart_commerce::cart::CartError::Forbidden
            ))
        ));
        assert_eq!(count(&pool, "orders").await, 1);
        assert_eq!(count(&pool, "stock").await, 1);
        assert_eq!(count(&pool, "mail").await, 2);
        assert_eq!(
            confirm(&orders, &old_guest, &old_review, 1)
                .await
                .unwrap()
                .id,
            old_order.id
        );
        assert_eq!(
            orders
                .for_guest(&old_guest, &old_order.reference)
                .await
                .unwrap()
                .id,
            old_order.id
        );
        assert_eq!(carts.quote(&guest, lang()).await.unwrap().lines.len(), 1);
        let app = router(test_app(pool.clone(), "Guest policy".into()).await);
        let cookie = format!("talocart_cart={raw}");
        for prefix in ["", "/it"] {
            let login = format!("{prefix}/account/login?next=%2Fcheckout");
            for path in ["/checkout", "/checkout/methods"] {
                for method in ["GET", "HEAD", "POST"] {
                    for htmx in [false, true] {
                        let response = request(
                            &app,
                            method,
                            &format!("{prefix}{path}"),
                            &cookie,
                            "customer_id=123&email=buyer%40example.test",
                            htmx,
                        )
                        .await;
                        assert_eq!(
                            response.status(),
                            if htmx {
                                StatusCode::UNAUTHORIZED
                            } else {
                                StatusCode::SEE_OTHER
                            }
                        );
                        assert_eq!(response.headers()[header::LOCATION], login);
                        if htmx {
                            assert_eq!(response.headers()["hx-redirect"], login);
                        }
                        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
                    }
                }
            }
        }
        // Login merges the preserved guest cart through the existing authorized boundary.
        carts
            .merge_customer(&signed, &signed.csrf, &guest, &guest.csrf, guest.revision)
            .await
            .unwrap();
        let owned = carts.customer_cart(&signed).await.unwrap().unwrap();
        let q = carts
            .checkout_quote(&owned, lang(), "IT", Some(selection(1)))
            .await
            .unwrap();
        let order = orders
            .confirm_for_customer(input(&owned, &q.review), &signed)
            .await
            .unwrap();
        assert_ne!(order.id, old_order.id);
        customers.logout(&signed, &signed.csrf).await.unwrap();
        let response = request(
            &app,
            "GET",
            "/checkout",
            &format!("talocart_customer={customer_raw}; {cookie}"),
            "",
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(
            response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .contains("/account/login")
        );
        policy = RegistrationSettings::load(&pool).await.unwrap();
        policy.allow_guest_checkout = true;
        policy.save(&pool, &owner, &owner.csrf).await.unwrap();
        let (_, fresh, review) = prepare(&carts, (product, 0), 1, 1).await;
        confirm(&orders, &fresh, &review, 1).await.unwrap();
    })
    .await;
}
