use super::*;
use talocart_platform::{
    identity::{
        CustomerError, CustomerProfile, Customers, Permission,
        customer_admin::CustomerAdmin,
        customer_registration::{RegistrationEmail, RegistrationSettings},
    },
    mail::{Mail, Settings},
};
const SECRET: &str = "synthetic registration password";
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn registration_rejects_undeliverable_email_as_a_field_error() {
    isolated(|pool,_|async move{
        let(_,_,service)=configured(&pool).await;let(_,s)=service.anonymous().await.unwrap();
        let address=format!("{}@example.test","x".repeat(80));
        assert!(talocart_platform::mail::address(&address).is_err());
        assert!(matches!(service.register(&s,&s.csrf,&profile(&address),SECRET.into(),Some(&delivery())).await,Err(CustomerError::Fields(f)) if f.contains(&"email")));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM customers").fetch_one(&pool).await.unwrap(),0);
    }).await;
}
fn profile(address: &str) -> CustomerProfile {
    CustomerProfile {
        email: address.into(),
        first_name: "Alex".into(),
        last_name: "Example".into(),
        ..Default::default()
    }
}
fn delivery() -> RegistrationEmail<'static> {
    RegistrationEmail {
        origin: "https://shop.example.test",
        path: "/en/account/confirm",
        locale: "en",
    }
}
async fn configured(pool: &PgPool) -> (Identity, talocart_platform::identity::Session, Customers) {
    let identity = installed(pool).await;
    let owner = taxonomy_session(&identity).await;
    Mail::new(pool.clone(), identity.clone())
        .save(
            &owner,
            &owner.csrf,
            &Settings {
                enabled: true,
                from_name: "Synthetic shop".into(),
                from_email: "sender@example.test".into(),
                admin_recipients: vec!["owner@example.test".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    sqlx::query("UPDATE email_transport SET enabled=true")
        .execute(pool)
        .await
        .unwrap();
    RegistrationSettings {
        revision: 0,
        allow_guest_checkout: true,
        require_email_confirmation: true,
    }
    .save(pool, &owner, &owner.csrf)
    .await
    .unwrap();
    (
        identity.clone(),
        owner,
        Customers::new(pool.clone(), identity),
    )
}
async fn token_from_outbox(pool: &PgPool, address: &str) -> String {
    let body: String = sqlx::query_scalar(
        "SELECT body_text FROM email_outbox WHERE recipient=$1 ORDER BY id DESC LIMIT 1",
    )
    .bind(address)
    .fetch_one(pool)
    .await
    .unwrap();
    let link = body
        .split_whitespace()
        .find(|s| s.starts_with("https://"))
        .unwrap();
    link.split_once("token=").unwrap().1.to_owned()
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn registration_pending_blocks_access_and_confirm_is_single_use_and_concurrent() {
    isolated(|pool, _| async move {
        let (_, _, service) = configured(&pool).await;
        let (anonymous, s) = service.anonymous().await.unwrap();
        let d = delivery();
        let p1 = profile(" Buyer@Example.Test ");
        let p2 = profile("buyer@example.test");
        let (a, b) = tokio::join!(
            service.register(&s, &s.csrf, &p1, SECRET.into(), Some(&d)),
            service.register(&s, &s.csrf, &p2, SECRET.into(), Some(&d))
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(a.or(b).unwrap().is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM email_outbox")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        let id: i64 = sqlx::query_scalar(
            "SELECT id FROM customers WHERE email='buyer@example.test' AND NOT active",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            service
                .session(&anonymous)
                .await
                .unwrap()
                .unwrap()
                .customer_id()
                .is_none()
        );
        assert!(
            service
                .login(&s, &s.csrf, "buyer@example.test", SECRET.into())
                .await
                .is_err()
        );
        assert!(service.issue_reset("buyer@example.test").await.is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM guest_carts WHERE customer_id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        let raw = token_from_outbox(&pool, "buyer@example.test").await;
        let stored: String = sqlx::query_scalar(
            "SELECT token_hash FROM customer_email_confirmations WHERE customer_id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_ne!(stored, raw);
        assert_eq!(stored, digest(&raw));
        assert!(matches!(
            service.confirm_email(&s, "wrong", &raw).await,
            Err(CustomerError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        let (a, b) = tokio::join!(
            service.confirm_email(&s, &s.csrf, &raw),
            service.confirm_email(&s, &s.csrf, &raw)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(matches!(
            service.confirm_email(&s, &s.csrf, &raw).await,
            Err(CustomerError::ConfirmationInvalid)
        ));
        assert!(
            service
                .session(&anonymous)
                .await
                .unwrap()
                .unwrap()
                .customer_id()
                .is_none()
        );
        assert!(
            service
                .login(&s, &s.csrf, "buyer@example.test", SECRET.into())
                .await
                .is_ok()
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn registration_resend_expiry_cooldown_invalidation_and_unknown_addresses() {
    isolated(|pool,_|async move{
    let(_,_,service)=configured(&pool).await;let(_,s)=service.anonymous().await.unwrap();let d=delivery();
    service.register(&s,&s.csrf,&profile("buyer@example.test"),SECRET.into(),Some(&d)).await.unwrap();
    let old=token_from_outbox(&pool,"buyer@example.test").await;
    service.resend_confirmation(&s,&s.csrf,"buyer@example.test",&d).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM email_outbox").fetch_one(&pool).await.unwrap(),1);
    sqlx::query("UPDATE customer_email_confirmations SET expires_at=now()-interval '1 second',last_sent_at=now()-interval '2 minutes'").execute(&pool).await.unwrap();
    assert!(matches!(service.confirm_email(&s,&s.csrf,&old).await,Err(CustomerError::ConfirmationInvalid)));
    service.resend_confirmation(&s,&s.csrf,"buyer@example.test",&d).await.unwrap();let new=token_from_outbox(&pool,"buyer@example.test").await;assert_ne!(old,new);
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM email_outbox ORDER BY id LIMIT 1").fetch_one(&pool).await.unwrap(),"superseded");
    assert!(matches!(service.confirm_email(&s,&s.csrf,&old).await,Err(CustomerError::ConfirmationInvalid)));
    service.resend_confirmation(&s,&s.csrf,"unknown@example.test",&d).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM email_outbox").fetch_one(&pool).await.unwrap(),2);
    service.confirm_email(&s,&s.csrf,&new).await.unwrap();
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn registration_outbox_failure_rolls_back_and_transport_failure_never_activates() {
    isolated(|pool,_|async move{
    let(_,_,service)=configured(&pool).await;let(_,s)=service.anonymous().await.unwrap();let d=delivery();
    assert!(service.register(&s,&s.csrf,&profile("missing@example.test"),SECRET.into(),None).await.is_err());
    sqlx::query("UPDATE email_transport SET enabled=false").execute(&pool).await.unwrap();
    let disabled=delivery();
    assert!(service.register(&s,&s.csrf,&profile("disabled@example.test"),SECRET.into(),Some(&disabled)).await.is_err());
    sqlx::query("UPDATE email_transport SET enabled=true").execute(&pool).await.unwrap();
    sqlx::raw_sql("CREATE FUNCTION reject_confirmation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic'; END; $$; CREATE TRIGGER reject_confirmation BEFORE INSERT ON email_outbox FOR EACH ROW EXECUTE FUNCTION reject_confirmation();").execute(&pool).await.unwrap();
    assert!(matches!(service.register(&s,&s.csrf,&profile("rollback@example.test"),SECRET.into(),Some(&d)).await,Err(CustomerError::Technical(_))), "Outbox SQL failures must retain their technical cause");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM customers").fetch_one(&pool).await.unwrap(),0);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM customer_email_confirmations").fetch_one(&pool).await.unwrap(),0);
    sqlx::query("DROP TRIGGER reject_confirmation ON email_outbox").execute(&pool).await.unwrap();
    let expired=service.anonymous().await.unwrap().1;
    sqlx::query("UPDATE customer_sessions SET expires_at=now()-interval '1 second'").execute(&pool).await.unwrap();
    assert!(service.register(&expired,&expired.csrf,&profile("expired@example.test"),SECRET.into(),Some(&d)).await.is_err());
 }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn registration_admin_changes_cancel_pending_activation_and_policy_only_affects_new_accounts()
{
    isolated(|pool, _| async move {
        let (identity, owner, service) = configured(&pool).await;
        let (_, s) = service.anonymous().await.unwrap();
        let d = delivery();
        service
            .register(
                &s,
                &s.csrf,
                &profile("pending@example.test"),
                SECRET.into(),
                Some(&d),
            )
            .await
            .unwrap();
        let raw = token_from_outbox(&pool, "pending@example.test").await;
        let id: i64 = sqlx::query_scalar("SELECT id FROM customers")
            .fetch_one(&pool)
            .await
            .unwrap();
        let admin = CustomerAdmin::new(pool.clone(), identity);
        let member = admin.get(&owner, id).await.unwrap();
        admin
            .save(&owner, &owner.csrf, id, member.revision, &member.profile())
            .await
            .unwrap();
        assert!(matches!(
            service.confirm_email(&s, &s.csrf, &raw).await,
            Err(CustomerError::ConfirmationInvalid)
        ));
        service
            .resend_confirmation(&s, &s.csrf, "pending@example.test", &d)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM email_outbox")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        RegistrationSettings {
            revision: 1,
            allow_guest_checkout: true,
            require_email_confirmation: false,
        }
        .save(&pool, &owner, &owner.csrf)
        .await
        .unwrap();
        let immediate = service
            .register(
                &s,
                &s.csrf,
                &profile("immediate@example.test"),
                SECRET.into(),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            service
                .session(&immediate)
                .await
                .unwrap()
                .unwrap()
                .customer_id()
                .is_some()
        );
        assert!(!admin.get(&owner, id).await.unwrap().active);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn registration_settings_permissions_revision_transport_and_http_forms() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let owner = taxonomy_session(&identity).await;
        let settings = RegistrationSettings {
            revision: 0,
            allow_guest_checkout: true,
            require_email_confirmation: true,
        };
        assert!(settings.save(&pool, &owner, "wrong").await.is_err());
        assert!(settings.save(&pool, &owner, &owner.csrf).await.is_err());
        let mail = Mail::new(pool.clone(), identity.clone());
        mail.save(
            &owner,
            &owner.csrf,
            &Settings {
                enabled: true,
                from_name: "Shop".into(),
                from_email: "sender@example.test".into(),
                admin_recipients: vec!["owner@example.test".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        sqlx::query("UPDATE email_transport SET enabled=true")
            .execute(&pool)
            .await
            .unwrap();
        let immediate = RegistrationSettings {
            revision: 0,
            allow_guest_checkout: true,
            require_email_confirmation: false,
        };
        let (a, b) = tokio::join!(
            settings.save(&pool, &owner, &owner.csrf),
            immediate.save(&pool, &owner, &owner.csrf)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let reader_role = identity
            .create_role(&owner, &owner.csrf, "Reader", &[Permission::SettingsRead])
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "reader@example.test",
                "correct horse battery".into(),
                talocart_platform::identity::Role::parse(&reader_role).unwrap(),
            )
            .await
            .unwrap();
        let reader = login_as(&identity, "reader@example.test").await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/customer-registration",
                &format!("talocart_session={reader}"),
                "csrf=x&revision=1&allow_guest_checkout=true&activation=email",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        for (locale, label) in [
            ("it", "Attivazione dei nuovi account"),
            ("en", "New account activation"),
        ] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let response = request(
                &app,
                "GET",
                "/admin/settings/customer-fields",
                &cookie,
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(html(response).await.contains(label));
        }
        let session = identity.session(&raw).await.unwrap().unwrap();
        for extra in [
            "&unexpected=x",
            "&activation=immediate",
            "&allow_guest_checkout=false",
        ] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/admin/settings/customer-registration",
                    &cookie,
                    &format!(
                        "csrf={}&revision=1&allow_guest_checkout=true&activation=email{extra}",
                        session.csrf
                    ),
                    false
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        sqlx::query("UPDATE email_transport SET enabled=false")
            .execute(&pool)
            .await
            .unwrap();
        let response = request(
            &app,
            "POST",
            "/admin/settings/customer-registration",
            &cookie,
            &format!(
                "csrf={}&revision=1&allow_guest_checkout=true&activation=email",
                session.csrf
            ),
            false,
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "Disabled SMTP cannot enable confirmation"
        );
        sqlx::query("UPDATE email_transport SET enabled=true")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/customer-registration",
                &cookie,
                &format!(
                    "csrf={}&revision=1&allow_guest_checkout=true&activation=email",
                    session.csrf
                ),
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
                "/admin/settings/customer-registration",
                &cookie,
                &format!(
                    "csrf={}&revision=1&allow_guest_checkout=true&activation=immediate",
                    session.csrf
                ),
                false
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn registration_http_bilingual_no_auto_login_get_is_read_only_and_csrf() {
    isolated(|pool,_|async move{
    let(_,_,_)=configured(&pool).await;
    let app=router(test_app_config(pool.clone(),"Shop".into(), talocart_web::AppStateConfig { public_origin: "https://shop.example.test".into(), ..Default::default() }).await);
    for(prefix,locale)in[("","it"),("/en","en")]{
        let page=request(&app,"GET",&format!("{prefix}/account/register"),"","",false).await;
        let cookie=page.headers().get_all(header::SET_COOKIE).iter().map(|h|h.to_str().unwrap().split(';').next().unwrap().to_owned()).collect::<Vec<_>>().join("; ");
        let body=html(page).await;let csrf=body.split("name=\"csrf\" value=\"").nth(1).unwrap().split('"').next().unwrap();
        let address=format!("{locale}@example.test");
        let data=format!("csrf={csrf}&first_name=Alex&last_name=Example&email={address}&password={SECRET}&password_confirmation={SECRET}");
        let response=request(&app,"POST",&format!("{prefix}/account/register"),&cookie,&data,false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);assert_eq!(response.headers()[header::LOCATION],format!("{prefix}/account/confirmation?changed=pending"));
        assert!(response.headers().get_all(header::SET_COOKIE).iter().all(|h|!h.to_str().unwrap().starts_with("talocart_customer=")));
        let raw=token_from_outbox(&pool,&address).await;
        let response=request(&app,"GET",&format!("{prefix}/account/confirm?token={raw}"),&cookie,"",false).await;
        assert_eq!(response.status(),StatusCode::OK);assert_eq!(response.headers()[header::REFERRER_POLICY],"no-referrer");assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");assert_eq!(response.headers()[header::CONTENT_LANGUAGE],locale);
        let body=html(response).await;assert!(body.contains(&format!("value=\"{raw}\"")));
        assert!(!sqlx::query_scalar::<_,bool>("SELECT active FROM customers WHERE email=$1").bind(&address).fetch_one(&pool).await.unwrap());
        assert_eq!(request(&app,"POST",&format!("{prefix}/account/confirm"),&cookie,&format!("csrf=wrong&token={raw}"),false).await.status(),StatusCode::FORBIDDEN);
        let response=request(&app,"POST",&format!("{prefix}/account/confirm"),&cookie,&format!("csrf={csrf}&token={raw}"),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);assert_eq!(response.headers()[header::LOCATION],format!("{prefix}/account/login?changed=confirmed"));
        assert!(sqlx::query_scalar::<_,bool>("SELECT active FROM customers WHERE email=$1").bind(&address).fetch_one(&pool).await.unwrap());
    }
 }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn confirmation_supersession_failure_preserves_cause_and_rolls_back() {
    supersession_failure(false).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn admin_supersession_failure_preserves_cause_and_rolls_back() {
    supersession_failure(true).await;
}
async fn supersession_failure(admin_change: bool) {
    isolated(move |pool, _| async move {
        let (identity, owner, service) = configured(&pool).await;
        let (_, session) = service.anonymous().await.unwrap();
        service.register(&session, &session.csrf, &profile("pending@example.test"), SECRET.into(), Some(&delivery())).await.unwrap();
        let token = token_from_outbox(&pool, "pending@example.test").await;
        let id: i64 = sqlx::query_scalar("SELECT id FROM customers").fetch_one(&pool).await.unwrap();
        let admin = CustomerAdmin::new(pool.clone(), identity);
        let before = admin.get(&owner, id).await.unwrap();
        sqlx::raw_sql("CREATE FUNCTION reject_supersession() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic' USING ERRCODE='40001'; END $$; CREATE TRIGGER reject_supersession BEFORE UPDATE ON email_outbox FOR EACH ROW EXECUTE FUNCTION reject_supersession();").execute(&pool).await.unwrap();
        let result = if admin_change {
            let mut input = before.profile();
            input.email = "changed@example.test".into();
            input.active = true;
            admin.save(&owner, &owner.csrf, id, before.revision, &input).await
        } else {
            service.confirm_email(&session, &session.csrf, &token).await
        };
        let after = admin.get(&owner, id).await.unwrap();
        assert_eq!((after.email, after.active, after.revision), (before.email, before.active, before.revision));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM customer_email_confirmations").fetch_one(&pool).await.unwrap(), 1);
        assert!(!sqlx::query_scalar::<_,bool>("SELECT superseded FROM email_outbox WHERE template='customer_confirmation'").fetch_one(&pool).await.unwrap());
        assert!(matches!(result, Err(CustomerError::Technical(talocart_platform::technical::TechnicalError::Database(ref cause))) if cause.sqlstate() == Some("40001")), "Supersession must preserve SQLSTATE: {result:?}");
        sqlx::query("DROP TRIGGER reject_supersession ON email_outbox").execute(&pool).await.unwrap();
        service.confirm_email(&session, &session.csrf, &token).await.unwrap();
        assert!(admin.get(&owner, id).await.unwrap().active);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn guest_checkout_upgrade_preserves_registration_and_existing_data() {
    isolated_before(1, |pool, _| async move {
        sqlx::query(
            "UPDATE customer_registration_settings SET revision=7,require_email_confirmation=true",
        )
        .execute(&pool)
        .await
        .unwrap();
        let before = snapshot_tables(&pool).await;
        database::migrate(&pool).await.unwrap();
        let policy = RegistrationSettings::load(&pool).await.unwrap();
        assert!(policy.allow_guest_checkout);
        assert!(policy.require_email_confirmation);
        assert_eq!(policy.revision, 7);
        assert_tables_preserved(&pool, before).await;
        database::migrate(&pool).await.unwrap();
        assert!(
            RegistrationSettings::load(&pool)
                .await
                .unwrap()
                .allow_guest_checkout
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn guest_checkout_policy_write_waits_for_checkout_transaction() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let owner = taxonomy_session(&identity).await;
        let mut tx = pool.begin().await.unwrap();
        assert!(
            RegistrationSettings::guests_allowed_in(&mut tx)
                .await
                .unwrap()
        );
        let policy = RegistrationSettings {
            allow_guest_checkout: false,
            ..Default::default()
        };
        let save = policy.save(&pool, &owner, &owner.csrf);
        tokio::pin!(save);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut save)
                .await
                .is_err()
        );
        tx.commit().await.unwrap();
        save.await.unwrap();
        assert!(
            !RegistrationSettings::load(&pool)
                .await
                .unwrap()
                .allow_guest_checkout
        );
    })
    .await;
}
