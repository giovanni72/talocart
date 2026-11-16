use super::*;
use talocart_platform::identity::{
    CustomerProfile, Customers, Permission,
    customer_groups::{CustomerGroups, GroupError, GroupInput},
};
fn input() -> GroupInput {
    GroupInput {
        name_it: "Rivenditori".into(),
        name_en: "Resellers".into(),
        is_default: false,
        ..GroupInput::default()
    }
}
async fn register(customers: &Customers, email: &str) -> i64 {
    let (_, s) = customers.anonymous().await.unwrap();
    let raw = customers
        .register(
            &s,
            &s.csrf,
            &CustomerProfile {
                email: email.into(),
                first_name: "Alex".into(),
                last_name: "Buyer".into(),
                customer_type: "private".into(),
            },
            "synthetic password".into(),
            None,
        )
        .await
        .unwrap()
        .expect("Immediate activation");
    customers
        .session(&raw)
        .await
        .unwrap()
        .unwrap()
        .customer_id()
        .unwrap()
}
async fn group_of(pool: &PgPool, id: i64) -> i64 {
    sqlx::query_scalar("SELECT group_id FROM customers WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn groups_crud_default_registration_assignment_and_protected_deletion() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await; let s = taxonomy_session(&identity).await;
        let groups = CustomerGroups::new(pool.clone(), identity.clone());
        let customers = Customers::new(pool.clone(), identity);
        let a = register(&customers, "a@example.test").await; assert_eq!(group_of(&pool,a).await,1);
        let rev = groups.editor(&s,None).await.unwrap().0;
        let id = groups.save(&s,&s.csrf,None,rev,&input()).await.unwrap();
        assert!(matches!(groups.save(&s,&s.csrf,Some(id),rev,&input()).await,Err(GroupError::Stale)));
        let rev = groups.editor(&s,None).await.unwrap().0;
        assert!(matches!(groups.save(&s,&s.csrf,None,rev,&input()).await,Err(GroupError::Duplicate)));
        assert_eq!(groups.editor(&s,None).await.unwrap().0,rev);
        assert!(matches!(groups.delete(&s,&s.csrf,1,rev).await,Err(GroupError::InUse)));
        let mut changed = input(); changed.is_default = true;
        groups.save(&s,&s.csrf,Some(id),rev,&changed).await.unwrap();
        let b = register(&customers, "b@example.test").await;
        assert_eq!(group_of(&pool,a).await,1); assert_eq!(group_of(&pool,b).await,id);
        let (member,choices) = groups.member_editor(&s,a).await.unwrap(); assert_eq!(choices.len(),2);
        assert!(matches!(groups.assign(&s,&s.csrf,a,member.revision,99999).await,Err(GroupError::Invalid(fields)) if fields.contains(&"group_id")));
        groups.assign(&s,&s.csrf,a,member.revision,id).await.unwrap();
        assert!(matches!(groups.assign(&s,&s.csrf,a,member.revision,1).await,Err(GroupError::Stale)));
        assert_eq!(group_of(&pool,a).await,id);
        let rev = groups.editor(&s,None).await.unwrap().0;
        assert!(matches!(groups.delete(&s,&s.csrf,id,rev).await,Err(GroupError::InUse)));
        groups.delete(&s,&s.csrf,1,rev).await.unwrap();
        assert!(matches!(groups.editor(&s,Some(1)).await,Err(GroupError::NotFound)));
        let current = groups.editor(&s,Some(id)).await.unwrap();
        groups.save(&s,&s.csrf,Some(id),current.0,&input()).await.unwrap();
        assert!(groups.editor(&s,Some(id)).await.unwrap().1.unwrap().is_default, "Default cannot be unset");
        for sql in ["UPDATE customers SET group_id=NULL", "UPDATE customers SET group_id=99999", "DELETE FROM customer_group_configuration", "DELETE FROM customer_groups"] {
            assert!(sqlx::query(sql).execute(&pool).await.is_err(),"{sql}");
        }
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn groups_concurrent_defaults_and_assignments_have_one_winner() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let s = taxonomy_session(&identity).await;
        let groups = CustomerGroups::new(pool.clone(), identity.clone());
        let customers = Customers::new(pool.clone(), identity);
        let mut one = input();
        one.is_default = true;
        let mut two = input();
        two.name_it = "Privati".into();
        two.name_en = "Retail".into();
        two.is_default = true;
        let (a, b) = tokio::join!(
            groups.save(&s, &s.csrf, None, 0, &one),
            groups.save(&s, &s.csrf, None, 0, &two)
        );
        assert!(matches!(
            (a, b),
            (Ok(_), Err(GroupError::Stale)) | (Err(GroupError::Stale), Ok(_))
        ));
        let id = register(&customers, "concurrent@example.test").await;
        let new = groups.editor(&s, None).await.unwrap().0;
        let alternative = GroupInput {
            name_it: "Alternativo".into(),
            name_en: "Alternative".into(),
            is_default: false,
            ..GroupInput::default()
        };
        let other = groups
            .save(&s, &s.csrf, None, new, &alternative)
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            groups.assign(&s, &s.csrf, id, 0, 1),
            groups.assign(&s, &s.csrf, id, 0, other)
        );
        assert!(matches!(
            (a, b),
            (Ok(()), Err(GroupError::Stale)) | (Err(GroupError::Stale), Ok(()))
        ));
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn groups_registration_waits_for_default_change_and_uses_committed_group() {
    isolated(|pool, _| async move {
        let identity=installed(&pool).await;let s=taxonomy_session(&identity).await;
        let groups=CustomerGroups::new(pool.clone(),identity.clone());let customers=Customers::new(pool.clone(),identity);
        let other=groups.save(&s,&s.csrf,None,0,&input()).await.unwrap();
        let mut tx=pool.begin().await.unwrap();
        sqlx::query("UPDATE customer_group_configuration SET default_group_id=$1 WHERE id=1").bind(other).execute(&mut *tx).await.unwrap();
        let pending=tokio::spawn(async move {register(&customers,"waiting@example.test").await});
        // Wait for a database lock, so the concurrency assertion does not depend on hashing time.
        let mut blocked=false;
        for _ in 0..300 {
            blocked=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO customers%')").fetch_one(&pool).await.unwrap();
            if blocked {break;} tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(blocked,"Registration must wait for the default change");assert!(!pending.is_finished());
        tx.commit().await.unwrap();let id=pending.await.unwrap();assert_eq!(group_of(&pool,id).await,other);
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn groups_capacity_constraints_pagination_and_validation_rollback() {
    isolated(|pool, _| async move {
        let identity=installed(&pool).await;let s=taxonomy_session(&identity).await;
        let groups=CustomerGroups::new(pool.clone(),identity);
        sqlx::query("INSERT INTO customer_groups(name_it,name_en) SELECT 'Gruppo '||i,'Group '||i FROM generate_series(1,99) i").execute(&pool).await.unwrap();
        let page=groups.list(&s,999).await.unwrap();assert_eq!(page.total,100);assert_eq!(page.page,5);assert_eq!(page.entries.len(),20);
        assert!(matches!(groups.save(&s,&s.csrf,None,0,&input()).await,Err(GroupError::Capacity)));
        assert_eq!(groups.editor(&s,None).await.unwrap().0,0);
        assert!(matches!(groups.list(&s,0).await,Err(GroupError::Invalid(_))));
        assert!(matches!(groups.save(&s,&s.csrf,Some(1),0,&GroupInput{name_it:" ".into(),..input()}).await,Err(GroupError::Invalid(_))));
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn groups_http_bilingual_forms_validation_escaping_and_csrf() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let groups = CustomerGroups::new(pool.clone(), identity.clone());
        let customers = Customers::new(pool.clone(), identity.clone());
        let id = register(&customers, "web@example.test").await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_session={raw}");
        for (locale, title) in [("it", "Gruppi clienti"), ("en", "Customer groups")] {
            identity.update_locale(&s, &s.csrf, locale).await.unwrap();
            for path in [
                "/admin/customer-groups",
                "/admin/customer-groups/new",
                "/admin/customers",
            ] {
                let r = request(&app, "GET", path, &cookie, "", true).await;
                assert_eq!(r.status(), StatusCode::OK);
                assert_eq!(r.headers()[header::CONTENT_LANGUAGE], locale);
                assert!(html(r).await.contains(title));
            }
            let rev = groups.editor(&s, None).await.unwrap().0;
            let body = format!(
                "csrf={}&revision={rev}&name_it=%3Cscript%3E&name_en=",
                s.csrf
            );
            let r = request(
                &app,
                "POST",
                "/admin/customer-groups/new",
                &cookie,
                &body,
                true,
            )
            .await;
            assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let h = html(r).await;
            assert!(h.contains("&#60;script&#62;"));
            assert!(h.contains("cg-name_en-error"));
            assert!(!h.contains("<script>"));
            assert_eq!(
                request(
                    &app,
                    "POST",
                    "/admin/customer-groups/new",
                    &cookie,
                    &body.replace(&s.csrf, "wrong"),
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            let endpoint = format!("/admin/customers/{id}");
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &endpoint,
                    &cookie,
                    &format!("csrf={}&revision=0&group_id=99999", s.csrf),
                    true
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &endpoint,
                    &cookie,
                    "csrf=wrong&revision=0&group_id=1",
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                request(
                    &app,
                    "GET",
                    "/admin/customer-groups/1/delete",
                    &cookie,
                    "",
                    false
                )
                .await
                .status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
        }
        let endpoint = format!("/admin/customers/{id}");
        assert_eq!(
            request(
                &app,
                "POST",
                &endpoint,
                &cookie,
                &format!("csrf={}&revision=0&group_id=1", s.csrf),
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
                &endpoint,
                &cookie,
                &format!("csrf={}&revision=0&group_id=1", s.csrf),
                true
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            request(&app, "GET", "/admin/customers/99999", &cookie, "", false)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn groups_permissions_reader_revocation_and_no_customer_self_assignment() {
    isolated(|pool, _| async move {
        let identity=installed(&pool).await;let owner=taxonomy_session(&identity).await;
        let role=identity.create_role(&owner,&owner.csrf,"Customer reader",&[Permission::CustomersRead]).await.unwrap();
        identity.create_administrator(&owner,&owner.csrf,"reader@example.test","correct horse battery".into(),talocart_platform::identity::Role::Custom(role.clone())).await.unwrap();
        let raw=login_as(&identity,"reader@example.test").await;let reader=identity.session(&raw).await.unwrap().unwrap();
        let groups=CustomerGroups::new(pool.clone(),identity.clone());
        assert_eq!(groups.list(&reader,1).await.unwrap().total,1);
        assert!(matches!(groups.save(&reader,&reader.csrf,None,0,&input()).await,Err(GroupError::Access(talocart_platform::technical::AccessError::Forbidden))));
        let app=router(test_app(pool.clone(),"Shop".into()).await);let cookie=format!("talocart_session={raw}");
        let h=html(request(&app,"GET","/admin/customer-groups/1",&cookie,"",false).await).await;assert!(h.contains("<fieldset class=\"field-stack\" disabled"));
        assert_eq!(request(&app,"GET","/admin/customer-groups/new",&cookie,"",false).await.status(),StatusCode::FORBIDDEN);
        assert_eq!(request(&app,"POST","/admin/customer-groups/new",&cookie,&format!("csrf={}&revision=0&name_it=Uno&name_en=One",reader.csrf),false).await.status(),StatusCode::FORBIDDEN);
        sqlx::query("DELETE FROM admin_role_permissions WHERE role_key=$1").bind(role.as_str()).execute(&pool).await.unwrap();
        assert!(matches!(groups.list(&reader,1).await,Err(GroupError::Access(talocart_platform::technical::AccessError::Forbidden))));
        let customers=Customers::new(pool.clone(),identity);
        let (anonymous,anon)=customers.anonymous().await.unwrap();
        let r=request(&app,"POST","/account/register",&format!("talocart_customer={anonymous}"),&format!("csrf={}&email=attack@example.test&first_name=A&last_name=B&password=synthetic+password&password_confirmation=synthetic+password&group_id=99999",anon.csrf),false).await;
        assert_eq!(r.status(),StatusCode::SEE_OTHER);
        let assigned: i64=sqlx::query_scalar("SELECT group_id FROM customers WHERE email='attack@example.test'").fetch_one(&pool).await.unwrap();assert_eq!(assigned,1,"Customer input cannot select membership");
        assert_ne!(request(&app,"GET","/admin/customers","","",false).await.status(),StatusCode::OK);
        let id=register(&customers,"normal@example.test").await;assert_eq!(group_of(&pool,id).await,1);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn audit_anonymous_cleanup_skips_locked_sessions_and_limits_batch() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let customers = Customers::new(pool.clone(), identity);
        sqlx::query("INSERT INTO customer_sessions(token_hash,csrf,expires_at) SELECT lpad(to_hex(n),64,'0'),repeat('a',64),now()-interval '1 day' FROM generate_series(1,150) n")
            .execute(&pool).await.unwrap();
        let mut lock = pool.begin().await.unwrap();
        sqlx::query("SELECT token_hash FROM customer_sessions WHERE token_hash=lpad('1',64,'0') FOR UPDATE")
            .fetch_one(&mut *lock).await.unwrap();
        let attempt = tokio::time::timeout(std::time::Duration::from_secs(2), customers.anonymous()).await;
        lock.rollback().await.unwrap();
        let (raw, _) = attempt.expect("Expired locked sessions must not block a new visitor").unwrap();
        assert!(customers.session(&raw).await.unwrap().is_some());
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM customer_sessions WHERE expires_at<=now()")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(remaining, 50, "Cleanup is bounded to 100 rows per new session");
        assert!(sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM customer_sessions WHERE token_hash=lpad('1',64,'0'))").fetch_one(&pool).await.unwrap());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn audit_malformed_membership_and_group_forms_preserve_localized_errors() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let customers = Customers::new(pool.clone(), identity);
        let id = register(&customers, "audit@example.test").await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for locale in ["en", "it"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let cookie = format!("talocart_session={raw}");
            for group in ["", "bad", "9223372036854775808"] {
                let r = request(
                    &app,
                    "POST",
                    &format!("/admin/customers/{id}"),
                    &cookie,
                    &format!("csrf={}&revision=0&group_id={group}", session.csrf),
                    false,
                )
                .await;
                assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let body = html(r).await;
                assert!(
                    body.contains("id=\"customer-group-error\""),
                    "Malformed membership must render the form: {body}"
                );
                // The complete customer card replaces the old "Group assignment" title.
                // Assert the actual localized group field, rather than a title substring.
                assert!(body.contains(if locale == "it" {
                    "Gruppo cliente"
                } else {
                    "Customer group"
                }));
            }
            let r = request(
                &app,
                "POST",
                "/admin/customer-groups/new",
                &cookie,
                &format!(
                    "csrf={}&revision=0&name_it=Audit&name_en=Audit&is_default=invalid",
                    session.csrf
                ),
                false,
            )
            .await;
            assert_eq!(
                r.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "Malformed checkbox must not create a group"
            );
            for endpoint in [
                format!("/admin/customers/{id}"),
                "/admin/customer-groups/new".into(),
            ] {
                let r = request(
                    &app,
                    "POST",
                    &endpoint,
                    &cookie,
                    "csrf=wrong&revision=bad&group_id=bad",
                    false,
                )
                .await;
                assert_eq!(
                    r.status(),
                    StatusCode::FORBIDDEN,
                    "CSRF is checked before field validation"
                );
            }
        }
        assert_eq!(group_of(&pool, id).await, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM customer_groups")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn audit_membership_revision_protects_customer_profile_from_lost_updates() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let owner = taxonomy_session(&identity).await;
        let customers = Customers::new(pool.clone(), identity.clone());
        let id = register(&customers, "revision@example.test").await;
        let groups = CustomerGroups::new(pool.clone(), identity);
        let other = groups
            .save(&owner, &owner.csrf, None, 0, &input())
            .await
            .unwrap();
        let (_, anonymous) = customers.anonymous().await.unwrap();
        let raw = customers
            .login(
                &anonymous,
                &anonymous.csrf,
                "revision@example.test",
                "synthetic password".into(),
            )
            .await
            .unwrap();
        let customer = customers.session(&raw).await.unwrap().unwrap();
        let profile = CustomerProfile {
            email: "revision@example.test".into(),
            first_name: "Updated".into(),
            last_name: "Buyer".into(),
            customer_type: "private".into(),
        };
        customers
            .update(&customer, &customer.csrf, 0, &profile, String::new())
            .await
            .unwrap();
        assert!(matches!(
            groups.assign(&owner, &owner.csrf, id, 0, other).await,
            Err(GroupError::Stale)
        ));
        groups
            .assign(&owner, &owner.csrf, id, 1, other)
            .await
            .unwrap();
        let row: (String, i64, i64) =
            sqlx::query_as("SELECT first_name,revision,group_id FROM customers WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row, ("Updated".into(), 2, other));
        assert!(matches!(
            customers
                .update(&customer, &customer.csrf, 1, &profile, String::new())
                .await,
            Err(talocart_platform::identity::CustomerError::Stale)
        ));
        assert!(
            customers.session(&raw).await.unwrap().is_some(),
            "Membership changes preserve the authenticated session"
        );
    })
    .await;
}

fn schedule(enabled: bool) -> GroupInput {
    use talocart_platform::identity::customer_groups::QuantityTierInput;
    GroupInput {
        quantity_discounts_enabled: enabled,
        quantity_tiers: vec![
            QuantityTierInput {
                min_quantity: "50".into(),
                percent: "10".into(),
            },
            QuantityTierInput {
                min_quantity: "10".into(),
                percent: "2,55".into(),
            },
        ],
        ..input()
    }
}
#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn quantity_groups_atomic_schedule_validation_toggle_concurrency_and_constraints() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await; let s = taxonomy_session(&identity).await;
        let groups = CustomerGroups::new(pool.clone(), identity);
        let id = groups.save(&s,&s.csrf,None,0,&schedule(true)).await.unwrap();
        let (revision, saved) = groups.editor(&s,Some(id)).await.unwrap(); let saved=saved.unwrap();
        assert!(saved.quantity_discounts_enabled); assert_eq!(saved.quantity_tiers[0].min_quantity,10);assert_eq!(saved.quantity_tiers[0].discount_basis_points,255);
        let mut bad = schedule(true); bad.quantity_tiers[1].min_quantity="50".into();
        assert!(matches!(groups.save(&s,&s.csrf,Some(id),revision,&bad).await,Err(GroupError::Invalid(_))));
        assert_eq!(groups.editor(&s,Some(id)).await.unwrap().0,revision);
        assert!(matches!(groups.save(&s,"bad",Some(id),revision,&schedule(false)).await,Err(GroupError::Access(talocart_platform::technical::AccessError::Forbidden))));
        let disabled = schedule(false);
        let (a,b)=tokio::join!(groups.save(&s,&s.csrf,Some(id),revision,&disabled),groups.save(&s,&s.csrf,Some(id),revision,&disabled));
        assert!(matches!((a,b),(Ok(_),Err(GroupError::Stale))|(Err(GroupError::Stale),Ok(_))));
        let (revision,saved)=groups.editor(&s,Some(id)).await.unwrap();let saved=saved.unwrap();assert!(!saved.quantity_discounts_enabled);assert_eq!(saved.quantity_tiers.len(),2);
        groups.save(&s,&s.csrf,Some(id),revision,&schedule(true)).await.unwrap();
        for sql in [
            format!("DELETE FROM customer_group_quantity_tiers WHERE group_id={id}"),
            format!("UPDATE customer_group_quantity_tiers SET discount_basis_points=10001 WHERE group_id={id}"),
            format!("UPDATE customer_group_quantity_tiers SET min_quantity=0 WHERE group_id={id}"),
            format!("INSERT INTO customer_group_quantity_tiers VALUES({id},20,100,100)"),
            format!("INSERT INTO customer_group_quantity_tiers VALUES({id},2,10,100)"),
            "UPDATE customer_groups SET quantity_discounts_enabled=true WHERE id=1".into(),
        ] { assert!(sqlx::query(sqlx::AssertSqlSafe(sql)).execute(&pool).await.is_err()); }
        let revision=groups.editor(&s,None).await.unwrap().0;
        groups.delete(&s,&s.csrf,id,revision).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM customer_group_quantity_tiers").fetch_one(&pool).await.unwrap(),0);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn quantity_groups_http_draft_actions_validation_preserve_inputs_and_revision() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let s = identity.session(&raw).await.unwrap().unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_session={raw}");
        let base = format!(
            "csrf={}&revision=0&name_it=Nuovo&name_en=New&quantity_discounts_enabled=true&quantity_discounts_for_guests=true",
            s.csrf
        );
        for locale in ["it", "en"] {
            identity.update_locale(&s, &s.csrf, locale).await.unwrap();
            let r = request(
                &app,
                "POST",
                "/admin/customer-groups/new",
                &cookie,
                &format!("{base}&action=add_tier"),
                false,
            )
            .await;
            assert_eq!(r.status(), StatusCode::OK);
            let body = html(r).await;
            assert!(body.contains("name=\"tier_quantity_0\""));
            assert!(body.contains("value=\"Nuovo\""));
            assert!(body.contains("name=\"quantity_discounts_for_guests\" value=\"true\" checked"));
            let invalid = format!(
                "{base}&tier_quantity_0=10&tier_percent_0=2%2C5&tier_quantity_1=10&tier_percent_1=5"
            );
            let r = request(
                &app,
                "POST",
                "/admin/customer-groups/new",
                &cookie,
                &invalid,
                false,
            )
            .await;
            assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body = html(r).await;
            assert!(body.contains("id=\"cg-tiers-error\""));
            assert!(body.contains("value=\"2,5\""));
            let r = request(
                &app,
                "POST",
                "/admin/customer-groups/new",
                &cookie,
                &format!("{invalid}&action=remove_tier_1"),
                false,
            )
            .await;
            assert_eq!(r.status(), StatusCode::OK);
            let body = html(r).await;
            assert!(!body.contains("name=\"tier_quantity_1\""));
            assert!(body.contains("value=\"2,5\""));
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM customer_groups")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT revision FROM customer_group_configuration")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            request(
                &app,
                "POST",
                "/admin/customer-groups/new",
                &cookie,
                &format!("{}&action=add_tier", base.replace(&s.csrf, "bad")),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    })
    .await;
}
