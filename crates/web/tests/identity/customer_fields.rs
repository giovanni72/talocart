use super::*;
use talocart_commerce::{
    cart::Carts,
    checkout_details::Details,
    customer_addresses::{AddressInput, CustomerAddresses},
};
use talocart_platform::{
    customer_fields::{self, Error, Policy},
    identity::{CustomerError, CustomerProfile, Customers},
};
fn details() -> Details {
    Details {
        email: "buyer@example.test".into(),
        first_name: "Alex".into(),
        last_name: "Example".into(),
        phone: "+39 3331234567".into(),
        shipping_country: "IT".into(),
        shipping_street: "Via Example".into(),
        shipping_number: "1".into(),
        shipping_city: "Roma".into(),
        shipping_postal: "00100".into(),
        shipping_region: "RM".into(),
        ..Default::default()
    }
}
fn choices(changes: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut choices = Policy::default().choices();
    for (key, mode) in changes {
        choices
            .iter_mut()
            .find(|(k, _)| k == key)
            .expect("Known policy key")
            .1 = mode.to_string();
    }
    choices
}
async fn owner(pool: &PgPool) -> talocart_platform::identity::Session {
    let identity = installed(pool).await;
    taxonomy_session(&identity).await
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_fields_permissions_csrf_registry_revision_and_http_bilingual() {
    isolated(|pool, _| async move {
        let session = owner(&pool).await;
        assert!(matches!(
            customer_fields::save(&pool, &session, "invalid", 0, &choices(&[])).await,
            Err(Error::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert!(matches!(
            customer_fields::save(
                &pool,
                &session,
                &session.csrf,
                0,
                &choices(&[("private.email", "hidden")])
            )
            .await,
            Err(Error::Invalid)
        ));
        let custom = choices(&[
            ("business.pec", "required"),
            ("business.recipient_code", "required"),
        ]);
        let baseline = choices(&[]);
        let (a, b) = tokio::join!(
            customer_fields::save(&pool, &session, &session.csrf, 0, &custom),
            customer_fields::save(&pool, &session, &session.csrf, 0, &baseline)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let current = customer_fields::load(&pool).await.unwrap();
        assert_eq!(current.revision, 1);
        customer_fields::save(&pool, &session, &session.csrf, 1, &custom)
            .await
            .unwrap();
        for json in [
            r#"{"private.unknown":"required"}"#,
            r#"{"private.email":"hidden"}"#,
            r#"{"private.first_name":null}"#,
        ] {
            assert!(
                sqlx::query(
                    "UPDATE customer_field_policy SET revision=revision+1,overrides=$1::jsonb"
                )
                .bind(json)
                .execute(&pool)
                .await
                .is_err()
            );
        }
        let identity = Identity::new(pool.clone());
        let role = identity
            .create_role(
                &session,
                &session.csrf,
                "Field reader",
                &[talocart_platform::identity::Permission::SettingsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &session,
                &session.csrf,
                "field-reader@example.test",
                "correct horse battery".into(),
                talocart_platform::identity::Role::parse(&role).unwrap(),
            )
            .await
            .unwrap();
        let reader = login_as(&identity, "field-reader@example.test").await;
        let raw = authenticated(&Identity::new(pool.clone())).await;
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Fields".into()).await);
        let reader_cookie = format!("talocart_session={reader}");
        let readonly = request(
            &app,
            "GET",
            "/admin/settings/customer-fields",
            &reader_cookie,
            "",
            false,
        )
        .await;
        assert_eq!(readonly.status(), StatusCode::OK);
        assert!(
            !html(readonly)
                .await
                .contains("<button class=\"ui-button\" type=\"submit\"")
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/customer-fields",
                &reader_cookie,
                "csrf=invalid&revision=2",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        for (locale, title) in [
            ("it", "Clienti e indirizzi"),
            ("en", "Customers and addresses"),
        ] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let r = request(
                &app,
                "GET",
                "/admin/settings/customer-fields",
                &cookie,
                "",
                false,
            )
            .await;
            assert_eq!(r.status(), StatusCode::OK);
            let h = html(r).await;
            assert!(h.contains(title));
            assert!(h.contains("SDI"));
            assert!(h.contains("PEC"));
            assert!(h.contains("name=\"business.pec\""));
        }
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/customer-fields",
                &cookie,
                "csrf=invalid&revision=2",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert!(
            request(
                &app,
                "GET",
                "/admin/settings/customer-fields",
                "",
                "",
                false
            )
            .await
            .status()
            .is_redirection()
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_fields_checkout_hidden_retention_required_channels_and_changed_draft() {
    isolated(|pool, _| async move {
        let s = owner(&pool).await;
        sqlx::query("INSERT INTO checkout_countries(country_code) VALUES('IT')")
            .execute(&pool)
            .await
            .unwrap();
        let carts = Carts::new(pool.clone());
        let (raw, g) = carts.create().await.unwrap();
        let mut d = details();
        d.shipping_company = "Existing company".into();
        carts.save_details(&g, &g.csrf, 0, d.clone()).await.unwrap();
        customer_fields::save(
            &pool,
            &s,
            &s.csrf,
            0,
            &choices(&[
                ("private.shipping_company", "hidden"),
                ("business.recipient_code", "required"),
                ("business.pec", "required"),
                ("private.delivery_notes", "required"),
            ]),
        )
        .await
        .unwrap();
        let mut forged = d.clone();
        forged.shipping_company = "Overwrite hidden".into();
        forged.delivery_notes = "Leave at reception".into();
        carts.save_details(&g, &g.csrf, 1, forged).await.unwrap();
        let saved = carts.details(&g).await.unwrap().unwrap();
        assert_eq!(saved.shipping_company, "Existing company");
        let policy = customer_fields::load(&pool).await.unwrap();
        assert!(
            saved
                .clone()
                .validate_with(&policy)
                .unwrap()
                .shipping_company
                .is_empty()
        );
        let mut missing = details();
        missing.customer_type = "business".into();
        missing.billing_company = "Business".into();
        missing.vat_id = "12345678903".into();
        let errors = missing.clone().validate_with(&policy).err().unwrap();
        assert!(errors.contains(&"recipient_code") && errors.contains(&"pec"));
        missing.recipient_code = "ABC1234".into();
        missing.pec = "invoice@example.test".into();
        let normalized = missing.validate_with(&policy).unwrap();
        assert_eq!(normalized.invoice_channel, "both");
        carts
            .save_details(&g, &g.csrf, 2, normalized)
            .await
            .unwrap();
        let saved = carts.details(&g).await.unwrap().unwrap();
        assert_eq!(saved.recipient_code, "ABC1234");
        assert_eq!(saved.pec, "invoice@example.test");
        // A draft that used to be complete must now return to the details form.
        let (_, old) = carts.create().await.unwrap();
        customer_fields::save(&pool, &s, &s.csrf, 1, &choices(&[]))
            .await
            .unwrap();
        carts.save_details(&old, &old.csrf, 0, d).await.unwrap();
        customer_fields::save(
            &pool,
            &s,
            &s.csrf,
            2,
            &choices(&[("private.delivery_notes", "required")]),
        )
        .await
        .unwrap();
        assert!(
            carts
                .checkout_step(
                    &old,
                    talocart_platform::content::Language::new(
                        talocart_platform::locale::Locale::ENGLISH,
                        talocart_platform::locale::Locale::ENGLISH
                    ),
                    None
                )
                .await
                .unwrap()
                .is_none()
        );
        let app = router(test_app(pool, "Fields".into()).await);
        let r = request(
            &app,
            "GET",
            "/checkout",
            &format!("talocart_cart={raw}"),
            "",
            false,
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_fields_account_and_address_hidden_data_survive_save_and_reenable() {
    isolated(|pool, _| async move {
        let owner = owner(&pool).await;
        let identity = talocart_platform::identity::Identity::new(pool.clone());
        let customers = Customers::new(pool.clone(), identity);
        let (_, anon) = customers.anonymous().await.unwrap();
        let mut p = CustomerProfile {
            email: "account@example.test".into(),
            first_name: "Existing".into(),
            last_name: "Person".into(),
            ..Default::default()
        };
        let raw = customers
            .register(
                &anon,
                &anon.csrf,
                &p,
                "synthetic customer password".into(),
                None,
            )
            .await
            .unwrap()
            .expect("Immediate activation");
        let c = customers.session(&raw).await.unwrap().unwrap();
        let addresses = CustomerAddresses::new(pool.clone());
        let a = AddressInput {
            label: "Home".into(),
            first_name: "Recipient".into(),
            last_name: "Example".into(),
            company: "Saved company".into(),
            phone: "+39 3331234567".into(),
            country: "IT".into(),
            street: "Via Example".into(),
            number: "1".into(),
            postal: "00100".into(),
            city: "Roma".into(),
            region: "RM".into(),
            ..Default::default()
        };
        let id = addresses.save(&c, &c.csrf, None, &a).await.unwrap();
        customer_fields::save(
            &pool,
            &owner,
            &owner.csrf,
            0,
            &choices(&[
                ("private.first_name", "hidden"),
                ("private.last_name", "optional"),
                ("private.shipping_company", "hidden"),
            ]),
        )
        .await
        .unwrap();
        p.first_name = "Forged".into();
        p.last_name.clear();
        customers
            .update(&c, &c.csrf, 0, &p, String::new())
            .await
            .unwrap();
        let current = customers.account(&c).await.unwrap();
        assert_eq!(current.first_name, "Existing");
        assert_eq!(current.last_name, "");
        let mut edit = a.clone();
        edit.company = "Forged".into();
        addresses
            .save(&c, &c.csrf, Some((id, 0)), &edit)
            .await
            .unwrap();
        assert_eq!(
            addresses.get(&c, id).await.unwrap().company,
            "Saved company"
        );
        customer_fields::save(
            &pool,
            &owner,
            &owner.csrf,
            1,
            &choices(&[
                ("business.first_name", "optional"),
                ("business.last_name", "optional"),
            ]),
        )
        .await
        .unwrap();
        p.customer_type = "business".into();
        p.first_name.clear();
        customers
            .update(&c, &c.csrf, 1, &p, String::new())
            .await
            .unwrap();
        assert_eq!(
            customers.account(&c).await.unwrap().customer_type,
            "business"
        );
        let (_, anon) = customers.anonymous().await.unwrap();
        p.email = "new-business@example.test".into();
        assert!(
            customers
                .register(
                    &anon,
                    &anon.csrf,
                    &p,
                    "synthetic customer password".into(),
                    None
                )
                .await
                .is_ok()
        );
        sqlx::query("INSERT INTO checkout_countries(country_code) VALUES('IT')")
            .execute(&pool)
            .await
            .unwrap();
        let carts = Carts::new(pool.clone());
        let (_, guest) = carts.create().await.unwrap();
        addresses
            .use_in_checkout(&c, &c.csrf, id, (&guest, &guest.csrf, 0))
            .await
            .unwrap();
        let draft = carts.details(&guest).await.unwrap().unwrap();
        assert_eq!(draft.customer_type, "business");
        assert!(draft.invoice_requested);
        assert!(
            draft
                .validate_with(&customer_fields::load(&pool).await.unwrap())
                .is_err()
        );
        let mut invalid = p;
        invalid.customer_type = "invalid".into();
        invalid.email = customers.account(&c).await.unwrap().email;
        assert!(matches!(
            customers
                .update(&c, &c.csrf, 2, &invalid, String::new())
                .await,
            Err(CustomerError::Fields(_))
        ));
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_fields_nameless_accounts_keep_accessible_admin_titles() {
    isolated(|pool, _| async move {
        let session = owner(&pool).await;
        let identity = Identity::new(pool.clone());
        customer_fields::save(
            &pool,
            &session,
            &session.csrf,
            0,
            &choices(&[
                ("private.first_name", "optional"),
                ("private.last_name", "optional"),
            ]),
        )
        .await
        .unwrap();
        let customers = Customers::new(pool.clone(), identity.clone());
        let (_, anon) = customers.anonymous().await.unwrap();
        let raw = customers
            .register(
                &anon,
                &anon.csrf,
                &CustomerProfile {
                    email: "nameless@example.test".into(),
                    ..Default::default()
                },
                "synthetic customer password".into(),
                None,
            )
            .await
            .unwrap()
            .expect("Immediate activation");
        let c = customers.session(&raw).await.unwrap().unwrap();
        let id = customers.account(&c).await.unwrap().id;
        let admin = authenticated(&identity).await;
        let cookie = format!("talocart_session={admin}");
        let app = router(test_app(pool.clone(), "Fields".into()).await);
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let h = html(
                request(
                    &app,
                    "GET",
                    &format!("/admin/customers/{id}"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(
                h.contains("<h1>nameless@example.test</h1>"),
                "Customer title must use email when both names are absent"
            );
            let h = html(request(&app, "GET", "/admin/customers", &cookie, "", false).await).await;
            assert!(
                h.contains(&format!(
                    "href=\"/admin/customers/{id}\">nameless@example.test</a>"
                )),
                "Customer link must have an accessible name"
            );
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn customer_fields_independent_billing_reveals_required_fields_missing_from_shipping() {
    isolated(|pool,_|async move {
        let owner=owner(&pool).await;
        customer_fields::save(&pool,&owner,&owner.csrf,0,&choices(&[("private.shipping_street","hidden")])).await.unwrap();
        sqlx::query("INSERT INTO checkout_countries(country_code) VALUES('IT')").execute(&pool).await.unwrap();
        let(raw,g)=Carts::new(pool.clone()).create().await.unwrap();let cookie=format!("talocart_cart={raw}");let app=router(test_app(pool,"Fields".into()).await);
        let body=format!("csrf={}&revision=0&customer_type=private&email=buyer%40example.test&first_name=Alex&last_name=Example&phone=%2B39+3331234567&shipping_country=IT&shipping_number=1&shipping_postal=00100&shipping_city=Roma&shipping_region=RM&billing_same=true&invoice_requested=true&tax_id=RSSMRA80A01H501U",g.csrf);
        let response=request(&app,"POST","/checkout",&cookie,&body,false).await;assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let h=html(response).await;
        let checkbox=h.split("id=\"billing_same\"").nth(1).unwrap().split('>').next().unwrap();
        assert!(!checkbox.contains(" checked"),"A missing required billing field must be editable when shipping hides it");
        assert!(h.contains("err-billing_street"));
    }).await;
}
