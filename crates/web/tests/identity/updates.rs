use super::*;
use talocart_platform::identity::{IdentityError, Role};
use talocart_platform::updates::{Candidate, Updates};

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn owner_only_updates_check_csrf_current_identity_and_publish_complete_requests() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let root = std::env::temp_dir().join(format!("talocart-update-http-{}-{}", std::process::id(), session.administrator_id.unwrap()));
        tokio::fs::create_dir_all(root.join("requests")).await.unwrap();
        let candidate = Candidate { version: "999.0.0".into(), commit: "a".repeat(40), digest: "b".repeat(64) };
        tokio::fs::write(root.join("status.json"), serde_json::to_vec(&serde_json::json!({
            "format":1,"phase":"available","checked_at":"2026-10-31T10:00:00+00:00","candidate": candidate
        })).unwrap()).await.unwrap();
        let updates = Updates::new(Some(root.clone()));
        let app = router(test_app(pool.clone(), "Shop".into()).await.with_updates(updates.clone()));
        for hx in [false, true] {
            let response = request(&app, "GET", "/admin/updates", &cookie, "", hx).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers()[header::CACHE_CONTROL].to_str().unwrap().contains("no-store"));
            assert!(html(response).await.contains("999.0.0"));
            let form = format!("csrf=wrong&version=999.0.0&commit={}&digest={}&confirm=yes", candidate.commit,candidate.digest);
            assert_eq!(request(&app,"POST","/admin/updates",&cookie,&form,hx).await.status(),StatusCode::FORBIDDEN);
        }
        assert!(!root.join("requests/install.json").exists());
        let form = format!("csrf={}&version=999.0.0&commit={}&digest={}&confirm=yes",session.csrf,candidate.commit,candidate.digest);
        let mut changed = candidate.clone(); changed.digest = "c".repeat(64);
        assert!(matches!(updates.request(&identity,&session,changed).await,Err(IdentityError::Invalid)));
        assert_eq!(request(&app,"POST","/admin/updates",&cookie,&form,false).await.status(),StatusCode::SEE_OTHER);
        assert_eq!(serde_json::from_slice::<Candidate>(&tokio::fs::read(root.join("requests/install.json")).await.unwrap()).unwrap().digest,candidate.digest);
        assert!(matches!(updates.request(&identity,&session,candidate.clone()).await,Err(IdentityError::Busy)));
        identity.create_administrator(&session,&session.csrf,"viewer@example.test","correct horse battery".into(),Role::Viewer).await.unwrap();
        let viewer_raw = login_as(&identity,"viewer@example.test").await;
        let viewer = identity.session(&viewer_raw).await.unwrap().unwrap();
        assert!(matches!(updates.read(&identity,&viewer).await,Err(IdentityError::Forbidden)));
        for hx in [false,true] {
            assert_ne!(request(&app,"GET","/admin/updates",&format!("talocart_session={viewer_raw}"),"",hx).await.status(),StatusCode::OK);
        }
        tokio::fs::remove_dir_all(root).await.unwrap();
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn unified_update_plan_blocks_conflicts_and_recovery_but_allows_safe_retry() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        sqlx::query("UPDATE administrators SET locale='en' WHERE id=$1")
            .bind(session.administrator_id).execute(&pool).await.unwrap();
        let root = std::env::temp_dir().join(format!("talocart-unified-http-{}-{}", std::process::id(), session.administrator_id.unwrap()));
        tokio::fs::create_dir_all(root.join("requests")).await.unwrap();
        let candidate = Candidate { version: "999.0.0".into(), commit: "a".repeat(40), digest: "b".repeat(64) };
        let updates = Updates::new(Some(root.clone()));
        let app = router(test_app(pool.clone(), "Shop".into()).await.with_updates(updates.clone()));
        let mut status = serde_json::json!({"format":2,"phase":"available","checked_at":"2026-11-01T12:00:00+00:00","candidate":candidate,
            "plan":{"release":"v999.0.0","sequence":1,"base_from":"11.11.0","base_to":"999.0.0","base_action":"update","blocked":false,
                "modules":[{"id":"koala-import","from":"1.0.0","to":"2.0.0","enabled":false,"action":"update","blocker":null}]},"recovery_required":false});
        let write = |value: serde_json::Value| { let path=root.join("status.json"); async move { tokio::fs::write(path,serde_json::to_vec(&value).unwrap()).await.unwrap(); } };
        write(status.clone()).await;
        let cookie=format!("talocart_session={raw}");
        let body=html(request(&app,"GET","/admin/updates",&cookie,"",false).await).await;
        assert!(body.contains("koala-import") && body.contains("Stays disabled"));
        assert!(body.contains("name=\"digest\""));
        status["phase"]="blocked".into(); status["plan"]["blocked"]=true.into();
        status["plan"]["modules"][0]["action"]="blocked".into(); status["plan"]["modules"][0]["blocker"]="incompatible".into();
        write(status.clone()).await;
        assert!(matches!(updates.request(&identity,&session,candidate.clone()).await,Err(IdentityError::Invalid)));
        let body=html(request(&app,"GET","/admin/updates",&cookie,"",false).await).await;
        assert!(!body.contains("name=\"digest\""));
        assert!(body.contains("Incompatible with the proposed base"));
        status["plan"]["modules"][0]["action"]="update".into(); status["plan"]["modules"][0]["blocker"]=serde_json::Value::Null;
        status["notice"]="engine_failed".into(); status["candidate"]=serde_json::Value::Null;
        write(status.clone()).await;
        assert!(matches!(updates.request(&identity,&session,candidate.clone()).await,Err(IdentityError::Invalid)));
        let body=html(request(&app,"GET","/admin/updates",&cookie,"",false).await).await;
        assert!(body.contains("The update service failed its self-check."));
        assert!(!body.contains("name=\"digest\""));
        status["notice"]=serde_json::Value::Null; status["candidate"]=serde_json::to_value(&candidate).unwrap();
        status["phase"]="failed".into(); status["plan"]["blocked"]=false.into();
        status["plan"]["modules"][0]["action"]="update".into(); status["plan"]["modules"][0]["blocker"]=serde_json::Value::Null;
        write(status.clone()).await;
        updates.request(&identity,&session,candidate.clone()).await.unwrap();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(tokio::fs::metadata(root.join("requests/install.json")).await.unwrap().permissions().mode() & 0o777,0o600);
        }
        tokio::fs::remove_file(root.join("requests/install.json")).await.unwrap();
        status["recovery_required"]=true.into(); write(status).await;
        assert!(matches!(updates.request(&identity,&session,candidate).await,Err(IdentityError::Invalid)));
        let body=html(request(&app,"GET","/admin/updates",&cookie,"",false).await).await;
        assert!(!body.contains("name=\"digest\""));
        assert!(body.contains("Server intervention required"));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn owner_manual_check_requires_csrf_and_never_competes_with_installs() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        sqlx::query("UPDATE administrators SET locale='en' WHERE id=$1")
            .bind(session.administrator_id)
            .execute(&pool)
            .await
            .unwrap();
        let cookie = format!("talocart_session={raw}");
        let root = std::env::temp_dir().join(format!(
            "talocart-update-check-{}-{}",
            std::process::id(),
            session.administrator_id.unwrap()
        ));
        tokio::fs::create_dir_all(root.join("requests"))
            .await
            .unwrap();
        tokio::fs::write(root.join("status.json"), serde_json::to_vec(&serde_json::json!({
            "format":1,"phase":"current","checked_at":"2026-11-01T10:00:00+00:00","candidate":null
        })).unwrap()).await.unwrap();
        let updates = Updates::new(Some(root.clone()));
        let app = router(
            test_app(pool.clone(), "Shop".into())
                .await
                .with_updates(updates.clone()),
        );
        let marker = root.join("requests/check.json");
        let body = html(request(&app, "GET", "/admin/updates", &cookie, "", false).await).await;
        assert!(body.contains("action=\"/admin/updates/check\"") && body.contains("Check now"));
        for hx in [false, true] {
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/admin/updates/check",
                    &cookie,
                    "csrf=wrong",
                    hx
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                request(&app, "GET", "/admin/updates/check", &cookie, "", hx)
                    .await
                    .status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
        }
        assert!(!marker.exists());
        let form = format!("csrf={}", session.csrf);
        assert_eq!(
            request(&app, "POST", "/admin/updates/check", &cookie, &form, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(&marker)
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let body = html(request(&app, "GET", "/admin/updates", &cookie, "", false).await).await;
        assert!(
            body.contains("Check requested") && !body.contains("action=\"/admin/updates/check\"")
        );
        updates.request_check(&identity, &session).await.unwrap();
        tokio::fs::remove_file(&marker).await.unwrap();
        tokio::fs::write(root.join("requests/install.json"), b"{}")
            .await
            .unwrap();
        assert!(matches!(
            updates.request_check(&identity, &session).await,
            Err(IdentityError::Invalid)
        ));
        assert!(
            !html(request(&app, "GET", "/admin/updates", &cookie, "", false).await)
                .await
                .contains("action=\"/admin/updates/check\"")
        );
        tokio::fs::remove_file(root.join("requests/install.json"))
            .await
            .unwrap();
        identity
            .create_administrator(
                &session,
                &session.csrf,
                "viewer@example.test",
                "correct horse battery".into(),
                Role::Viewer,
            )
            .await
            .unwrap();
        let viewer_raw = login_as(&identity, "viewer@example.test").await;
        let viewer = identity.session(&viewer_raw).await.unwrap().unwrap();
        assert!(matches!(
            updates.request_check(&identity, &viewer).await,
            Err(IdentityError::Forbidden)
        ));
        let viewer_form = format!("csrf={}", viewer.csrf);
        assert_ne!(
            request(
                &app,
                "POST",
                "/admin/updates/check",
                &format!("talocart_session={viewer_raw}"),
                &viewer_form,
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        assert!(!marker.exists());
        tokio::fs::remove_dir_all(root).await.unwrap();
    })
    .await;
}
