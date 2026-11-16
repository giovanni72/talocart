use super::*;

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn application_snapshot_allows_other_readers_of_the_same_identity_and_session() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let mut other_reader = pool.begin().await.expect("other reader");
        sqlx::query("SELECT id FROM identity_state WHERE id=1 FOR SHARE")
            .fetch_one(&mut *other_reader)
            .await
            .expect("shared identity guard");
        sqlx::query("SELECT a.id FROM administrators a JOIN admin_sessions s ON s.administrator_id=a.id WHERE s.token_hash=$1 FOR SHARE OF a,s")
            .bind(&session.token_hash)
            .fetch_one(&mut *other_reader)
            .await
            .expect("shared account/session guard");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            talocart_commerce::settings::snapshot(&pool, &identity, &session),
        )
        .await;
        other_reader.rollback().await.expect("release other reader");
        let snapshot = result
            .expect("settings readers must coexist without exclusive lock upgrades")
            .expect("authorized snapshot");
        assert_eq!(snapshot.store.values.name, "Shop");
        assert!(snapshot.preferences.is_some());
        assert!(!snapshot.time_zones.is_empty());
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn settings_read_guards_exclude_writers_and_logout_until_released() {
    use talocart_platform::identity::Permission;
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let mut first = pool.begin().await.unwrap();
        Identity::authorize_read_transaction(&mut first, &session, Permission::SettingsRead)
            .await
            .expect("first reader");
        let mut second = pool.begin().await.unwrap();
        Identity::authorize_read_transaction(&mut second, &session, Permission::SettingsRead)
            .await
            .expect("second reader of the same session");
        let mut writer = pool.begin().await.unwrap();
        {
            let write = Identity::authorize_transaction(
                &mut writer,
                &session,
                &session.csrf,
                Permission::SettingsWrite,
            );
            tokio::pin!(write);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), &mut write)
                    .await
                    .is_err()
            );
            first.rollback().await.unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), &mut write)
                    .await
                    .is_err(),
                "writer must wait for every reader"
            );
            second.rollback().await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), &mut write)
                .await
                .expect("writer proceeds after both readers")
                .expect("authorized writer");
        }
        writer.rollback().await.unwrap();

        let mut reader = pool.begin().await.unwrap();
        Identity::authorize_read_transaction(&mut reader, &session, Permission::SettingsRead)
            .await
            .unwrap();
        let logout = identity.logout(&session, &session.csrf);
        tokio::pin!(logout);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut logout)
                .await
                .is_err(),
            "logout cannot revoke a session during a guarded read"
        );
        reader.rollback().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), &mut logout)
            .await
            .expect("logout proceeds after the read")
            .expect("logout");
        assert!(
            matches!(
                talocart_commerce::settings::snapshot(&pool, &identity, &session).await,
                Err(talocart_commerce::settings::ReadError::Access(_))
            ),
            "a captured session cannot read after logout"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn settings_snapshot_rechecks_current_permissions_account_and_expiry() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        // Keep the captured owner session deliberately stale to exercise the
        // service boundary independently of the HTTP session middleware.
        for change in [
            "UPDATE administrators SET role='viewer'", // no TaxesRead
            "UPDATE administrators SET role='viewer'; DELETE FROM admin_role_permissions WHERE role_key='viewer' AND permission='settings.read'",
            "UPDATE administrators SET active=false",
            "UPDATE admin_sessions SET expires_at=now()-interval '1 second'",
        ] {
            sqlx::raw_sql(change).execute(&pool).await.unwrap();
            assert!(matches!(
                talocart_commerce::settings::snapshot(&pool, &identity, &session).await,
                Err(talocart_commerce::settings::ReadError::Access(_))
            ), "stale permissions/session must be rejected: {change}");
            sqlx::raw_sql("UPDATE administrators SET role='owner',active=true; UPDATE admin_sessions SET expires_at=now()+interval '1 hour'")
                .execute(&pool).await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn application_snapshot_uses_only_one_pool_connection() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let logo_key = format!("{}.png", "a".repeat(64));
        sqlx::query("INSERT INTO media_objects(key,content_type,byte_size,width,height,public) VALUES($1,'image/png',100,16,16,true)")
            .bind(&logo_key).execute(&pool).await.unwrap();
        sqlx::query("UPDATE store_settings SET logo_key=$1 WHERE id=1")
            .bind(&logo_key).execute(&pool).await.unwrap();
        let mut reserved = Vec::new();
        for _ in 1..pool.options().get_max_connections() {
            reserved.push(pool.acquire().await.expect("reserve other connections"));
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            talocart_commerce::settings::snapshot(&pool, &identity, &session),
        )
        .await;
        drop(reserved);
        assert!(
            result.is_ok(),
            "snapshot must finish with one available connection"
        );
        let snapshot = result.unwrap().expect("snapshot");
        assert_eq!(snapshot.store.values.name, "Shop");
        assert_eq!(snapshot.logo.expect("logo metadata").key, logo_key);
        assert!(snapshot.preferences.is_some());
        assert!(!snapshot.time_zones.is_empty());
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn all_settings_share_one_native_form_and_branding_is_deferred() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let app = router(test_app(pool, "Shop".into()).await);
        let page = html(
            request(
                &app,
                "GET",
                "/admin/settings",
                &format!("talocart_session={raw}"),
                "",
                false,
            )
            .await,
        )
        .await;
        assert_eq!(page.matches("id=\"settings-editor\"").count(), 1);
        for id in [
            "store-name",
            "platform-time-zone",
            "theme-palette",
            "store-currency",
            "checkout-countries",
            "settings-logo",
            "settings-favicon",
        ] {
            assert!(page.contains(&format!("id=\"{id}\"")), "Missing {id}");
        }
        assert!(!page.contains("action=\"/admin/settings/date-time\""));
        assert!(page.contains("/assets/settings.js"));
    })
    .await;
}

fn attr(page: &str, marker: &str) -> String {
    page.split(marker)
        .nth(1)
        .expect("attribute exists")
        .split('"')
        .next()
        .unwrap()
        .replace("&#38;", "&")
        .replace("&amp;", "&")
        .replace("&#60;", "<")
        .replace("&#62;", ">")
        .replace("&#34;", "\"")
        .replace("&#39;", "'")
}
pub(super) async fn form(app: &Router, auth: &str) -> String {
    let page = html(request(app, "GET", "/admin/settings", auth, "", false).await).await;
    let mut body = attr(&page, "data-baseline=\"");
    for key in [
        "csrf",
        "settings_revision",
        "regional_revision",
        "fiscal_revision",
    ] {
        body.push_str(&format!(
            "&{key}={}",
            attr(&page, &format!("name=\"{key}\" value=\""))
        ));
    }
    body
}
pub(super) fn set(body: &str, key: &str, value: &str) -> String {
    let mut pairs: Vec<_> = body
        .split('&')
        .filter(|p| p.split('=').next() != Some(key))
        .map(str::to_owned)
        .collect();
    pairs.push(format!("{key}={value}"));
    pairs.join("&")
}
pub(super) async fn snapshot(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT jsonb_build_array((SELECT to_jsonb(s) FROM store_settings s),(SELECT to_jsonb(s) FROM storefront_settings s),(SELECT jsonb_agg(t ORDER BY locale) FROM storefront_settings_translations t),(SELECT jsonb_agg(l ORDER BY code) FROM storefront_languages l),(SELECT to_jsonb(r) FROM platform_regional_settings r),(SELECT to_jsonb(p) FROM pricing_settings p),(SELECT to_jsonb(t) FROM tax_configuration t),(SELECT jsonb_agg(c ORDER BY country_code) FROM checkout_countries c))::text").fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn one_commit_validates_every_area_and_preserves_all_values_on_errors() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let auth = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let original = snapshot(&pool).await;
        let body = form(&app, &auth).await;
        let body = set(&body, "indexing_enabled", "true");
        let edited = set(
            &set(&set(&body, "name", "Unified+shop"), "time_zone", "UTC"),
            "palette",
            "clay",
        );
        for (key, bad, status) in [
            ("contact_email", "invalid", StatusCode::UNPROCESSABLE_ENTITY),
            ("time_zone", "Bad%2FZone", StatusCode::UNPROCESSABLE_ENTITY),
            (
                "public_origin",
                "https%3A%2F%2Fbad.test%2Fpath",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            ("currency", "ZZZ", StatusCode::UNPROCESSABLE_ENTITY),
            ("purchase_country", "ZZ", StatusCode::UNPROCESSABLE_ENTITY),
            ("logo", "9999999", StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            let invalid = set(&edited, key, bad);
            let r = request(&app, "POST", "/admin/settings/save", &auth, &invalid, false).await;
            assert_eq!(r.status(), status, "{key}");
            let page = html(r).await;
            assert!(page.contains("Unified shop"), "Values retained for {key}");
            assert_eq!(snapshot(&pool).await, original, "No partial save: {key}");
        }
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/save",
                &auth,
                &set(&edited, "csrf", "wrong"),
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let r = request(
            &app,
            "POST",
            "/admin/settings/save",
            &auth,
            &set(&edited, "purchase_country", "IT"),
            false,
        )
        .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            identity.settings().await.unwrap().unwrap().values.name,
            "Unified shop"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT time_zone FROM platform_regional_settings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "UTC"
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT palette FROM storefront_settings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "clay"
        );
        assert!(
            talocart_platform::storefront::Storefront::new(pool.clone())
                .settings()
                .await
                .unwrap()
                .indexing_enabled
        );
        let saved = snapshot(&pool).await;
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &auth, &edited, false)
                .await
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(snapshot(&pool).await, saved);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn competing_editors_never_overwrite_and_media_picker_never_commits_settings() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let auth = format!("talocart_session={raw}");
        let media = test_media(&pool).await;
        let library = talocart_platform::media::library::MediaLibrary::new(
            pool.clone(),
            identity.clone(),
            media.clone(),
        );
        let id = library
            .upload(
                &s,
                &s.csrf,
                None,
                include_bytes!("../fixtures/avatar.png").to_vec(),
                "choice.png",
                false,
            )
            .await
            .unwrap();
        let app = router(
            test_app_config(
                pool.clone(),
                "Shop".into(),
                talocart_web::AppStateConfig {
                    media,
                    ..Default::default()
                },
            )
            .await,
        );
        let original = snapshot(&pool).await;
        let body = set(&form(&app, &auth).await, "name", "Pending");
        let browse = request(
            &app,
            "POST",
            "/admin/settings/save",
            &auth,
            &format!("{body}&action=browse_logo"),
            false,
        )
        .await;
        assert_eq!(browse.status(), StatusCode::OK);
        assert!(html(browse).await.contains("choice.png"));
        assert_eq!(snapshot(&pool).await, original);
        let choose = request(
            &app,
            "POST",
            "/admin/settings/save",
            &auth,
            &format!("{body}&action=choose_{id}&media_target=logo"),
            false,
        )
        .await;
        assert_eq!(choose.status(), StatusCode::OK);
        let page = html(choose).await;
        assert!(page.contains(&format!("name=\"logo\" value=\"{id}\"")));
        assert_eq!(snapshot(&pool).await, original);
        let a = set(
            &set(&body, "logo", &id.to_string()),
            "favicon",
            &id.to_string(),
        );
        let b = set(&body, "name", "Other+editor");
        let (a, b) = tokio::join!(
            request(&app, "POST", "/admin/settings/save", &auth, &a, false),
            request(&app, "POST", "/admin/settings/save", &auth, &b, false)
        );
        assert_eq!(
            [a.status(), b.status()]
                .iter()
                .filter(|s| **s == StatusCode::SEE_OTHER)
                .count(),
            1
        );
        assert_eq!(
            [a.status(), b.status()]
                .iter()
                .filter(|s| **s == StatusCode::CONFLICT)
                .count(),
            1
        );
        // Select both images in one confirmed transaction, then remove them together.
        let body = set(
            &set(&form(&app, &auth).await, "logo", &id.to_string()),
            "favicon",
            &id.to_string(),
        );
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &auth, &body, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        let branding = identity.branding().await.unwrap().unwrap();
        assert!(branding.has_logo && branding.has_favicon);
        let body = set(
            &set(&form(&app, &auth).await, "logo", "remove"),
            "favicon",
            "remove",
        );
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &auth, &body, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        let branding = identity.branding().await.unwrap().unwrap();
        assert!(!branding.has_logo && !branding.has_favicon);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn settings_permissions_do_not_grant_tax_or_private_media_access_and_revocation_wins() {
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let owner = identity.session(&raw).await.unwrap().unwrap();
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Settings editor",
                &[Permission::SettingsRead, Permission::SettingsWrite],
            )
            .await
            .unwrap();
        let id = identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "editor@example.test",
                "correct horse battery".into(),
                Role::Viewer,
            )
            .await
            .unwrap();
        identity
            .update_administrator(&owner, &owner.csrf, id, Role::Custom(role.clone()), true)
            .await
            .unwrap();
        let editor_raw = login_as(&identity, "editor@example.test").await;
        let auth = format!("talocart_session={editor_raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        assert_eq!(
            request(&app, "GET", "/admin/settings/taxes", &auth, "", false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let body = form(&app, &auth).await;
        let attempt = set(&set(&body, "name", "Authorized"), "currency", "USD");
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &auth, &attempt, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(identity.settings().await.unwrap().unwrap().currency, "EUR");
        assert_eq!(
            identity.settings().await.unwrap().unwrap().values.name,
            "Authorized"
        );
        let original = snapshot(&pool).await;
        let body = form(&app, &auth).await;
        let denied = set(&body, "logo", "123");
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &auth, &denied, false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(snapshot(&pool).await, original);
        let denied = format!("{body}&action=browse_logo");
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &auth, &denied, false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        identity
            .update_administrator(&owner, &owner.csrf, id, Role::Custom(role), false)
            .await
            .unwrap();
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/save",
                &auth,
                &set(&body, "name", "Revoked"),
                true
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(snapshot(&pool).await, original);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn tax_writers_with_general_read_access_can_save_only_their_writable_area() {
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let owner = identity.session(&raw).await.unwrap().unwrap();
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Tax editor",
                &[Permission::SettingsRead, Permission::TaxesWrite],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "tax-editor@example.test",
                "correct horse battery".into(),
                Role::Custom(role),
            )
            .await
            .unwrap();
        let raw = login_as(&identity, "tax-editor@example.test").await;
        let auth = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let page =
            html(request(&app, "GET", "/admin/settings/taxes", &auth, "", false).await).await;
        assert!(
            page.contains("id=\"settings-save\""),
            "Tax writing must remain available"
        );
        let body = set(
            &set(&form(&app, &auth).await, "currency", "USD"),
            "name",
            "Unauthorized+name",
        );
        let response = request(&app, "POST", "/admin/settings/save", &auth, &body, false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let store = identity.settings().await.unwrap().unwrap();
        assert_eq!(store.currency, "USD");
        assert_eq!(store.values.name, "Shop");
        let body = set(&form(&app, &auth).await, "currency", "ZZZ");
        let response = request(&app, "POST", "/admin/settings/save", &auth, &body, false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let page = html(response).await;
        assert!(page.contains("value=\"ZZZ\" selected"));
        assert!(page.contains("Shop"));
        assert_eq!(identity.settings().await.unwrap().unwrap().currency, "USD");
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn stale_tax_class_selection_survives_another_editor_removing_or_disabling_it() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let auth = format!("talocart_session={raw}");
        let rate: i64 = sqlx::query_scalar(
            "INSERT INTO tax_rates(name,basis_points) VALUES('Draft VAT',2200) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for removed in [false, true] {
            let class: i64 = sqlx::query_scalar(
                "INSERT INTO tax_classes(name,rate_id) VALUES($1,$2) RETURNING id",
            )
            .bind(format!("Draft class {removed}"))
            .bind(rate)
            .fetch_one(&pool)
            .await
            .unwrap();
            let body = set(
                &form(&app, &auth).await,
                "default_class_id",
                &class.to_string(),
            );
            let mutation = if removed {
                "DELETE FROM tax_classes WHERE id=$1"
            } else {
                "UPDATE tax_classes SET active=false WHERE id=$1"
            };
            sqlx::query(mutation)
                .bind(class)
                .execute(&pool)
                .await
                .unwrap();
            let before = snapshot(&pool).await;
            let response = request(&app, "POST", "/admin/settings/save", &auth, &body, false).await;
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let page = html(response).await;
            assert!(
                page.contains(&format!("<option value=\"{class}\" selected>")),
                "The obsolete selection must remain a successful form control, not become None"
            );
            assert_eq!(snapshot(&pool).await, before);
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn application_snapshot_waits_for_writer_and_releases_guard_before_return() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity
            .session(&raw)
            .await
            .expect("session lookup")
            .expect("authenticated owner");
        let mut writer = pool.begin().await.expect("writer");
        Identity::authorize_transaction(
            &mut writer,
            &session,
            &session.csrf,
            talocart_platform::identity::Permission::SettingsWrite,
        )
        .await
        .expect("writer lock");
        sqlx::query("UPDATE store_settings SET name='Committed snapshot' WHERE id=1")
            .execute(&mut *writer)
            .await
            .expect("pending change");
        let read = talocart_commerce::settings::snapshot(&pool, &identity, &session);
        tokio::pin!(read);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut read)
                .await
                .is_err(),
            "snapshot must wait for the writer"
        );
        writer.commit().await.expect("commit writer");
        let snapshot = read.await.expect("coherent snapshot");
        assert_eq!(snapshot.store.values.name, "Committed snapshot");
        let mut guard = pool.begin().await.expect("subsequent writer");
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Identity::authorize_transaction(
                &mut guard,
                &session,
                &session.csrf,
                talocart_platform::identity::Permission::SettingsWrite,
            ),
        )
        .await
        .expect("snapshot guard already released")
        .expect("authorized writer");
        assert_eq!(
            snapshot.revision,
            talocart_platform::settings::revision(&mut guard)
                .await
                .expect("revision matches values")
        );
        guard.rollback().await.expect("release writer");
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn module_mode_and_maintenance_share_atomic_save_and_require_owner_password() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let original = form(&app, &cookie).await;
        let changed = set(
            &set(&original, "module_install_mode", "panel"),
            "maintenance_enabled",
            "true",
        );
        let bad = set(
            &changed,
            "module_install_password",
            "wrong-password-not-returned",
        );
        let before = snapshot(&pool).await;
        let response = request(&app, "POST", "/admin/settings/save", &cookie, &bad, false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!html(response).await.contains("wrong-password-not-returned"));
        assert_eq!(
            snapshot(&pool).await,
            before,
            "Failed reauthentication must not enable maintenance or change any setting"
        );
        let picker = set(&bad, "action", "browse_logo");
        let response = request(
            &app,
            "POST",
            "/admin/settings/save",
            &cookie,
            &picker,
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !html(response).await.contains("wrong-password-not-returned"),
            "Passwords must not enter hidden media drafts"
        );
        let valid = set(&changed, "module_install_password", "correct+horse+battery");
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &cookie, &valid, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT maintenance_enabled FROM storefront_settings")
                .fetch_one(&pool)
                .await
                .unwrap()
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT module_install_mode FROM store_settings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "panel"
        );
        assert_eq!(
            request(&app, "GET", "/", "", "", false).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        // A stale whole-form submission must not revert the newly saved operational settings.
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/save",
                &cookie,
                &set(
                    &original,
                    "module_install_password",
                    "correct+horse+battery"
                ),
                false
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
        let mut body = form(&app, &cookie).await;
        body = body
            .split('&')
            .filter(|p| !p.starts_with("maintenance_enabled="))
            .collect::<Vec<_>>()
            .join("&");
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &cookie, &body, false)
                .await
                .status(),
            StatusCode::SEE_OTHER,
            "Unchanged mode needs no password; unchecked maintenance is saved with the rest"
        );
        assert!(
            !sqlx::query_scalar::<_, bool>("SELECT maintenance_enabled FROM storefront_settings")
                .fetch_one(&pool)
                .await
                .unwrap()
        );
        // The same running router observes policy changes immediately.
        let package_form = format!("csrf={}&preview=invalid&password=wrong", session.csrf);
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/modules/install",
                &cookie,
                &package_form,
                false
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let body = set(
            &set(&form(&app, &cookie).await, "module_install_mode", "cli"),
            "module_install_password",
            "correct+horse+battery",
        );
        assert_eq!(
            request(&app, "POST", "/admin/settings/save", &cookie, &body, false)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/modules/install",
                &cookie,
                &package_form,
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let role = identity
            .create_role(
                &session,
                &session.csrf,
                "Settings editor",
                &[
                    talocart_platform::identity::Permission::SettingsRead,
                    talocart_platform::identity::Permission::SettingsWrite,
                ],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &session,
                &session.csrf,
                "settings-editor@example.test",
                "correct horse battery".into(),
                talocart_platform::identity::Role::Custom(role),
            )
            .await
            .unwrap();
        let editor_cookie = format!(
            "talocart_session={}",
            login_as(&identity, "settings-editor@example.test").await
        );
        let page =
            html(request(&app, "GET", "/admin/settings", &editor_cookie, "", false).await).await;
        assert!(!page.contains("id=\"module-install-mode\""));
        let crafted = set(
            &set(
                &form(&app, &editor_cookie).await,
                "module_install_mode",
                "panel",
            ),
            "module_install_password",
            "correct+horse+battery",
        );
        let before = snapshot(&pool).await;
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/settings/save",
                &editor_cookie,
                &crafted,
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(snapshot(&pool).await, before);
        let restarted = router(test_app(pool.clone(), "Shop".into()).await);
        assert_eq!(
            request(
                &restarted,
                "POST",
                "/admin/modules/install",
                &cookie,
                &package_form,
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    })
    .await;
}
