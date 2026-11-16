use super::*;
use talocart_platform::identity::IdentityError;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn personal_account_credentials_and_session_contract() {
    isolated(|pool, _| async move {
        use talocart_platform::identity::{Permission, Role};
        let service = installed(&pool).await;
        let owner_raw = authenticated(&service).await;
        let owner = service.session(&owner_raw).await.unwrap().unwrap();
        let empty_role = service
            .create_role(&owner, &owner.csrf, "No backend areas", &[])
            .await
            .unwrap();
        let id = service
            .create_administrator(
                &owner,
                &owner.csrf,
                "personal@example.test",
                "personal account password".into(),
                Role::parse(&empty_role).unwrap(),
            )
            .await
            .unwrap();
        let (_, anonymous) = service.anonymous().await.unwrap();
        let raw = service
            .login(
                &anonymous,
                &anonymous.csrf,
                "personal@example.test",
                "personal account password".into(),
            )
            .await
            .unwrap();
        let session = service.session(&raw).await.unwrap().unwrap();
        assert!(!session.allows(Permission::AdminRead));
        let (_, anonymous) = service.anonymous().await.unwrap();
        let second = service
            .login(
                &anonymous,
                &anonymous.csrf,
                "personal@example.test",
                "personal account password".into(),
            )
            .await
            .unwrap();
        let recovery = service.issue_reset("personal@example.test").await.unwrap();
        assert_eq!(service.account(&session).await.unwrap().id, id);
        assert!(matches!(
            service
                .update_account(
                    &session,
                    "wrong",
                    "Name",
                    "personal@example.test",
                    String::new()
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(
            !service
                .update_account(
                    &session,
                    &session.csrf,
                    "  Personal User  ",
                    "personal@example.test",
                    String::new()
                )
                .await
                .unwrap()
        );
        assert_eq!(
            service.account(&session).await.unwrap().display_name,
            "Personal User"
        );
        assert!(service.session(&raw).await.unwrap().is_some());
        assert!(service.session(&second).await.unwrap().is_some());
        assert!(matches!(
            service
                .update_account(
                    &session,
                    &session.csrf,
                    "No change",
                    "changed@example.test",
                    "wrong".into()
                )
                .await,
            Err(IdentityError::CurrentPassword)
        ));
        assert_eq!(
            service.account(&session).await.unwrap().email,
            "personal@example.test"
        );
        assert!(matches!(
            service
                .update_account(
                    &session,
                    &session.csrf,
                    "No change",
                    "owner@example.com",
                    "personal account password".into()
                )
                .await,
            Err(IdentityError::DuplicateEmail)
        ));
        assert!(
            service
                .update_account(
                    &session,
                    &session.csrf,
                    "Personal User",
                    "CHANGED@example.test",
                    "personal account password".into()
                )
                .await
                .unwrap()
        );
        assert!(service.session(&raw).await.unwrap().is_none());
        assert!(service.session(&second).await.unwrap().is_none());
        assert!(matches!(
            service.account(&session).await,
            Err(IdentityError::Forbidden)
        ));
        let kept_role: String = sqlx::query_scalar("SELECT role FROM administrators WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(kept_role, empty_role);
        let gone: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS(SELECT 1 FROM password_resets WHERE token_hash=$1)",
        )
        .bind(digest(&recovery))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(gone);
        assert!(service.session(&owner_raw).await.unwrap().is_some());
        let (_, anonymous) = service.anonymous().await.unwrap();
        let raw = service
            .login(
                &anonymous,
                &anonymous.csrf,
                "changed@example.test",
                "personal account password".into(),
            )
            .await
            .unwrap();
        let session = service.session(&raw).await.unwrap().unwrap();
        assert!(matches!(
            service
                .change_password(
                    &session,
                    &session.csrf,
                    "wrong".into(),
                    "new correct password".into(),
                    "new correct password"
                )
                .await,
            Err(IdentityError::CurrentPassword)
        ));
        assert!(matches!(
            service
                .change_password(
                    &session,
                    &session.csrf,
                    "personal account password".into(),
                    "new correct password".into(),
                    "not matching password"
                )
                .await,
            Err(IdentityError::PasswordMismatch)
        ));
        assert!(matches!(
            service
                .change_password(
                    &session,
                    &session.csrf,
                    "personal account password".into(),
                    "short".into(),
                    "short"
                )
                .await,
            Err(IdentityError::Invalid)
        ));
        service
            .change_password(
                &session,
                &session.csrf,
                "personal account password".into(),
                "new correct password".into(),
                "new correct password",
            )
            .await
            .unwrap();
        assert!(service.session(&raw).await.unwrap().is_none());
        assert!(matches!(
            service.update_avatar(&session, &session.csrf, None).await,
            Err(IdentityError::Forbidden)
        ));
        let (_, anonymous) = service.anonymous().await.unwrap();
        assert!(
            service
                .login(
                    &anonymous,
                    &anonymous.csrf,
                    "changed@example.test",
                    "personal account password".into()
                )
                .await
                .is_err()
        );
        let raw = service
            .login(
                &anonymous,
                &anonymous.csrf,
                "changed@example.test",
                "new correct password".into(),
            )
            .await
            .unwrap();
        let session = service.session(&raw).await.unwrap().unwrap();
        service
            .update_account(
                &session,
                &session.csrf,
                "",
                "changed@example.test",
                String::new(),
            )
            .await
            .unwrap();
        assert!(
            service
                .account(&session)
                .await
                .unwrap()
                .display_name
                .is_empty()
        );
        service
            .update_administrator(
                &owner,
                &owner.csrf,
                id,
                Role::parse(&empty_role).unwrap(),
                false,
            )
            .await
            .unwrap();
        assert!(service.account(&session).await.is_err());
        assert!(
            service
                .update_account(
                    &session,
                    &session.csrf,
                    "Name",
                    "changed@example.test",
                    String::new()
                )
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn account_http_avatar_security_and_private_ownership() {
    isolated(|pool,_| async move {
        use talocart_platform::identity::Role;
        let service=installed(&pool).await;
        let raw=authenticated(&service).await;
        let session=service.session(&raw).await.unwrap().unwrap();
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let owner_cookie=format!("talocart_session={raw}");
        let response=request(&app,"GET","/admin/account",&owner_cookie,"",false).await;
        assert_eq!(response.status(),StatusCode::OK);
        let page=html(response).await;
        assert!(page.contains("Menu account"));assert!(!page.contains("Fase 1.2"));
        for path in ["/admin/account","/admin/account/avatar","/admin/account/password","/admin/account/avatar/delete"] {
            let response=request(&app,"GET",path,"","",false).await;
            assert_eq!(response.status(),StatusCode::SEE_OTHER);
        }
        let response=request(&app,"GET","/admin/account/avatar",&owner_cookie,"",false).await;
        assert_eq!(response.status(),StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE],"image/svg+xml");
        let raw_png=avatar_fixture();
        assert!(raw_png.len()>16*1024, "Upload exercises its route-specific body limit");
        for (data,token,expected) in [(raw_png.as_slice(),"wrong",StatusCode::FORBIDDEN),(b"<svg onload='alert(1)'/>".as_slice(),session.csrf.as_str(),StatusCode::UNPROCESSABLE_ENTITY),(raw_png.as_slice(),session.csrf.as_str(),StatusCode::SEE_OTHER)] {
            let response=avatar_upload_request(&app,&owner_cookie,token,data,false).await;
            assert_eq!(response.status(),expected);
        }
        let response=request(&app,"GET","/admin/account/avatar",&owner_cookie,"",false).await;
        assert_eq!(response.status(),StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE],"image/png");
        assert_eq!(response.headers()[header::CACHE_CONTROL],"private, no-cache");
        assert_eq!(response.headers()[header::X_CONTENT_TYPE_OPTIONS],"nosniff");
        let png=to_bytes(response.into_body(),600_000).await.unwrap();
        let image=image::load_from_memory(&png).unwrap();
        assert_eq!((image.width(),image.height()),(256,256));
        assert!(service.session(&raw).await.unwrap().unwrap().has_avatar);
        let before=service.avatar(&session).await.unwrap().unwrap();
        let response=avatar_upload_request(&app,&owner_cookie,&session.csrf,&raw_png,true).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(service.avatar(&session).await.unwrap().unwrap(),before);
        let huge=vec![0;talocart_platform::identity::AVATAR_UPLOAD_LIMIT+20*1024];
        let response=avatar_upload_request(&app,&owner_cookie,&session.csrf,&huge,false).await;
        assert!(matches!(response.status(),StatusCode::PAYLOAD_TOO_LARGE|StatusCode::UNPROCESSABLE_ENTITY));
        assert_eq!(service.avatar(&session).await.unwrap().unwrap(),before);
        service.create_administrator(&session,&session.csrf,"other@example.test","other correct password".into(),Role::Viewer).await.unwrap();
        let (_,anon)=service.anonymous().await.unwrap();
        let other_raw=service.login(&anon,&anon.csrf,"other@example.test","other correct password".into()).await.unwrap();
        let other_cookie=format!("talocart_session={other_raw}");
        let response=request(&app,"GET","/admin/account/avatar?id=1",&other_cookie,"",false).await;
        assert_eq!(response.status(),StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE],"image/svg+xml");
        let other=service.session(&other_raw).await.unwrap().unwrap();
        let response=request(&app,"POST","/admin/account",&other_cookie,&format!("csrf={}&display_name=Other&email=other%40example.test&administrator_id=1&role=owner",other.csrf),false).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(service.account(&other).await.unwrap().role_name,"Lettore");
        let response=request(&app,"POST","/admin/account",&owner_cookie,&format!("csrf={}&display_name=%3Cscript%3E&email=owner%40example.com",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        let page=html(request(&app,"GET","/admin/account",&owner_cookie,"",false).await).await;
        assert!(!page.contains("<script>"));assert!(page.contains("&#60;script&#62;"));
        let response=request(&app,"POST","/admin/account/password",&owner_cookie,&format!("csrf={}&current_password=secret_wrong&new_password=new%20secret%20password&confirmation=new%20secret%20password",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        let page=html(response).await;
        assert!(!page.contains("secret_wrong"));assert!(!page.contains("new secret password"));
        let response=request(&app,"GET","/admin/account/avatar/delete",&owner_cookie,"",false).await;
        assert_eq!(response.status(),StatusCode::METHOD_NOT_ALLOWED);
        let response=request(&app,"POST","/admin/account/avatar/delete",&owner_cookie,"csrf=wrong",false).await;
        assert_eq!(response.status(),StatusCode::FORBIDDEN);
        let response=request(&app,"POST","/admin/account/avatar/delete",&owner_cookie,&format!("csrf={}",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert!(service.avatar(&session).await.unwrap().is_none());
        assert!(service.session(&raw).await.unwrap().is_some());
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn concurrent_password_changes_require_the_live_session_and_hash() {
    isolated(|pool, _| async move {
        let first = installed(&pool).await;
        let second = test_identity(pool.clone()).await;
        let raw = authenticated(&first).await;
        let session = first.session(&raw).await.unwrap().unwrap();
        let (a, b) = tokio::join!(
            first.change_password(
                &session,
                &session.csrf,
                "correct horse battery".into(),
                "first new password".into(),
                "first new password"
            ),
            second.change_password(
                &session,
                &session.csrf,
                "correct horse battery".into(),
                "second new password".into(),
                "second new password"
            )
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let error = if let Err(error) = a {
            error
        } else {
            b.unwrap_err()
        };
        assert!(matches!(error, IdentityError::Forbidden));
        assert!(first.session(&raw).await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn account_reauthentication_is_limited_without_blocking_profile_reads() {
    isolated(|pool,_| async move {
        let service=installed(&pool).await;
        let raw=authenticated(&service).await;
        let session=service.session(&raw).await.unwrap().unwrap();
        for _ in 0..10 {
            assert!(matches!(service.update_account(&session,&session.csrf,"Owner","changed@example.test","wrong password".into()).await,Err(IdentityError::CurrentPassword)));
        }
        assert!(matches!(service.change_password(&session,&session.csrf,"correct horse battery".into(),"new correct password".into(),"new correct password").await,Err(IdentityError::Limited)));
        assert_eq!(service.account(&session).await.unwrap().email,"owner@example.com");
        assert!(!service.update_account(&session,&session.csrf,"Owner","owner@example.com",String::new()).await.unwrap());
        assert!(service.session(&raw).await.unwrap().is_some());
        sqlx::query("UPDATE auth_limits SET expires_at=now()-interval '1 second' WHERE bucket LIKE 'account-change:%'").execute(&pool).await.unwrap();
        service.change_password(&session,&session.csrf,"correct horse battery".into(),"new correct password".into(),"new correct password").await.unwrap();
        assert!(service.session(&raw).await.unwrap().is_none());
    }).await;
}
