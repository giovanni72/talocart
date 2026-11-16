use super::*;
use talocart_platform::identity::IdentityError;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn general_settings_http_validation_permissions_and_persistence() {
    use talocart_platform::identity::{Role, SettingsField, SettingsInput};
    isolated(|pool, options| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let owner = service.session(&raw).await.unwrap().unwrap();
        let auth = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Koala".into()).await);
        let initial = service.settings().await.unwrap().unwrap().values;
        assert_eq!(initial.name, "Shop");
        for field in SettingsField::ALL.into_iter().skip(1) {
            assert_eq!(initial.value(field), "");
        }
        let fields = "name=++Atelier++&legal_name=++Societ%C3%A0+%3Cscript%3E++&contact_email=++CONTATTI%40EXAMPLE.TEST++&phone=++%2B39+02+1234++&address_line1=++Via+Roma+1++&address_line2=++Scala+B++&postal_code=++00100++&city=++Roma++&province=++RM++&country=++Italia++&currency=USD&locale=en&role=owner";
        let body = format!("csrf={}&{fields}", owner.csrf);
        for partial in [false, true] {
            assert_eq!(request(&app,"POST","/admin/settings","",&body,partial).await.status(), if partial {StatusCode::UNAUTHORIZED} else {StatusCode::SEE_OTHER});
            assert_eq!(request(&app,"POST","/admin/settings",&auth,&format!("csrf=wrong&{fields}"),partial).await.status(), StatusCode::FORBIDDEN);
        }
        let invalid = body.replace("++CONTATTI%40EXAMPLE.TEST++", "bad-email").replace("++%2B39+02+1234++", &"x".repeat(41));
        let response = request(&app,"POST","/admin/settings",&auth,&invalid,true).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        let page = html(response).await;
        assert!(page.contains("id=\"error-contact_email\""));
        assert!(page.contains("aria-describedby=\"error-contact_email\""));
        assert!(page.contains("id=\"error-phone\""));
        assert_eq!(page.matches("aria-invalid=\"true\"").count(),2);
        assert!(page.contains("value=\"  Società &#60;script&#62;  \""));
        assert!(page.contains("value=\"  Via Roma 1  \""));
        assert!(service.settings().await.unwrap().unwrap().values == initial);
        let response = request(&app,"POST","/admin/settings",&auth,&body,false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION],"/admin/settings?saved=1");
        let expected = SettingsInput { name:"Atelier".into(),legal_name:"Società <script>".into(),contact_email:"contatti@example.test".into(),phone:"+39 02 1234".into(),address_line1:"Via Roma 1".into(),address_line2:"Scala B".into(),postal_code:"00100".into(),city:"Roma".into(),province:"RM".into(),country:"Italia".into() };
        let stored = service.settings().await.unwrap().unwrap();
        assert!(stored.values == expected);
        assert_eq!((stored.currency.as_str(),stored.locale.as_str()),("EUR","it"));
        assert_eq!(service.account(&owner).await.unwrap().email,"owner@example.com");
        let response = request(&app,"GET","/admin/settings?name=Overwrite",&auth,"",false).await;
        assert_eq!(response.status(),StatusCode::OK);
        assert!(service.settings().await.unwrap().unwrap().values == expected);
        service.create_administrator(&owner,&owner.csrf,"reader@example.test","correct horse battery".into(),Role::Viewer).await.unwrap();
        let reader_raw = login_as(&service,"reader@example.test").await;
        let reader = service.session(&reader_raw).await.unwrap().unwrap();
        let reader_auth = format!("talocart_session={reader_raw}");
        let page = html(request(&app,"GET","/admin/settings",&reader_auth,"",false).await).await;
        assert!(page.contains("Società &#60;script&#62;"));
        assert!(!page.contains("<script>"));
        assert!(page.contains("contatti@example.test"));
        assert!(page.contains("Via Roma 1"));
        assert!(!page.contains("name=\"contact_email\""));
        assert!(!page.contains("id=\"settings-save\""));
        for partial in [false,true] {
            let denied = format!("csrf={}&{fields}",reader.csrf);
            assert_eq!(request(&app,"POST","/admin/settings",&reader_auth,&denied,partial).await.status(),StatusCode::FORBIDDEN);
        }
        assert!(service.settings().await.unwrap().unwrap().values == expected);
        let reopened = PgPoolOptions::new().connect_with(options).await.unwrap();
        let restarted = test_identity(reopened.clone()).await;
        assert!(restarted.settings().await.unwrap().unwrap().values == expected);
        assert!(restarted.session(&raw).await.unwrap().is_some());
        // Removing optional data is a real replacement, never an implicit reset of the account.
        let cleared = SettingsInput { name:"Atelier".into(),..Default::default() };
        restarted.save_settings(&owner,&owner.csrf,&cleared).await.unwrap();
        assert!(restarted.settings().await.unwrap().unwrap().values == cleared);
        // Database limits also protect callers that bypass the service.
        for sql in ["UPDATE store_settings SET phone=repeat('x',41)","UPDATE store_settings SET city='x'||chr(10)||'y'","UPDATE store_settings SET contact_email='Bad@Example.test'","UPDATE store_settings SET legal_name=' untrimmed '"] {
            let error = sqlx::query(sql).execute(&pool).await.unwrap_err();
            assert_eq!(error.as_database_error().unwrap().code().unwrap(),"23514");
        }
        assert!(service.settings().await.unwrap().unwrap().values == cleared);
        reopened.close().await;
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn concurrent_general_settings_saves_do_not_mix_fields_and_revoked_sessions_cannot_write() {
    use talocart_platform::identity::SettingsInput;
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let owner = service.session(&raw).await.unwrap().unwrap();
        let a = SettingsInput {
            name: "Shop A".into(),
            legal_name: "Company A".into(),
            contact_email: "a@example.test".into(),
            phone: "111".into(),
            city: "City A".into(),
            ..Default::default()
        };
        let b = SettingsInput {
            name: "Shop B".into(),
            legal_name: "Company B".into(),
            contact_email: "b@example.test".into(),
            phone: "222".into(),
            city: "City B".into(),
            ..Default::default()
        };
        let (first, second) = tokio::join!(
            service.save_settings(&owner, &owner.csrf, &a),
            service.save_settings(&owner, &owner.csrf, &b)
        );
        first.unwrap();
        second.unwrap();
        let saved = service.settings().await.unwrap().unwrap().values;
        assert!(saved == a || saved == b);
        service.logout(&owner, &owner.csrf).await.unwrap();
        assert!(matches!(
            service.save_settings(&owner, &owner.csrf, &a).await,
            Err(IdentityError::Forbidden)
        ));
        assert!(service.settings().await.unwrap().unwrap().values == saved);
    })
    .await;
}
