use super::*;
use talocart_commerce::{
    customer_addresses::{AddressInput, CustomerAddresses},
    customer_billing::{BillingInput, CustomerBilling},
};
use talocart_platform::identity::{
    CustomerError, CustomerProfile, Customers, Permission, customer_admin::CustomerAdmin,
};
async fn fixture(
    pool: &PgPool,
) -> (
    Identity,
    talocart_platform::identity::Session,
    String,
    i64,
    String,
) {
    let identity = installed(pool).await;
    let raw = authenticated(&identity).await;
    let owner = identity.session(&raw).await.unwrap().unwrap();
    let customers = Customers::new(pool.clone(), identity.clone());
    let (_, anon) = customers.anonymous().await.unwrap();
    let buyer = customers
        .register(
            &anon,
            &anon.csrf,
            &CustomerProfile {
                email: "buyer@example.test".into(),
                first_name: "Alex".into(),
                last_name: "Buyer".into(),
                ..CustomerProfile::default()
            },
            "synthetic customer password".into(),
            None,
        )
        .await
        .unwrap()
        .expect("Immediate activation");
    let id = customers
        .session(&buyer)
        .await
        .unwrap()
        .unwrap()
        .customer_id()
        .unwrap();
    (identity, owner, raw, id, buyer)
}
fn billing_input() -> BillingInput {
    BillingInput {
        billing_first_name: "Alex".into(),
        billing_last_name: "Buyer".into(),
        billing_company: "Example Srl".into(),
        billing_country: "IT".into(),
        billing_street: "Via Example".into(),
        billing_number: "1".into(),
        billing_city: "Roma".into(),
        billing_postal: "00100".into(),
        billing_region: "RM".into(),
        vat_id: "12345678903".into(),
        tax_id: "RSSMRA80A01H501U".into(),
        recipient_code: "ABC1234".into(),
        pec: "invoice@example.test".into(),
        invoice_requested: true,
        ..BillingInput::default()
    }
}
fn address() -> AddressInput {
    AddressInput {
        label: "Office".into(),
        first_name: "Alex".into(),
        last_name: "Buyer".into(),
        phone: "+39 3331234567".into(),
        country: "IT".into(),
        street: "Via Example".into(),
        number: "1".into(),
        postal: "00100".into(),
        city: "Roma".into(),
        region: "RM".into(),
        ..AddressInput::default()
    }
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_profile_revisions_uniqueness_and_access_revocation() {
    isolated(|pool, _| async move {
        let (identity, owner, _, id, buyer) = fixture(&pool).await;
        let admin = CustomerAdmin::new(pool.clone(), identity.clone());
        let customers = Customers::new(pool.clone(), identity);
        let other:i64=sqlx::query_scalar("INSERT INTO customers(email,password_hash,first_name,last_name) SELECT 'other@example.test',password_hash,'Other','Buyer' FROM customers WHERE id=$1 RETURNING id").bind(id).fetch_one(&pool).await.unwrap();
        let before = admin.get(&owner, id).await.unwrap();
        assert!(before.active);
        assert!(before.phone.is_empty());
        assert!(!before.created_at.is_empty());
        let mut input = before.profile();
        input.email = "Changed@Example.test".into();
        input.phone = "+39 3331234567".into();
        input.customer_type = "business".into();
        admin
            .save(&owner, &owner.csrf, id, 0, &input)
            .await
            .unwrap();
        assert!(customers.session(&buyer).await.unwrap().is_none());
        let current = admin.get(&owner, id).await.unwrap();
        assert_eq!(current.email, "changed@example.test");
        assert_eq!(current.phone, input.phone);
        assert_eq!(current.customer_type, "business");
        assert!(matches!(
            admin.save(&owner, &owner.csrf, id, 0, &input).await,
            Err(CustomerError::Stale)
        ));
        let mut duplicate=input.clone();duplicate.email="other@example.test".into();
        assert!(matches!(admin.save(&owner,&owner.csrf,id,1,&duplicate).await,Err(CustomerError::Duplicate)));
        assert_ne!(other,id);
        input.first_name = "Different".into();
        let a = admin.save(&owner, &owner.csrf, id, 1, &input);
        let b = admin.save(&owner, &owner.csrf, id, 1, &input);
        let (a, b) = tokio::join!(a, b);
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let (_, anon) = customers.anonymous().await.unwrap();
        let raw = customers
            .login(
                &anon,
                &anon.csrf,
                "changed@example.test",
                "synthetic customer password".into(),
            )
            .await
            .unwrap();
        let mut input = admin.get(&owner, id).await.unwrap().profile();
        input.active = false;
        admin
            .save(&owner, &owner.csrf, id, 2, &input)
            .await
            .unwrap();
        assert!(customers.session(&raw).await.unwrap().is_none());
        assert!(matches!(
            admin.access(&owner, &owner.csrf, id, 3, true).await,
            Err(CustomerError::Fields(_))
        ));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_recovery_is_single_use_revokes_previous_and_never_appears_in_get() {
    isolated(|pool, _| async move {
        let (identity, owner, raw, id, buyer) = fixture(&pool).await;
        let admin = CustomerAdmin::new(pool.clone(), identity.clone());
        let customers = Customers::new(pool.clone(), identity);
        let token = admin
            .access(&owner, &owner.csrf, id, 0, true)
            .await
            .unwrap()
            .unwrap();
        assert!(customers.session(&buyer).await.unwrap().is_none());
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT token_hash FROM customer_password_resets WHERE customer_id=$1"
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            digest(&token)
        );
        assert!(matches!(
            admin.access(&owner, &owner.csrf, id, 0, true).await,
            Err(CustomerError::Stale)
        ));
        let (_, anon) = customers.anonymous().await.unwrap();
        customers
            .reset(&anon, &anon.csrf, &token, "new synthetic password".into())
            .await
            .unwrap();
        let (_, anon) = customers.anonymous().await.unwrap();
        assert!(
            customers
                .reset(&anon, &anon.csrf, &token, "new synthetic password".into())
                .await
                .is_err()
        );
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let endpoint = format!("/admin/customers/{id}");
        let cookie = format!("talocart_session={raw}");
        let revision = admin.get(&owner, id).await.unwrap().revision;
        let response = request(
            &app,
            "POST",
            &format!("{endpoint}/access"),
            &cookie,
            &format!(
                "csrf={}&revision={revision}&action=recovery&confirm=true",
                owner.csrf
            ),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = html(response).await;
        assert!(body.contains("id=\"ca-recovery-token\""));
        let response = request(&app, "GET", &endpoint, &cookie, "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!html(response).await.contains("id=\"ca-recovery-token\""));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_billing_revisions_validation_prefill_and_data_preservation() {
    isolated(|pool,_|async move {
  let(identity,owner,_,id,buyer)=fixture(&pool).await;let service=CustomerBilling::new(pool.clone(),identity.clone());let customers=Customers::new(pool.clone(),identity.clone());let session=customers.session(&buyer).await.unwrap().unwrap();
  assert!(service.get(&owner,id).await.unwrap().is_none());let mut input=billing_input();service.save(&owner,&owner.csrf,id,-1,&input).await.unwrap();
  let stored=service.get(&owner,id).await.unwrap().unwrap();assert_eq!(stored.input.billing_company,"Example Srl");let initial=service.initial(&session).await.unwrap();assert_eq!(initial.email,"buyer@example.test");assert_eq!(initial.pec,input.pec);assert!(!initial.billing_same);
  assert!(matches!(service.save(&owner,&owner.csrf,id,-1,&input).await,Err(CustomerError::Stale)));
  input.billing_postal="bad".into();assert!(matches!(service.save(&owner,&owner.csrf,id,stored.revision,&input).await,Err(CustomerError::Fields(f)) if f.contains(&"billing_postal")));
  input=billing_input();input.invoice_requested=false;service.save(&owner,&owner.csrf,id,stored.revision,&input).await.unwrap();let stored=service.get(&owner,id).await.unwrap().unwrap();assert!(!stored.input.invoice_requested);assert_eq!(stored.input.pec,"invoice@example.test");
  let a=service.save(&owner,&owner.csrf,id,stored.revision,&input);let b=service.save(&owner,&owner.csrf,id,stored.revision,&input);let(a,b)=tokio::join!(a,b);assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
  let current=service.get(&owner,id).await.unwrap().unwrap();
  service.delete(&owner,&owner.csrf,id,current.revision).await.unwrap();
  service.save(&owner,&owner.csrf,id,-1,&input).await.unwrap();
  assert!(matches!(service.save(&owner,&owner.csrf,id,current.revision,&input).await,Err(CustomerError::Stale)),"Removed and recreated billing cannot reuse an old revision");
  assert!(sqlx::query("UPDATE customer_billing SET revision=revision WHERE customer_id=$1").bind(id).execute(&pool).await.is_err(),"Database rejects missing revision increments");
  let profile=CustomerAdmin::new(pool.clone(),identity).get(&owner,id).await.unwrap();assert_eq!(profile.revision,0,"Separate sections do not invalidate the profile revision");
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_addresses_enforce_owner_revisions_limits_and_shared_visibility() {
    isolated(|pool, _| async move {
        let (identity, owner, _, id, buyer) = fixture(&pool).await;
        let service = CustomerAddresses::new(pool.clone());
        let customers = Customers::new(pool.clone(), identity.clone());
        let session = customers.session(&buyer).await.unwrap().unwrap();
        let other:i64=sqlx::query_scalar("INSERT INTO customers(email,password_hash,first_name,last_name) SELECT 'address-other@example.test',password_hash,'Other','Buyer' FROM customers WHERE id=$1 RETURNING id").bind(id).fetch_one(&pool).await.unwrap();
        let address = address();
        let address_id = service
            .admin_save(&owner, &owner.csrf, id, None, &address)
            .await
            .unwrap();
        assert_eq!(service.list(&session).await.unwrap()[0].id, address_id);
        assert!(matches!(
            service
                .admin_save(
                    &owner,
                    &owner.csrf,
                    other,
                    Some((address_id, 0)),
                    &address
                )
                .await,
            Err(CustomerError::NotFound)
        ));
        let mut changed = address.clone();
        changed.city = "Milano".into();
        service
            .admin_save(&owner, &owner.csrf, id, Some((address_id, 0)), &changed)
            .await
            .unwrap();
        assert_eq!(service.list(&session).await.unwrap()[0].city, "Milano");
        assert!(matches!(
            service
                .admin_delete(&owner, &owner.csrf, id, address_id, 0)
                .await,
            Err(CustomerError::Stale)
        ));
        for _ in 1..20 {
            service
                .admin_save(&owner, &owner.csrf, id, None, &address)
                .await
                .unwrap();
        }
        assert!(matches!(
            service
                .admin_save(&owner, &owner.csrf, id, None, &address)
                .await,
            Err(CustomerError::AddressLimit)
        ));
        service
            .admin_delete(&owner, &owner.csrf, id, address_id, 1)
            .await
            .unwrap();
        assert_eq!(service.list(&session).await.unwrap().len(), 19);
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_http_bilingual_validation_csrf_and_unknown_fields() {
    isolated(|pool,_|async move {
  let(identity,owner,raw,id,_)=fixture(&pool).await;let app=router(test_app(pool.clone(),"Shop".into()).await);let cookie=format!("talocart_session={raw}");let path=format!("/admin/customers/{id}");
  for(locale,title) in [("en","Customer details"),("it","Scheda cliente")] {
   identity.update_locale(&owner,&owner.csrf,locale).await.unwrap();let response=request(&app,"GET",&path,&cookie,"",false).await;assert_eq!(response.status(),StatusCode::OK);assert_eq!(response.headers()[header::CONTENT_LANGUAGE],locale);let body=html(response).await;
   for selector in ["customer-profile-form","customer-billing-form","customer-group","customer-access-form","customer-orders"]{assert!(body.contains(selector),"Missing {selector}");}assert!(body.contains(title));assert!(!body.contains("id=\"customer-group-form\""));
   for suffix in ["profile","billing","access","addresses/new","billing/delete"] {assert_eq!(request(&app,"POST",&format!("{path}/{suffix}"),&cookie,"csrf=wrong&revision=0",false).await.status(),StatusCode::FORBIDDEN);}
   let data=format!("csrf={}&revision=0&email=bad&first_name=%3Cscript%3E&last_name=Buyer&customer_type=private&active=true",owner.csrf);
   let response=request(&app,"POST",&format!("{path}/profile"),&cookie,&data,false).await;assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let body=html(response).await;assert!(body.contains("ca-email-error"));assert!(body.contains("&#60;script&#62;"));assert!(!body.contains("<script>"));
   assert_eq!(request(&app,"POST",&format!("{path}/profile"),&cookie,&format!("csrf={}&revision=0&email=buyer%40example.test&first_name=Alex&last_name=Buyer&customer_type=private&active=true&unexpected=value",owner.csrf),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
   assert_eq!(request(&app,"GET",&format!("{path}/access"),&cookie,"",false).await.status(),StatusCode::METHOD_NOT_ALLOWED);
  }
 }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_permissions_are_rechecked_for_every_section() {
    isolated(|pool, _| async move {
        let (identity, owner, _, id, _) = fixture(&pool).await;
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Customer reader",
                &[Permission::CustomersRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "reader@example.test",
                "correct horse battery".into(),
                talocart_platform::identity::Role::Custom(role.clone()),
            )
            .await
            .unwrap();
        let raw = login_as(&identity, "reader@example.test").await;
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let admin = CustomerAdmin::new(pool.clone(), identity.clone());
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_session={raw}");
        let path = format!("/admin/customers/{id}");
        let response = request(&app, "GET", &path, &cookie, "", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = html(response).await;
        assert!(body.contains("<fieldset class=\"field-stack\" disabled"));
        assert!(!body.contains("id=\"customer-orders\""));
        assert!(!body.contains("id=\"customer-access-form\""));
        let input = admin.get(&reader, id).await.unwrap().profile();
        assert!(
            !html(request(&app, "GET", "/admin/customers", &cookie, "", false).await)
                .await
                .contains("href=\"/admin/customers/new\"")
        );
        assert_eq!(
            request(&app, "GET", "/admin/customers/new", &cookie, "", false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/customers/new",
                &cookie,
                &format!("csrf={}", reader.csrf),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert!(matches!(
            admin
                .create(
                    &reader,
                    &reader.csrf,
                    &input,
                    "synthetic initial password".into()
                )
                .await,
            Err(CustomerError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));

        assert!(matches!(
            admin.save(&reader, &reader.csrf, id, 0, &input).await,
            Err(CustomerError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert!(matches!(
            admin.access(&reader, &reader.csrf, id, 0, true).await,
            Err(CustomerError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        let billing = CustomerBilling::new(pool.clone(), identity.clone());
        assert!(matches!(
            billing
                .save(&reader, &reader.csrf, id, -1, &billing_input())
                .await,
            Err(CustomerError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        let addresses = CustomerAddresses::new(pool.clone());
        assert!(matches!(
            addresses
                .admin_save(&reader, &reader.csrf, id, None, &address())
                .await,
            Err(CustomerError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        for suffix in [
            "profile",
            "billing",
            "billing/delete",
            "access",
            "addresses/new",
        ] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &format!("{path}/{suffix}"),
                    &cookie,
                    &format!("csrf={}&revision=0", reader.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        sqlx::query("DELETE FROM admin_role_permissions WHERE role_key=$1")
            .bind(role.as_str())
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            admin.get(&reader, id).await,
            Err(CustomerError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert_eq!(
            request(&app, "GET", &path, &cookie, "", false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_profile_and_group_are_saved_atomically() {
    isolated(|pool, _| async move {
        let (identity, owner, raw, id, _) = fixture(&pool).await;
        let admin = CustomerAdmin::new(pool.clone(), identity);
        let original = admin.get(&owner, id).await.unwrap();
        let group: i64 = sqlx::query_scalar("INSERT INTO customer_groups(name_en,name_it) VALUES('Trade','Rivenditori') RETURNING id")
            .fetch_one(&pool).await.unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_session={raw}");
        let endpoint = format!("/admin/customers/{id}/profile");
        let form = |group: i64, revision: i64| format!("csrf={}&revision={revision}&group_id={group}&first_name=Updated&last_name=Buyer&email=buyer%40example.test&customer_type=private&active=true", owner.csrf);
        let malformed = form(group, 0).replace(&format!("group_id={group}"), "group_id=bad");
        let response = request(&app, "POST", &endpoint, &cookie, &malformed, false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(html(response).await.contains("id=\"customer-group-error\""));
        let response = request(&app,"POST",&endpoint,&cookie,&form(i64::MAX,0),false).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        let body=html(response).await;
        assert!(body.contains("id=\"customer-group-error\""));
        let unchanged=admin.get(&owner,id).await.unwrap();
        assert_eq!(unchanged.first_name,original.first_name);
        assert_eq!(unchanged.group_id,original.group_id);
        assert_eq!(unchanged.revision,0);
        let response=request(&app,"POST",&endpoint,&cookie,&form(group,0),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        let updated=admin.get(&owner,id).await.unwrap();
        assert_eq!(updated.first_name,"Updated");
        assert_eq!(updated.group_id,group);
        assert_eq!(updated.revision,1);
        let response=request(&app,"POST",&endpoint,&cookie,&form(original.group_id,0),false).await;
        assert_eq!(response.status(),StatusCode::CONFLICT);
        assert_eq!(admin.get(&owner,id).await.unwrap().group_id,group);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_creation_form_and_account_login() {
    isolated(|pool, _| async move {
        let (identity, owner, raw, _, _) = fixture(&pool).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_session={raw}");
        for (locale, title) in [("en", "New customer"), ("it", "Nuovo cliente")] {
            identity
                .update_locale(&owner, &owner.csrf, locale)
                .await
                .unwrap();
            let response = request(&app, "GET", "/admin/customers/new", &cookie, "", false).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "Creation form must be reachable"
            );
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(response.headers()[header::CONTENT_LANGUAGE], locale);
            assert!(html(response).await.contains(title));
            assert!(
                html(request(&app, "GET", "/admin/customers", &cookie, "", false).await)
                    .await
                    .contains("href=\"/admin/customers/new\"")
            );
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_creation_validates_preserves_drafts_and_creates_login() {
    isolated(|pool, _| async move {
        let (identity, owner, raw, _, _) = fixture(&pool).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_session={raw}");
        let group: i64 = sqlx::query_scalar("INSERT INTO customer_groups(name_it,name_en) VALUES('Backend','Backend') RETURNING id").fetch_one(&pool).await.unwrap();
        let path = "/admin/customers/new";
        let secret = "synthetic initial password";
        for locale in ["en", "it"] {
            identity.update_locale(&owner, &owner.csrf, locale).await.unwrap();
            let data = format!("csrf={}&email=NEW-{locale}%40Example.test&first_name=New&last_name=Buyer&customer_type=private&phone=%2B39+3331234567&group_id={group}&active=true&password={secret}&password_confirmation={secret}", owner.csrf);
            assert_eq!(request(&app,"POST",path,&cookie,&data.replace(&owner.csrf,"wrong"),false).await.status(),StatusCode::FORBIDDEN);
            let before:i64=sqlx::query_scalar("SELECT count(*) FROM customers").fetch_one(&pool).await.unwrap();
            for (data, field) in [
                (data.replace("password_confirmation=synthetic initial password", "password_confirmation=does not match"), "password_confirmation"),
                (data.replace(secret,"short"),"password"),
                (data.replace(&format!("NEW-{locale}%40Example.test"),"bad"),"email"),
                (data.replace("phone=%2B39+3331234567","phone=bad"),"phone"),
                (data.replace(&format!("group_id={group}"),"group_id=99999999"),"group"),
                (data.replace(&format!("group_id={group}"),"group_id=invalid"),"group"),
                (data.replace("customer_type=private","customer_type=invalid"),"customer-type"),
            ] {
                let response=request(&app,"POST",path,&cookie,&data,false).await;
                assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY,"{field}");
                assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");
                let body=html(response).await;
                assert!(body.contains(&format!("{field}-error")),"Missing error: {field}");
                assert!(body.contains("value=\"New\""));
                assert!(!body.contains(secret));
                assert!(!body.contains("does not match"));
            }
            for invalid in [format!("{data}&unexpected=value"),format!("{data}&email=other%40example.test"),format!("{data}&revision=0"),format!("{data}&action=invalid")] {
                assert_eq!(request(&app,"POST",path,&cookie,&invalid,false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
            }
            let response=request(&app,"POST",path,&cookie,&format!("{data}&action=refresh"),false).await;
            assert_eq!(response.status(),StatusCode::OK);
            assert!(!html(response).await.contains(secret));
            assert_eq!(before,sqlx::query_scalar::<_,i64>("SELECT count(*) FROM customers").fetch_one(&pool).await.unwrap());
            let response=request(&app,"POST",path,&cookie,&data,false).await;
            assert_eq!(response.status(),StatusCode::SEE_OTHER);
            assert!(response.headers().get(header::SET_COOKIE).is_none(),"Admin creation must not sign in as the customer");
            let id:i64=sqlx::query_scalar("SELECT id FROM customers WHERE email=$1").bind(format!("new-{locale}@example.test")).fetch_one(&pool).await.unwrap();
            assert_eq!(response.headers()[header::LOCATION],format!("/admin/customers/{id}?saved=1#customer-profile"));
            let member=CustomerAdmin::new(pool.clone(),identity.clone()).get(&owner,id).await.unwrap();
            assert_eq!(member.group_id,group);assert_eq!(member.phone,"+39 3331234567");assert_eq!(member.revision,0);
            assert_eq!(request(&app,"POST",path,&cookie,&data,false).await.status(),StatusCode::CONFLICT);
            let customers=Customers::new(pool.clone(),identity.clone());
            let(_,anon)=customers.anonymous().await.unwrap();
            let raw=customers.login(&anon,&anon.csrf,&member.email,secret.into()).await.unwrap();
            assert_eq!(customers.session(&raw).await.unwrap().unwrap().customer_id(),Some(id));
        }
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_creation_is_atomic_for_duplicates_groups_and_inactive_accounts() {
    isolated(|pool, _| async move {
        let (identity, owner, _, id, _) = fixture(&pool).await;
        let admin=CustomerAdmin::new(pool.clone(),identity.clone());
        let mut input=admin.get(&owner,id).await.unwrap().profile();
        input.email="new@example.test".into();input.group_id=None;input.active=false;
        let a=admin.create(&owner,&owner.csrf,&input,"synthetic initial password".into());
        let b=admin.create(&owner,&owner.csrf,&input,"synthetic initial password".into());
        let(a,b)=tokio::join!(a,b);
        assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        assert!(matches!((&a,&b),(Ok(_),Err(CustomerError::Duplicate))|(Err(CustomerError::Duplicate),Ok(_))));
        let created= a.or(b).unwrap();
        let member=admin.get(&owner,created).await.unwrap();assert!(!member.active);
        assert_eq!(member.group_id,sqlx::query_scalar::<_,i64>("SELECT default_group_id FROM customer_group_configuration").fetch_one(&pool).await.unwrap());
        let customers=Customers::new(pool.clone(),identity.clone());let(_,anon)=customers.anonymous().await.unwrap();
        assert!(customers.login(&anon,&anon.csrf,"new@example.test","synthetic initial password".into()).await.is_err());
        input.email="missing-group@example.test".into();input.group_id=Some(99999999);
        assert!(matches!(admin.create(&owner,&owner.csrf,&input,"synthetic initial password".into()).await,Err(CustomerError::Fields(f)) if f.contains(&"group_id")));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM customers WHERE email='missing-group@example.test'").fetch_one(&pool).await.unwrap(),0);
        assert!(matches!(admin.create(&owner,"wrong",&input,"synthetic initial password".into()).await,Err(CustomerError::Access(talocart_platform::technical::AccessError::Forbidden))));
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_table_filters_sorting_pagination_and_localized_http() {
    use talocart_platform::identity::customer_groups::{CustomerGroups, MemberFilters};
    isolated(|pool, _| async move {
        let (identity, owner, raw, id, _) = fixture(&pool).await;
        let service = CustomerGroups::new(pool.clone(), identity.clone());
        let group: i64 = sqlx::query_scalar("INSERT INTO customer_groups(name_it,name_en) VALUES('AAA aziende','ZZZ businesses') RETURNING id").fetch_one(&pool).await.unwrap();
        for n in 0..23 {
            sqlx::query("INSERT INTO customers(email,password_hash,first_name,last_name,group_id,customer_type,active) SELECT $1,password_hash,$2,'Table',$3,'business',false FROM customers WHERE id=$4")
                .bind(format!("table{n:02}@example.test")).bind(format!("Customer {n:02}"))
                .bind(group).bind(id).execute(&pool).await.unwrap();
        }
        sqlx::query("UPDATE customers SET first_name='Literal %_\\',last_name='<Example>' WHERE id=$1").bind(id).execute(&pool).await.unwrap();
        let filters = MemberFilters { name: "customer".into(), email: "TABLE".into(), group: group.to_string(), kind: "business".into(), state: "inactive".into(), sort: "name".into(), dir: "asc".into(), per_page: "10".into() };
        let result = service.filtered_members(&owner,&filters,1,"en").await.unwrap();
        assert_eq!(result.total,23);assert_eq!(result.entries.len(),10);assert_eq!(result.entries[0].first_name,"Customer 00");
        let last = service.filtered_members(&owner,&filters,999,"it").await.unwrap();
        assert_eq!(last.page,3);assert_eq!(last.entries.len(),3);assert_eq!(last.entries[0].first_name,"Customer 20");
        let descending=MemberFilters { dir:"desc".into(), ..filters.clone() };
        assert_eq!(service.filtered_members(&owner,&descending,1,"en").await.unwrap().entries[0].first_name,"Customer 22");
        for column in ["email","group","kind","state"] {
            let f = MemberFilters { sort:column.into(),..filters.clone() };
            let a=service.filtered_members(&owner,&f,1,"en").await.unwrap();
            let b=service.filtered_members(&owner,&MemberFilters{dir:"desc".into(),..f},1,"en").await.unwrap();
            assert_ne!(a.entries[0].id,b.entries[0].id);
        }
        let locale_sort=MemberFilters{sort:"group".into(),..MemberFilters::default()};
        assert_eq!(service.filtered_members(&owner,&locale_sort,1,"it").await.unwrap().entries[0].group_id,group);
        assert_ne!(service.filtered_members(&owner,&locale_sort,1,"en").await.unwrap().entries[0].group_id,group);
        for name in ["%", "_", "\\"] {
            let f=MemberFilters{name:name.into(),..MemberFilters::default()};
            let found=service.filtered_members(&owner,&f,1,"en").await.unwrap();
            assert_eq!(found.total,1);assert_eq!(found.entries[0].id,id);
        }
        let empty=MemberFilters{state:"active".into(),..filters.clone()};
        assert_eq!(service.filtered_members(&owner,&empty,1,"en").await.unwrap().total,0);
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let cookie=format!("talocart_session={raw}");
        for locale in ["en","it"] {
            identity.update_locale(&owner,&owner.csrf,locale).await.unwrap();
            let response=request(&app,"GET","/admin/customers?name=customer&sort=email&dir=desc&per_page=10",&cookie,"",false).await;
            assert_eq!(response.status(),StatusCode::OK);
            assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");
            let body=html(response).await;
            assert!(body.contains("ui-table ui-table--fixed customer-table"));assert!(body.contains("aria-sort=\"descending\""));
            assert!(body.contains("name=customer&#38;email="));assert!(body.contains("&#38;page=2"));
            for query in ["sort=bad","dir=bad","kind=other","state=other","group=-1","per_page=500","page=0","name=line%0Abreak"] {
                let response=request(&app,"GET",&format!("/admin/customers?{query}"),&cookie,"",false).await;
                assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY,"{query}");
                let body=html(response).await;assert!(body.contains("customer-filter-error"));assert!(!body.contains("class=\"shared-pagination\""));
                assert!(body.contains(if locale=="it" {"Controlla i filtri clienti"} else {"Check customer filters"}));
            }
            let body=html(request(&app,"GET","/admin/customers?name=Literal",&cookie,"",false).await).await;
            assert!(body.contains("&#60;Example&#62;"));assert!(!body.contains("<Example>"));
        }
        sqlx::query("UPDATE customers SET first_name='',last_name='',email='zzz@example.test' WHERE id=$1").bind(id).execute(&pool).await.unwrap();
        let unnamed=MemberFilters{per_page:"100".into(),..MemberFilters::default()};
        assert_eq!(service.filtered_members(&owner,&unnamed,1,"en").await.unwrap().entries.last().unwrap().id,id);
        let unnamed=MemberFilters{name:"zzz@".into(),..unnamed};
        assert_eq!(service.filtered_members(&owner,&unnamed,1,"en").await.unwrap().total,1);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM customers").fetch_one(&pool).await.unwrap(),24);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_customer_and_order_page_sizes_persist_independently() {
    isolated(|pool, _| async move {
        let (identity, owner, raw, _, _) = fixture(&pool).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let session = format!("talocart_session={raw}");
        for locale in ["en", "it"] {
            identity.update_locale(&owner, &owner.csrf, locale).await.unwrap();
            for (route, name, other) in [
                ("/admin/customers", "talocart_admin_customer_page_size", "talocart_admin_order_page_size"),
                ("/admin/orders", "talocart_admin_order_page_size", "talocart_admin_customer_page_size"),
            ] {
                let response=request(&app,"GET",&format!("{route}?per_page=50"),&session,"",false).await;
                assert_eq!(response.status(),StatusCode::OK);
                let saved=response.headers().get_all(header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).find(|v| v.starts_with(&format!("{name}="))).expect("explicit page size must persist");
                assert!(saved.contains("Path=/admin") && saved.contains("HttpOnly") && saved.contains("SameSite=Lax") && saved.contains("Max-Age=15552000"));
                let cookie=format!("{session}; {}; {other}=100; talocart_admin_product_page_size=500; talocart_catalog_page_size=84",saved.split(';').next().unwrap());
                for suffix in ["", "?sort=email", "?per_page="] {
                    // Customer/email and order/reference are distinct allowed sort keys.
                    let suffix=if route.ends_with("orders") && suffix=="?sort=email" {"?sort=reference"} else {suffix};
                    let response=request(&app,"GET",&format!("{route}{suffix}"),&cookie,"",false).await;
                    assert_eq!(response.status(),StatusCode::OK);
                    assert!(!response.headers().get_all(header::SET_COOKIE).iter().any(|v| v.to_str().unwrap_or("").starts_with(name)));
                    assert!(html(response).await.contains("value=\"50\" selected"),"{locale}: {route}{suffix}");
                }
                let response=request(&app,"GET",&format!("{route}?per_page=10"),&cookie,"",false).await;
                assert_eq!(response.status(),StatusCode::OK);
                assert!(html(response).await.contains("value=\"10\" selected"));
                for suffix in ["?per_page=500", "?per_page=wrong", "?sort=wrong&per_page=100"] {
                    let response=request(&app,"GET",&format!("{route}{suffix}"),&cookie,"",false).await;
                    assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
                    assert!(!response.headers().get_all(header::SET_COOKIE).iter().any(|v| v.to_str().unwrap_or("").starts_with(name)));
                }
                for value in ["bad".to_owned(),"500".to_owned(),format!("50; {name}=100")] {
                    let cookie=format!("{session}; {name}={value}");
                    let response=request(&app,"GET",route,&cookie,"",false).await;
                    assert_eq!(response.status(),StatusCode::OK);
                    assert!(html(response).await.contains("value=\"20\" selected"));
                }
                let response=request(&app,"GET",&format!("{route}?per_page=50"),"","",false).await;
                assert_ne!(response.status(),StatusCode::OK);
                assert!(!response.headers().get_all(header::SET_COOKIE).iter().any(|v| v.to_str().unwrap_or("").starts_with(name)));
            }
        }
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn invalid_customer_order_page_uses_order_errors_in_both_languages() {
    isolated(|pool, _| async move {
        let (_, owner, raw, id, _) = fixture(&pool).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (locale, expected) in [
            ("en", "Invalid order filters."),
            ("it", "Filtri ordini non validi."),
        ] {
            sqlx::query("UPDATE administrators SET locale=$1 WHERE id=$2")
                .bind(locale)
                .bind(owner.administrator_id)
                .execute(&pool)
                .await
                .expect("owner language");
            let response = request(
                &app,
                "GET",
                &format!("/admin/customers/{id}?orders_page=1000001"),
                &format!("talocart_session={raw}"),
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let page = html(response).await;
            assert!(
                page.contains(expected),
                "Order pagination feedback in {locale}"
            );
            assert!(
                !page.contains("check email, name and password")
                    && !page.contains("controlla email, nome e password")
            );
        }
        assert!(matches!(
            talocart_commerce::orders::Orders::new(pool.clone(), Identity::new(pool.clone()))
                .admin_customer_history(&owner, id, 1000001)
                .await,
            Err(talocart_commerce::orders::OrderError::ListFilters)
        ));
    })
    .await;
}
