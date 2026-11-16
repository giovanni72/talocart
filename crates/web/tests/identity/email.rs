include!("email_support.rs");

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn email_connection_check_survives_slow_smtp_and_reports_timeout() {
    isolated(|pool, _| async move {
        use talocart_platform::mail::MailKey;
        let (identity, s, mail) = configured(&pool).await;
        let key = MailKey::from_bytes(&[23; 32]).unwrap();
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Slow SMTP".into()).await);

        for responsive in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut values = mail.transport_settings(&s).await.unwrap();
            values.enabled = true;
            values.port = i32::from(listener.local_addr().unwrap().port());
            values.timeout_seconds = 20;
            mail.save_transport(&s, &s.csrf, &values, "", false, &key)
                .await
                .unwrap();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                if !responsive {
                    // Hold the connection past the maximum SMTP deadline.
                    let _socket = socket;
                    std::future::pending::<()>().await;
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(11)).await;
                let (reader, mut writer) = socket.into_split();
                let mut lines = BufReader::new(reader).lines();
                writer.write_all(b"220 slow SMTP\r\n").await.unwrap();
                while let Some(line) = lines.next_line().await.unwrap() {
                    let reply = match line.as_str() {
                        command if command.starts_with("EHLO") => "250 slow SMTP\r\n",
                        "NOOP" => "250 OK\r\n",
                        "QUIT" => "221 bye\r\n",
                        command => panic!("Connection check must not send a message: {command}"),
                    };
                    writer.write_all(reply.as_bytes()).await.unwrap();
                    if line == "QUIT" {
                        break;
                    }
                }
            });
            let response = request(
                &app,
                "POST",
                "/admin/settings/email/check",
                &cookie,
                &format!("csrf={}", session.csrf),
                false,
            )
            .await;
            server.abort();
            let _ = server.await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(
                response.headers()[header::LOCATION],
                if responsive {
                    "/admin/settings/email?notice=connected"
                } else {
                    "/admin/settings/email?notice=connection-error"
                }
            );
        }
        assert_eq!(mail.log(&s, 1).await.unwrap().total, 0);
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn email_configuration_http_permissions_csrf_locales_and_preview() {
    isolated(|pool,_|async move{
        let(identity,s,mail)=configured(&pool).await;let raw=authenticated(&identity).await;let cookie=format!("talocart_session={raw}");let session=identity.session(&raw).await.unwrap().unwrap();
        let app=router(test_app(pool.clone(),"Mail".into()).await);
        assert_eq!(request(&app,"GET","/admin/settings/email","","",false).await.status(),StatusCode::SEE_OTHER);
        for locale in ["it","en"]{
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let h=html(request(&app,"GET","/admin/settings/email",&cookie,"",false).await).await;assert!(h.contains(if locale=="it"{"Email e notifiche"}else{"Email and notifications"}));assert!(h.contains("name=\"revision\""));assert!(h.contains("type=\"password\""));
            for kind in Notice::ALL{for plain in [false,true]{let r=request(&app,"GET",&format!("/admin/settings/email/preview/{}/{}?plain={plain}",kind.key(),locale),&cookie,"",false).await;assert_eq!(r.status(),StatusCode::OK);assert!(r.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("sandbox"));let h=html(r).await;assert!(h.contains("DEMO-0001"));}}
        }
        let settings=mail.settings(&s).await.unwrap();let mut bad=settings.clone();bad.admin_recipients=vec!["same@example.test".into(),"SAME@example.test".into()];assert!(matches!(mail.save(&s,&s.csrf,&bad).await,Err(MailError::Invalid)));
        bad=settings.clone();bad.reply_to="good@example.test\r\nBcc: evil@example.test".into();assert!(matches!(mail.save(&s,&s.csrf,&bad).await,Err(MailError::Invalid)));
        let first=mail.save(&s,&s.csrf,&settings);let second=mail.save(&s,&s.csrf,&settings);let(a,b)=tokio::join!(first,second);assert_eq!([a.is_ok(),b.is_ok()].into_iter().filter(|v|*v).count(),1);
        assert_eq!(request(&app,"POST","/admin/settings/email/check",&cookie,"csrf=bad",false).await.status(),StatusCode::FORBIDDEN);
        let key=talocart_platform::mail::token().unwrap();let body=format!("csrf={}&key={key}&recipient=preview%40example.test&template=order_received&locale=en",session.csrf);
        assert_eq!(request(&app,"POST","/admin/settings/email/test",&cookie,&body,false).await.status(),StatusCode::SEE_OTHER);assert_eq!(request(&app,"POST","/admin/settings/email/test",&cookie,&body,false).await.status(),StatusCode::SEE_OTHER);
        assert_eq!(mail.log(&s,1).await.unwrap().total,1);
        let role=identity.create_role(&s,&s.csrf,"Mail reader",&[talocart_platform::identity::Permission::SettingsRead]).await.unwrap();identity.create_administrator(&s,&s.csrf,"mail-reader@example.test","correct horse battery".into(),talocart_platform::identity::Role::Custom(role)).await.unwrap();
        let raw=login_as(&identity,"mail-reader@example.test").await;let rs=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");let h=html(request(&app,"GET","/admin/settings/email",&cookie,"",false).await).await;assert!(h.contains(" disabled"));assert!(!h.contains("action=\"/admin/settings/email/test\""));
        for endpoint in ["check","test","1/retry"]{let body=format!("csrf={}&key={key}&recipient=preview%40example.test&template=order_received&locale=en",rs.csrf);assert_eq!(request(&app,"POST",&format!("/admin/settings/email/{endpoint}"),&cookie,&body,false).await.status(),StatusCode::FORBIDDEN);}
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL and local SMTP sockets"]
async fn email_worker_acceptance_pause_concurrency_retries_and_failures() {
    isolated(|pool, _| async move {
        let (_, s, mail) = configured(&pool).await;
        let (config, mode, count, task) = smtp(0).await;
        config.check_connection().await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        let a = enqueue(&pool, "accept").await;
        let mut disabled = config.clone();
        disabled.enabled = false;
        assert!(!mail.process_one(&disabled).await.unwrap());
        assert_eq!(state(&pool, a).await.1, 0);
        let mut settings = mail.settings(&s).await.unwrap();
        settings.enabled = false;
        mail.save(&s, &s.csrf, &settings).await.unwrap();
        assert!(!mail.process_one(&config).await.unwrap());
        assert_eq!(state(&pool, a).await.1, 0);
        settings.revision += 1;
        settings.enabled = true;
        mail.save(&s, &s.csrf, &settings).await.unwrap();
        let (a1, a2) = tokio::join!(mail.process_one(&config), mail.process_one(&config));
        assert_eq!(
            [a1.unwrap(), a2.unwrap()]
                .into_iter()
                .filter(|v| *v)
                .count(),
            1
        );
        assert_eq!(state(&pool, a).await, ("accepted".into(), 1, 1));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(mail.retry(&s, &s.csrf, a).await.is_err());
        mode.store(451, Ordering::SeqCst);
        let b = enqueue(&pool, "temporary").await;
        for attempt in 1..=5 {
            assert!(mail.process_one(&config).await.unwrap());
            let st = state(&pool, b).await;
            assert_eq!(st.1, attempt);
            assert_eq!(st.0, if attempt < 5 { "pending" } else { "failed" });
            if attempt < 5 {
                assert!(!mail.process_one(&config).await.unwrap());
                sqlx::query("UPDATE email_outbox SET available_at=clock_timestamp() WHERE id=$1")
                    .bind(b)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
        assert_eq!(mail.attempts(&s, b).await.unwrap().len(), 5);
        let identity = Identity::new(pool.clone());
        let raw = authenticated(&identity).await;
        let app = router(test_app(pool.clone(), "Mail".into()).await);
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let body = html(
                request(
                    &app,
                    "GET",
                    "/admin/settings/email",
                    &format!("talocart_session={raw}"),
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(!body.contains("nuovo tentativo programmato"));
            assert!(!body.contains("retry scheduled"));
        }
        mail.retry(&s, &s.csrf, b).await.unwrap();
        assert!(mail.retry(&s, &s.csrf, b).await.is_err());
        mode.store(0, Ordering::SeqCst);
        mail.process_one(&config).await.unwrap();
        assert_eq!(state(&pool, b).await, ("accepted".into(), 6, 1));
        mode.store(550, Ordering::SeqCst);
        let c = enqueue(&pool, "permanent").await;
        mail.process_one(&config).await.unwrap();
        assert_eq!(state(&pool, c).await.0, "failed");
        assert_eq!(mail.attempts(&s, c).await.unwrap()[0].outcome, "permanent");
        task.abort();
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL and local SMTP sockets"]
async fn email_worker_unknown_outcomes_leases_and_no_open_smtp_transaction() {
    isolated(|pool,_|async move{
        let(_,s,mail)=configured(&pool).await;let(config,mode,count,task)=smtp(999).await;let a=enqueue(&pool,"timeout").await;
        let work=tokio::spawn({let mail=mail.clone();let config=config.clone();async move{mail.process_one(&config).await}});
        for _ in 0..100{if count.load(Ordering::SeqCst)>0{break;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
        // Claim and configuration locks must already be released while SMTP awaits DATA ACK.
        let mut tx=pool.begin().await.unwrap();sqlx::query("SET LOCAL lock_timeout='100ms'").execute(&mut *tx).await.unwrap();sqlx::query("UPDATE email_outbox SET subject=subject WHERE id=$1").bind(a).execute(&mut *tx).await.unwrap();sqlx::query("UPDATE email_configuration SET enabled=enabled WHERE id=1").execute(&mut *tx).await.unwrap();tx.commit().await.unwrap();
        work.await.unwrap().unwrap();assert_eq!(state(&pool,a).await.0,"pending");assert_eq!(mail.attempts(&s,a).await.unwrap()[0].outcome,"timeout");
        // Simulate a process that died holding a committed claim.
        sqlx::query("UPDATE email_outbox SET state='sending',attempts=attempts+1,cycle_attempts=cycle_attempts+1,lease_token='old',lease_until=clock_timestamp()-interval '1 second' WHERE id=$1").bind(a).execute(&pool).await.unwrap();sqlx::query("INSERT INTO email_attempts(message_id,attempt,outcome) VALUES($1,2,'sending')").bind(a).execute(&pool).await.unwrap();mode.store(0,Ordering::SeqCst);assert!(!mail.process_one(&config).await.unwrap());assert_eq!(mail.attempts(&s,a).await.unwrap()[0].outcome,"interrupted");
        sqlx::query("UPDATE email_outbox SET available_at=clock_timestamp() WHERE id=$1").bind(a).execute(&pool).await.unwrap();mail.process_one(&config).await.unwrap();assert_eq!(state(&pool,a).await,("accepted".into(),3,3));task.abort();
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn email_commercial_event_fanout_is_atomic_bilingual_and_replay_safe() {
    isolated(|pool, _| async move {
        let (_, s, mail) = configured(&pool).await;
        let snapshot = notifications::sample().unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("UPDATE store_settings SET name='Uncommitted event' WHERE id=1")
            .execute(&mut *tx)
            .await
            .unwrap();
        let ids = notifications::enqueue_event(
            &mut tx,
            "order:123:created",
            notifications::Event::Created,
            &snapshot,
            "customer@example.test",
            ("it", "en"),
        )
        .await
        .unwrap();
        assert_eq!(ids.len(), 2);
        tx.rollback().await.unwrap();
        assert_eq!(mail.log(&s, 1).await.unwrap().total, 0);
        assert_ne!(
            sqlx::query_scalar::<_, String>("SELECT name FROM store_settings WHERE id=1")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "Uncommitted event"
        );
        for (key, event, count) in [
            ("created", notifications::Event::Created, 2),
            (
                "payment:provider-456",
                notifications::Event::PaymentRecorded,
                1,
            ),
            ("cancelled", notifications::Event::Cancelled, 2),
        ] {
            let mut tx = pool.begin().await.unwrap();
            let first = notifications::enqueue_event(
                &mut tx,
                key,
                event,
                &snapshot,
                "customer@example.test",
                ("it", "en"),
            )
            .await
            .unwrap();
            let replay = notifications::enqueue_event(
                &mut tx,
                key,
                event,
                &snapshot,
                "customer@example.test",
                ("it", "en"),
            )
            .await
            .unwrap();
            assert_eq!(first, replay);
            assert_eq!(first.len(), count);
            tx.commit().await.unwrap();
        }
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT template,recipient,locale,subject FROM email_outbox ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 5);
        for (template, recipient, locale, subject) in rows {
            assert!(!subject.contains("[PROVA]"));
            assert!(!subject.contains("[TEST]"));
            assert_eq!(
                locale,
                if recipient == "customer@example.test" {
                    "it"
                } else {
                    "en"
                }
            );
            if template == "payment_received" {
                assert_eq!(recipient, "customer@example.test");
            }
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL and local SMTP sockets"]
async fn email_worker_fences_stale_sender_results() {
    isolated(|pool, _| async move {
        let (_, session, mail) = configured(&pool).await;
        let (config, _, count, task) = smtp(999).await;
        let id = enqueue(&pool, "lost-lease").await;
        let sending = tokio::spawn({
            let mail = mail.clone();
            let config = config.clone();
            async move { mail.process_one(&config).await }
        });
        for _ in 0..100 {
            if count.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        sqlx::query("UPDATE email_outbox SET lease_token='new-owner' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sending.await.unwrap().unwrap();
        assert_eq!(state(&pool, id).await.0, "sending");
        assert_eq!(
            mail.attempts(&session, id).await.unwrap()[0].outcome,
            "sending"
        );
        sqlx::query(
            "UPDATE email_outbox SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(!mail.process_one(&config).await.unwrap());
        assert_eq!(
            mail.attempts(&session, id).await.unwrap()[0].outcome,
            "interrupted"
        );
        task.abort();
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn email_audit_replay_preserves_original_audience() {
    isolated(|pool, _| async move {
        let (_, s, mail) = configured(&pool).await;
        let snapshot = notifications::sample().unwrap();
        let mut tx = pool.begin().await.unwrap();
        let first = notifications::enqueue_event(
            &mut tx,
            "audit:created",
            notifications::Event::Created,
            &snapshot,
            "customer@example.test",
            ("it", "en"),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let mut settings = mail.settings(&s).await.unwrap();
        settings.admin_recipients = vec!["replacement@example.test".into()];
        mail.save(&s, &s.csrf, &settings).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let replay = notifications::enqueue_event(
            &mut tx,
            "audit:created",
            notifications::Event::Created,
            &snapshot,
            "customer@example.test",
            ("it", "en"),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let total = mail.log(&s, 1).await.unwrap().total;
        assert_eq!(
            (replay, total),
            (first, 2),
            "Replay must not add a new audience"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn email_audit_valid_large_snapshot_can_be_enqueued() {
    isolated(|pool, _| async move {
        configured(&pool).await;
        let mut snapshot = notifications::sample().unwrap();
        snapshot.reference = "R".repeat(80);
        snapshot.store_name = "&".repeat(120);
        snapshot.customer_name = "&".repeat(200);
        snapshot.payment_method = "&".repeat(120);
        snapshot.instructions = "&".repeat(4000);
        for description in ["A".repeat(500), "&".repeat(500), "é".repeat(250)] {
            snapshot.lines = (0..200)
                .map(|_| notifications::Line {
                    description: description.clone(),
                    quantity: u32::MAX,
                    total: talocart_commerce::pricing::Money::from_cents(20).unwrap(),
                })
                .collect();
            for locale in ["it", "en"] {
                for notice in Notice::ALL {
                    snapshot.fulfillment = notifications::sample_for(notice).unwrap().fulfillment;
                    let rendered = notifications::render(&snapshot, notice, locale, true).unwrap();
                    let mut tx = pool.begin().await.unwrap();
                    let result =
                        Mail::enqueue(&mut tx, "audit:large", "customer@example.test", &rendered)
                            .await;
                    tx.rollback().await.unwrap();
                    assert!(
                        result.is_ok(),
                        "Valid snapshot rejected: text={} html={} result={:?}",
                        rendered.text.len(),
                        rendered.html.len(),
                        result
                    );
                }
            }
        }
        for html in [false, true] {
            let mut oversized = content();
            if html {
                oversized.html = "a".repeat(talocart_platform::mail::MAX_HTML_BYTES + 1);
            } else {
                oversized.text = "a".repeat(talocart_platform::mail::MAX_TEXT_BYTES + 1);
            }
            let mut tx = pool.begin().await.unwrap();
            assert!(matches!(
                Mail::enqueue(&mut tx, "oversized", "customer@example.test", &oversized).await,
                Err(MailError::Invalid)
            ));
            tx.rollback().await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn queue_health_distinguishes_due_work_retries_expired_leases_and_terminal_messages() {
    isolated(|pool, _| async move {
        let (_, _, mail) = configured(&pool).await;
        let mut ids = Vec::new();
        for index in 0..7 {
            ids.push(enqueue(&pool, &format!("health:{index}")).await);
        }
        sqlx::query("UPDATE email_outbox SET available_at=clock_timestamp()-interval '2 minutes' WHERE id=$1").bind(ids[0]).execute(&pool).await.unwrap();
        sqlx::query("UPDATE email_outbox SET available_at=clock_timestamp()+interval '10 minutes' WHERE id=$1").bind(ids[1]).execute(&pool).await.unwrap();
        sqlx::query("UPDATE email_outbox SET attempts=1,cycle_attempts=1 WHERE id=$1").bind(ids[2]).execute(&pool).await.unwrap();
        sqlx::query("UPDATE email_outbox SET state='sending',attempts=1,cycle_attempts=1,lease_token='synthetic',lease_until=clock_timestamp()-interval '1 second' WHERE id=$1").bind(ids[3]).execute(&pool).await.unwrap();
        sqlx::query("UPDATE email_outbox SET state='failed',attempts=5,cycle_attempts=5 WHERE id=$1").bind(ids[4]).execute(&pool).await.unwrap();
        sqlx::query("UPDATE email_outbox SET state='accepted',accepted_at=clock_timestamp() WHERE id=$1").bind(ids[5]).execute(&pool).await.unwrap();
        sqlx::query("UPDATE email_outbox SET superseded=true WHERE id=$1").bind(ids[6]).execute(&pool).await.unwrap();
        let before: String = sqlx::query_scalar("SELECT md5(jsonb_agg(to_jsonb(e) ORDER BY id)::text) FROM email_outbox e").fetch_one(&pool).await.unwrap();
        let health = mail.queue_health().await.unwrap();
        assert_eq!((health.pending,health.running,health.failed,health.retrying,health.expired_leases),(3,1,1,1,1));
        assert!((119..=140).contains(&health.oldest_due_seconds));
        let after: String = sqlx::query_scalar("SELECT md5(jsonb_agg(to_jsonb(e) ORDER BY id)::text) FROM email_outbox e").fetch_one(&pool).await.unwrap();
        assert_eq!(before, after, "Diagnostics must never claim or alter work");
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn email_smtp_encrypted_settings_revision_permissions_and_secret_redaction() {
    isolated(|pool,_|async move {
        use talocart_platform::mail::MailKey;
        let(identity,s,mail)=configured(&pool).await;
        let key=MailKey::from_bytes(&[23;32]).unwrap();
        let mut values=mail.transport_settings(&s).await.unwrap();
        values.host="smtp.example.test".into();values.port=587;values.security="starttls".into();values.username="synthetic-user".into();
        assert!(mail.save_transport(&s,"wrong",&values,"synthetic-password",false,&key).await.is_err());
        assert!(!Mail::has_smtp_password(&pool).await.unwrap());
        mail.save_transport(&s,&s.csrf,&values,"synthetic-password",false,&key).await.unwrap();
        let cipher:Vec<u8>=sqlx::query_scalar("SELECT password_ciphertext FROM email_transport").fetch_one(&pool).await.unwrap();
        assert!(!cipher.windows(18).any(|w|w==b"synthetic-password"));
        assert_eq!(mail.smtp_config(&key).await.unwrap().password,"synthetic-password");
        assert!(mail.smtp_config(&MailKey::from_bytes(&[24;32]).unwrap()).await.is_err());
        assert!(matches!(mail.save_transport(&s,&s.csrf,&values,"",false,&key).await,Err(MailError::Stale)));
        values=mail.transport_settings(&s).await.unwrap();values.port=465;values.security="tls".into();
        let(a,b)=tokio::join!(mail.save_transport(&s,&s.csrf,&values,"",false,&key),mail.save_transport(&s,&s.csrf,&values,"",false,&key));
        assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        assert_eq!(mail.smtp_config(&key).await.unwrap().password,"synthetic-password");
        let raw=authenticated(&identity).await;let session=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let app=router(test_app(pool.clone(),"SMTP".into()).await);
        for locale in ["it","en"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let page=html(request(&app,"GET","/admin/settings/email",&cookie,"",false).await).await;
            assert!(page.contains(if locale=="it" {"Password salvata."} else {"Password saved."}));
            assert!(!page.contains("synthetic-password"));
            assert!(!page.contains("password_ciphertext"));
        }
        let revision=mail.transport_settings(&s).await.unwrap().revision;
        let body=format!("csrf={}&revision={revision}&host=smtp.example.test&port=587&security=starttls&username=synthetic-user&password=do-not-reflect-this&timeout_seconds=99",session.csrf);
        assert_eq!(request(&app,"GET","/admin/settings/email/transport",&cookie,"",false).await.status(),StatusCode::METHOD_NOT_ALLOWED);
        let bad_csrf=body.replace(&session.csrf,"wrong").replace("timeout_seconds=99","timeout_seconds=10");
        assert_eq!(request(&app,"POST","/admin/settings/email/transport",&cookie,&bad_csrf,false).await.status(),StatusCode::FORBIDDEN);
        let response=request(&app,"POST","/admin/settings/email/transport",&cookie,&body,false).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!html(response).await.contains("do-not-reflect-this"));
        let role=identity.create_role(&s,&s.csrf,"SMTP reader",&[talocart_platform::identity::Permission::SettingsRead]).await.unwrap();
        identity.create_administrator(&s,&s.csrf,"smtp-reader@example.test","correct horse battery".into(),talocart_platform::identity::Role::Custom(role)).await.unwrap();
        let raw=login_as(&identity,"smtp-reader@example.test").await;let reader=identity.session(&raw).await.unwrap().unwrap();
        let body=body.replace(&session.csrf,&reader.csrf).replace("timeout_seconds=99","timeout_seconds=10");
        assert_eq!(request(&app,"POST","/admin/settings/email/transport",&format!("talocart_session={raw}"),&body,false).await.status(),StatusCode::FORBIDDEN);
        values=mail.transport_settings(&s).await.unwrap();values.username.clear();
        mail.save_transport(&s,&s.csrf,&values,"",true,&key).await.unwrap();
        assert!(!Mail::has_smtp_password(&pool).await.unwrap());
        assert!(mail.smtp_config(&key).await.unwrap().password.is_empty());
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL and local SMTP sockets"]
async fn email_smtp_changes_apply_to_new_sends_without_interrupting_in_flight() {
    isolated(|pool, _| async move {
        use talocart_platform::mail::MailKey;
        let (_, s, mail) = configured(&pool).await;
        let key = Arc::new(MailKey::from_bytes(&[23; 32]).unwrap());
        let (a, _, count_a, task_a) = smtp(999).await;
        let (b, _, count_b, task_b) = smtp(0).await;
        let mut settings = mail.transport_settings(&s).await.unwrap();
        settings.enabled = true;
        settings.port = i32::from(a.port);
        settings.timeout_seconds = 1;
        mail.save_transport(&s, &s.csrf, &settings, "", false, &key)
            .await
            .unwrap();
        let first = enqueue(&pool, "in-flight-smtp").await;
        let worker = mail.clone();
        let old_key = key.clone();
        let sending = tokio::spawn(async move { worker.process_pending(&old_key).await.unwrap() });
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while count_a.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        settings = mail.transport_settings(&s).await.unwrap();
        settings.port = i32::from(b.port);
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            mail.save_transport(&s, &s.csrf, &settings, "", false, &key),
        )
        .await
        .unwrap()
        .unwrap();
        let second = enqueue(&pool, "new-smtp").await;
        assert!(mail.process_pending(&key).await.unwrap());
        assert_eq!(count_b.load(Ordering::SeqCst), 1);
        assert_eq!(state(&pool, second).await.0, "accepted");
        assert!(sending.await.unwrap());
        assert_eq!(state(&pool, first).await.0, "pending");
        settings = mail.transport_settings(&s).await.unwrap();
        settings.enabled = false;
        mail.save_transport(&s, &s.csrf, &settings, "", false, &key)
            .await
            .unwrap();
        let paused = enqueue(&pool, "smtp-paused").await;
        assert!(!mail.process_pending(&key).await.unwrap());
        assert_eq!(state(&pool, paused).await.1, 0);
        task_a.abort();
        task_b.abort();
    })
    .await;
}
