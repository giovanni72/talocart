use super::*;
use talocart_platform::identity::IdentityError;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn administrator_lifecycle_permissions_revocation_and_reactivation() {
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let owner_raw = authenticated(&service).await;
        let owner = service.session(&owner_raw).await.unwrap().unwrap();
        assert!(matches!(
            service
                .create_administrator(
                    &owner,
                    "wrong",
                    "reader@example.com",
                    "correct horse battery".into(),
                    Role::Viewer
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .create_administrator(
                    &owner,
                    &owner.csrf,
                    "reader@example.com",
                    "short".into(),
                    Role::Viewer
                )
                .await,
            Err(IdentityError::Invalid)
        ));
        let id = service
            .create_administrator(
                &owner,
                &owner.csrf,
                " Reader@Example.com ",
                "correct horse battery".into(),
                Role::Viewer,
            )
            .await
            .unwrap();
        assert!(matches!(
            service
                .create_administrator(
                    &owner,
                    &owner.csrf,
                    "reader@example.com",
                    "correct horse battery".into(),
                    Role::Owner
                )
                .await,
            Err(IdentityError::DuplicateEmail)
        ));
        assert_eq!(
            service.administrator(&owner, id).await.unwrap().email,
            "reader@example.com"
        );
        let reader_raw = login_as(&service, "reader@example.com").await;
        let reader = service.session(&reader_raw).await.unwrap().unwrap();
        assert!(
            service
                .authorize(&reader, Permission::SettingsRead)
                .await
                .is_ok()
        );
        assert!(matches!(
            service.administrators(&reader, 0).await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .create_administrator(
                    &reader,
                    &reader.csrf,
                    "other@example.com",
                    "correct horse battery".into(),
                    Role::Owner
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .update_administrator(&reader, &reader.csrf, id, Role::Owner, true)
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .save_settings(
                    &reader,
                    &reader.csrf,
                    &talocart_platform::identity::SettingsInput {
                        name: "Denied".into(),
                        ..Default::default()
                    }
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .update_administrator(&owner, "wrong", id, Role::Owner, true)
                .await,
            Err(IdentityError::Forbidden)
        ));
        service
            .update_administrator(&owner, &owner.csrf, id, Role::Viewer, true)
            .await
            .unwrap();
        assert!(
            service.session(&reader_raw).await.unwrap().is_some(),
            "No-op must preserve sessions"
        );
        service
            .update_administrator(&owner, &owner.csrf, id, Role::Owner, true)
            .await
            .unwrap();
        assert!(service.session(&reader_raw).await.unwrap().is_none());
        let promoted_raw = login_as(&service, "reader@example.com").await;
        let promoted = service.session(&promoted_raw).await.unwrap().unwrap();
        let other_raw = login_as(&service, "reader@example.com").await;
        let reset = service.issue_reset("reader@example.com").await.unwrap();
        service
            .update_administrator(&owner, &owner.csrf, id, Role::Viewer, false)
            .await
            .unwrap();
        assert!(service.session(&promoted_raw).await.unwrap().is_none());
        assert!(service.session(&other_raw).await.unwrap().is_none());
        assert!(matches!(
            service
                .create_administrator(
                    &promoted,
                    &promoted.csrf,
                    "stale@example.com",
                    "correct horse battery".into(),
                    Role::Owner
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .save_settings(
                    &promoted,
                    &promoted.csrf,
                    &talocart_platform::identity::SettingsInput {
                        name: "Stale".into(),
                        ..Default::default()
                    }
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        let (_, anon) = service.anonymous().await.unwrap();
        assert!(matches!(
            service
                .login(
                    &anon,
                    &anon.csrf,
                    "reader@example.com",
                    "correct horse battery".into()
                )
                .await,
            Err(IdentityError::Unauthorized)
        ));
        service
            .update_administrator(&owner, &owner.csrf, id, Role::Viewer, true)
            .await
            .unwrap();
        assert!(matches!(
            service
                .reset(&anon, &anon.csrf, &reset, "other correct horse".into())
                .await,
            Err(IdentityError::Unauthorized)
        ));
        assert!(
            service
                .session(&login_as(&service, "reader@example.com").await)
                .await
                .unwrap()
                .is_some()
        );
        assert!(service.session(&owner_raw).await.unwrap().is_some());
        assert!(matches!(
            service.administrator(&owner, i64::MAX).await,
            Err(IdentityError::NotFound)
        ));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn last_owner_is_protected_even_when_two_owners_demote_concurrently() {
    use talocart_platform::identity::Role;
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let first = service.session(&raw).await.unwrap().unwrap();
        let first_id = first.administrator_id.unwrap();
        for (role, active) in [(Role::Viewer, true), (Role::Owner, false)] {
            assert!(matches!(
                service
                    .update_administrator(&first, &first.csrf, first_id, role, active)
                    .await,
                Err(IdentityError::LastOwner)
            ));
            assert!(service.session(&raw).await.unwrap().is_some());
        }
        let second_id = service
            .create_administrator(
                &first,
                &first.csrf,
                "second@example.com",
                "correct horse battery".into(),
                Role::Owner,
            )
            .await
            .unwrap();
        let second_raw = login_as(&service, "second@example.com").await;
        let second = service.session(&second_raw).await.unwrap().unwrap();
        // Separate service instances share only the database, as independent application processes would.
        let other = test_identity(pool.clone()).await;
        let (a, b) = tokio::join!(
            service.update_administrator(&first, &first.csrf, first_id, Role::Viewer, true),
            other.update_administrator(&second, &second.csrf, second_id, Role::Owner, false)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(
            matches!(a, Err(IdentityError::LastOwner))
                || matches!(b, Err(IdentityError::LastOwner))
        );
        let owners: i64 =
            sqlx::query_scalar("SELECT count(*) FROM administrators WHERE role='owner' AND active")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(owners, 1);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn administrator_listing_is_bounded_and_has_no_secret_fields() {
    use talocart_platform::identity::ADMINISTRATOR_PAGE_SIZE;
    isolated(|pool, _| async move {
        let service=installed(&pool).await;
        let raw=authenticated(&service).await;
        let owner=service.session(&raw).await.unwrap().unwrap();
        // Synthetic fixtures reuse a real hash; avoid unrelated password hashing in pagination tests.
        sqlx::query("INSERT INTO administrators(email,password_hash,role) SELECT 'page-'||n||'@example.com',a.password_hash,'viewer' FROM administrators a CROSS JOIN generate_series(1,25) n WHERE a.email='owner@example.com'").execute(&pool).await.unwrap();
        let first=service.administrators(&owner,0).await.unwrap();
        assert_eq!(first.len(),ADMINISTRATOR_PAGE_SIZE+1);
        let next=first[ADMINISTRATOR_PAGE_SIZE-1].id;
        let second=service.administrators(&owner,next).await.unwrap();
        assert_eq!(second.len(),6);
        assert!(second.iter().all(|u|u.id>next));
        let app=router(test_app(pool,"Koala".into()).await);
        let body=html(request(&app,"GET","/admin/users",&format!("talocart_session={raw}"),"",false).await).await;
        assert_eq!(body.matches("class=\"user-row\"").count(),20);
        assert!(body.contains("/admin/users?page=2"));
        assert!(body.contains("Amministratori 1–20 di 26"));
        let legacy=request(&app,"GET",&format!("/admin/users?after={next}"),&format!("talocart_session={raw}"),"",false).await;
        assert_eq!(legacy.status(),StatusCode::SEE_OTHER);
        assert_eq!(legacy.headers()[header::LOCATION],"/admin/users?page=2");
        let body=html(request(&app,"GET","/admin/users?page=99",&format!("talocart_session={raw}"),"",false).await).await;
        assert!(body.contains("Amministratori 21–26 di 26"));
        assert!(body.contains("Pagina 2 di 2"));
        assert!(!body.contains("$argon2"));
        assert!(!body.contains(&raw));
        assert!(matches!(service.administrators(&owner,-1).await,Err(IdentityError::Invalid)));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn backend_http_forms_permissions_csrf_and_error_feedback() {
    use talocart_platform::identity::Role;
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let owner = service.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Koala".into()).await);
        let create = format!(
            "csrf={}&email=reader%40example.com&password=correct+horse+battery&role=viewer",
            owner.csrf
        );
        for path in ["/admin/settings", "/admin/users", "/admin/users/new"] {
            assert_eq!(
                request(&app, "GET", path, &cookie, "", false)
                    .await
                    .status(),
                StatusCode::OK
            );
            assert_eq!(
                request(&app, "GET", path, "", "", true).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/users/new",
                &cookie,
                "csrf=wrong&email=reader%40example.com&password=correct+horse+battery&role=viewer",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let invalid = request(
            &app,
            "POST",
            "/admin/users/new",
            &cookie,
            &create.replace("correct+horse+battery", "short"),
            false,
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = html(invalid).await;
        assert!(body.contains("role=\"alert\""));
        assert!(body.contains("value=\"reader@example.com\""));
        assert!(!body.contains("value=\"short\""));
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/users/new",
                &cookie,
                &create.replace("role=viewer", "role=root"),
                false
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            request(&app, "POST", "/admin/users/new", &cookie, &create, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            request(&app, "POST", "/admin/users/new", &cookie, &create, false)
                .await
                .status(),
            StatusCode::CONFLICT
        );
        let invalid = request(
            &app,
            "POST",
            "/admin/settings",
            &cookie,
            &format!("csrf={}&name=%20%20", owner.csrf),
            false,
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = html(invalid).await;
        assert!(body.contains("aria-invalid=\"true\""));
        assert!(body.contains("settings-error"));
        assert_eq!(
            service.settings().await.unwrap().unwrap().values.name,
            "Shop"
        );
        let response = request(
            &app,
            "POST",
            "/admin/settings",
            &cookie,
            &format!("csrf={}&name=Changed", owner.csrf),
            false,
        )
        .await;
        assert_eq!(
            response.headers()[header::LOCATION],
            "/admin/settings?saved=1"
        );
        let owner_id = owner.administrator_id.unwrap();
        let last = request(
            &app,
            "POST",
            &format!("/admin/users/{owner_id}"),
            &cookie,
            &format!("csrf={}&role=viewer&active=true", owner.csrf),
            false,
        )
        .await;
        assert_eq!(last.status(), StatusCode::CONFLICT);
        assert!(html(last).await.contains("almeno un proprietario attivo"));
        let reader_raw = login_as(&service, "reader@example.com").await;
        let reader = service.session(&reader_raw).await.unwrap().unwrap();
        let reader_cookie = format!("talocart_session={reader_raw}");
        for partial in [false, true] {
            for path in [
                "/admin/users",
                "/admin/users/new",
                &format!("/admin/users/{owner_id}"),
            ] {
                assert_eq!(
                    request(&app, "GET", path, &reader_cookie, "", partial)
                        .await
                        .status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
        let body =
            html(request(&app, "GET", "/admin/settings", &reader_cookie, "", false).await).await;
        assert!(body.contains("sola lettura"));
        // Read-only help may name the save action; the action itself is absent.
        assert!(!body.contains("id=\"settings-save\""));
        assert!(body.contains("data-settings-write=\"false\""));
        assert!(!body.contains("href=\"/admin/users\""));
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings",
                &reader_cookie,
                &format!("csrf={}&name=Denied", reader.csrf),
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            service.settings().await.unwrap().unwrap().values.name,
            "Changed"
        );
        assert_eq!(
            request(
                &app,
                "POST",
                &format!("/admin/users/{owner_id}"),
                &reader_cookie,
                &format!("csrf={}&role=viewer&active=false", reader.csrf),
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
                "/admin/users/9223372036854775807",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        // A legitimate self-demotion must redirect to login after revoking its session.
        service
            .create_administrator(
                &owner,
                &owner.csrf,
                "second@example.com",
                "correct horse battery".into(),
                Role::Owner,
            )
            .await
            .unwrap();
        let response = request(
            &app,
            "POST",
            &format!("/admin/users/{owner_id}"),
            &cookie,
            &format!("csrf={}&role=viewer&active=true", owner.csrf),
            false,
        )
        .await;
        assert_eq!(response.headers()[header::LOCATION], "/admin/login");
        assert!(service.session(&raw).await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn configurable_roles_crud_permissions_and_protected_owner() {
    use talocart_platform::identity::{PERMISSIONS, Permission, Role};
    isolated(|pool,_| async move {
        let service=installed(&pool).await;
        let raw=authenticated(&service).await;let owner=service.session(&raw).await.unwrap().unwrap();
        for permission in PERMISSIONS {assert!(owner.allows(permission));}
        assert!(matches!(service.update_role(&owner,&owner.csrf,"owner","Hacked",&[]).await,Err(IdentityError::ProtectedRole)));
        assert!(matches!(service.delete_role(&owner,&owner.csrf,"owner").await,Err(IdentityError::ProtectedRole)));
        for query in ["UPDATE admin_roles SET name='Changed' WHERE key='owner'","DELETE FROM admin_roles WHERE key='owner'","INSERT INTO admin_role_permissions(role_key,permission) VALUES ('owner','settings.read')"] {
            let error=sqlx::query(query).execute(&pool).await.unwrap_err();
            assert_eq!(error.as_database_error().unwrap().code().as_deref(),Some("23514"));
        }
        let role=service.role(&owner,"owner").await.unwrap();assert_eq!(role.name,"Proprietario");assert!(role.protected());
        for permission in PERMISSIONS {assert!(role.allows(permission));}
        assert!(matches!(service.create_role(&owner,"wrong","Editor",&[]).await,Err(IdentityError::Forbidden)));
        assert!(matches!(service.create_role(&owner,&owner.csrf,"  ",&[]).await,Err(IdentityError::Invalid)));
        let key=service.create_role(&owner,&owner.csrf,"Editor",&[Permission::SettingsWrite]).await.unwrap();
        let role=service.role(&owner,&key).await.unwrap();assert!(role.allows(Permission::SettingsRead));assert!(role.allows(Permission::SettingsWrite));assert!(!role.allows(Permission::UsersWrite));
        assert!(matches!(service.create_role(&owner,&owner.csrf,"editor",&[]).await,Err(IdentityError::DuplicateRole)));
        let id=service.create_administrator(&owner,&owner.csrf,"editor@example.com","correct horse battery".into(),Role::parse(&key).unwrap()).await.unwrap();
        let editor_raw=login_as(&service,"editor@example.com").await;let editor=service.session(&editor_raw).await.unwrap().unwrap();
        let app=router(test_app(pool.clone(),"Koala".into()).await);
        let cookie=format!("talocart_session={editor_raw}");
        let response=request(&app,"GET","/admin/settings",&cookie,"",false).await;assert_eq!(response.status(),StatusCode::OK);
        let body=html(response).await;assert!(body.contains("Salva impostazioni"));assert!(!body.contains("href=\"/admin/users\""));assert!(body.contains("class=\"wordmark\" href=\"/admin\""));assert!(!body.contains(">Panoramica</span></a>"));
        assert_eq!(request(&app,"GET","/admin",&cookie,"",false).await.status(),StatusCode::FORBIDDEN);
        assert!(service.save_settings(&editor,&editor.csrf,&talocart_platform::identity::SettingsInput {name: "Editor shop".into(), ..Default::default()}).await.is_ok());
        assert!(matches!(service.administrators(&editor,0).await,Err(IdentityError::Forbidden)));
        assert!(matches!(service.delete_role(&owner,&owner.csrf,&key).await,Err(IdentityError::RoleInUse)));
        service.update_role(&owner,&owner.csrf,&key,"Renamed",&[Permission::SettingsWrite]).await.unwrap();
        assert!(service.session(&editor_raw).await.unwrap().is_some(),"A rename must not revoke access");
        service.update_role(&owner,&owner.csrf,&key,"Read only",&[Permission::SettingsRead]).await.unwrap();
        assert!(service.session(&editor_raw).await.unwrap().is_none());
        assert!(matches!(service.save_settings(&editor,&editor.csrf,&talocart_platform::identity::SettingsInput {name: "Stale".into(), ..Default::default()}).await,Err(IdentityError::Forbidden)));
        let raw2=login_as(&service,"editor@example.com").await;let reader=service.session(&raw2).await.unwrap().unwrap();
        assert!(reader.allows(Permission::SettingsRead));assert!(!reader.allows(Permission::SettingsWrite));
        service.update_administrator(&owner,&owner.csrf,id,Role::Viewer,false).await.unwrap();
        service.delete_role(&owner,&owner.csrf,&key).await.unwrap();assert!(matches!(service.role(&owner,&key).await,Err(IdentityError::RoleNotFound)));
        service.update_role(&owner,&owner.csrf,"viewer","Lettore modificato",&[Permission::AdminRead,Permission::UsersRead]).await.unwrap();
        assert_eq!(service.role(&owner,"viewer").await.unwrap().name,"Lettore modificato");
        assert!(matches!(service.delete_role(&owner,&owner.csrf,"viewer").await,Err(IdentityError::RoleInUse)),"Inactive accounts still reference their role");
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn delegated_roles_cannot_escalate_or_manage_owners() {
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let owner = service.session(&raw).await.unwrap().unwrap();
        let rights = [
            Permission::AdminRead,
            Permission::SettingsRead,
            Permission::RolesWrite,
            Permission::UsersWrite,
        ];
        let key = service
            .create_role(&owner, &owner.csrf, "Delegato", &rights)
            .await
            .unwrap();
        let id = service
            .create_administrator(
                &owner,
                &owner.csrf,
                "delegate@example.com",
                "correct horse battery".into(),
                Role::parse(&key).unwrap(),
            )
            .await
            .unwrap();
        let delegate_raw = login_as(&service, "delegate@example.com").await;
        let delegate = service.session(&delegate_raw).await.unwrap().unwrap();
        assert!(matches!(
            service
                .create_role(
                    &delegate,
                    &delegate.csrf,
                    "Escalation",
                    &[Permission::SettingsWrite]
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .update_role(
                    &delegate,
                    &delegate.csrf,
                    &key,
                    "Escalation",
                    &[Permission::SettingsWrite]
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .create_administrator(
                    &delegate,
                    &delegate.csrf,
                    "owner2@example.com",
                    "correct horse battery".into(),
                    Role::Owner
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .update_administrator(&delegate, &delegate.csrf, id, Role::Owner, true)
                .await,
            Err(IdentityError::Forbidden)
        ));
        assert!(matches!(
            service
                .update_administrator(
                    &delegate,
                    &delegate.csrf,
                    owner.administrator_id.unwrap(),
                    Role::Viewer,
                    false
                )
                .await,
            Err(IdentityError::Forbidden)
        ));
        let child = service
            .create_role(
                &delegate,
                &delegate.csrf,
                "Consultazione",
                &[Permission::AdminRead],
            )
            .await
            .unwrap();
        service
            .update_role(
                &delegate,
                &delegate.csrf,
                &child,
                "Consultazione aggiornata",
                &[Permission::AdminRead],
            )
            .await
            .unwrap();
        service
            .delete_role(&delegate, &delegate.csrf, &child)
            .await
            .unwrap();
        let powerful = service
            .create_role(
                &owner,
                &owner.csrf,
                "Impostazioni complete",
                &[Permission::SettingsWrite],
            )
            .await
            .unwrap();
        assert!(matches!(
            service
                .delete_role(&delegate, &delegate.csrf, &powerful)
                .await,
            Err(IdentityError::Forbidden)
        ));
        service
            .update_role(
                &owner,
                &owner.csrf,
                &key,
                "Solo lettura",
                &[Permission::AdminRead],
            )
            .await
            .unwrap();
        assert!(matches!(
            service
                .create_role(&delegate, &delegate.csrf, "Stale", &[])
                .await,
            Err(IdentityError::Forbidden)
        ));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn role_http_crud_csrf_read_only_and_login_destination() {
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let service = installed(&pool).await;
        let raw = authenticated(&service).await;
        let owner = service.session(&raw).await.unwrap().unwrap();
        let app = router(test_app(pool.clone(), "Koala".into()).await);
        let cookie = format!("talocart_session={raw}");
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/roles/new",
                &cookie,
                "csrf=wrong&name=No",
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
                "/admin/roles/new",
                &cookie,
                &format!("csrf={}&name=%20", owner.csrf),
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
                "/admin/roles/new",
                &cookie,
                &format!("csrf={}&name=Supporto&settings_read=true", owner.csrf),
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        let roles = service.roles(&owner).await.unwrap();
        let key = roles
            .iter()
            .find(|r| r.name == "Supporto")
            .unwrap()
            .key
            .clone();
        let owner_page =
            html(request(&app, "GET", "/admin/roles/owner", &cookie, "", false).await).await;
        assert!(owner_page.contains("Ruolo protetto"));
        assert!(!owner_page.contains("Salva ruolo"));
        assert!(!owner_page.contains("action=\"/admin/roles/owner/delete\""));
        for path in ["/admin/roles/owner", "/admin/roles/owner/delete"] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    path,
                    &cookie,
                    &format!("csrf={}&name=Hack", owner.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::CONFLICT
            );
        }
        let role = Role::parse(&key).unwrap();
        service
            .create_administrator(
                &owner,
                &owner.csrf,
                "support@example.com",
                "correct horse battery".into(),
                role,
            )
            .await
            .unwrap();
        let response = request(&app, "GET", "/admin/login", "", "", false).await;
        let anon_cookie = cookie_from_response(&response);
        let token = csrf(&html(response).await);
        let response = request(
            &app,
            "POST",
            "/admin/login",
            &anon_cookie,
            &format!("csrf={token}&email=support%40example.com&password=correct+horse+battery"),
            false,
        )
        .await;
        assert_eq!(response.headers()[header::LOCATION], "/admin/settings");
        let support_cookie = cookie_from_response(&response);
        for partial in [false, true] {
            assert_eq!(
                request(&app, "GET", "/admin/roles", &support_cookie, "", partial)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                request(&app, "GET", "/admin/users", &support_cookie, "", partial)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings",
                &support_cookie,
                "csrf=wrong&name=Denied",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let role_path = format!("/admin/roles/{key}");
        assert_eq!(
            request(
                &app,
                "POST",
                &format!("{role_path}/delete"),
                &cookie,
                &format!("csrf={}", owner.csrf),
                false
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("{role_path}/delete"),
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let unused = service
            .create_role(&owner, &owner.csrf, "Vuoto", &[])
            .await
            .unwrap();
        service
            .create_administrator(
                &owner,
                &owner.csrf,
                "empty@example.com",
                "correct horse battery".into(),
                Role::parse(&unused).unwrap(),
            )
            .await
            .unwrap();
        let response = request(&app, "GET", "/admin/login", "", "", false).await;
        let anon_cookie = cookie_from_response(&response);
        let token = csrf(&html(response).await);
        let response = request(
            &app,
            "POST",
            "/admin/login",
            &anon_cookie,
            &format!("csrf={token}&email=empty%40example.com&password=correct+horse+battery"),
            false,
        )
        .await;
        assert_eq!(response.headers()[header::LOCATION], "/admin/access");
        let empty_cookie = cookie_from_response(&response);
        let page =
            html(request(&app, "GET", "/admin/access", &empty_cookie, "", false).await).await;
        assert!(page.contains("non ha ancora permessi"));
        // Read-only access to users/roles exposes data but never mutation controls.
        service
            .update_role(
                &owner,
                &owner.csrf,
                &unused,
                "Consultazione",
                &[Permission::UsersRead, Permission::RolesRead],
            )
            .await
            .unwrap();
        let empty_raw = login_as(&service, "empty@example.com").await;
        let read_cookie = format!("talocart_session={empty_raw}");
        let body = html(request(&app, "GET", "/admin/users", &read_cookie, "", false).await).await;
        assert!(!body.contains("Nuovo utente"));
        let body = html(request(&app, "GET", "/admin/roles", &read_cookie, "", false).await).await;
        assert!(!body.contains("Nuovo ruolo"));
        assert_eq!(
            request(&app, "GET", "/admin/users/new", &read_cookie, "", false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&app, "GET", "/admin/roles/new", &read_cookie, "", false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let body =
            html(request(&app, "GET", "/admin/roles/viewer", &read_cookie, "", false).await).await;
        assert!(!body.contains("Salva ruolo"));
        let detached = service
            .create_role(&owner, &owner.csrf, "Da eliminare", &[])
            .await
            .unwrap();
        assert_eq!(
            request(
                &app,
                "POST",
                &format!("/admin/roles/{detached}/delete"),
                &cookie,
                &format!("csrf={}", owner.csrf),
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn role_deletion_and_assignment_are_atomic() {
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool,_| async move {
        let service=installed(&pool).await;let raw=authenticated(&service).await;let owner=service.session(&raw).await.unwrap().unwrap();
        let key=service.create_role(&owner,&owner.csrf,"Concorrenza",&[Permission::AdminRead]).await.unwrap();
        let id=service.create_administrator(&owner,&owner.csrf,"race@example.com","correct horse battery".into(),Role::Viewer).await.unwrap();
        let other=test_identity(pool.clone()).await;
        let (deleted,assigned)=tokio::join!(service.delete_role(&owner,&owner.csrf,&key),other.update_administrator(&owner,&owner.csrf,id,Role::parse(&key).unwrap(),true));
        assert_eq!(usize::from(deleted.is_ok())+usize::from(assigned.is_ok()),1);
        assert!(matches!(deleted,Err(IdentityError::RoleInUse)) || matches!(assigned,Err(IdentityError::RoleNotFound)));
        let dangling:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators a LEFT JOIN admin_roles r ON r.key=a.role WHERE r.key IS NULL)").fetch_one(&pool).await.unwrap();assert!(!dangling);
    }).await;
}
