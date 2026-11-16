use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn gallery_order_primary_swap_ownership_public_selection_and_media_lifecycle() {
    use talocart_commerce::products::{GalleryAction as Action, ProductError, Products};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let media = test_media(&pool).await;
        let products = Products::new(pool.clone(), identity.clone(), media.clone());
        let (category, _, class) = product_references(&pool, &session).await;
        let mut values = product_values("GALLERY", "gallery");
        values.name = "Foto <script>".into();
        values.status = "published".into();
        values.category_id = category.to_string(); values.tax_class_id = class.to_string();
        let id = products.save(&session, &session.csrf, None, &values).await.unwrap();
        let other = products.save(&session, &session.csrf, None, &product_values("OTHER", "other")).await.unwrap();
        let png = include_bytes!("../fixtures/logo.png");
        products.image(&session, &session.csrf, id, Some(png.to_vec()), "details-main.png").await.unwrap();
        let primary = products.entry(&session, id).await.unwrap().image_key.unwrap();
        for name in ["first.png", "second.png", "third.png"] {
            products.add_gallery_image(&session, &session.csrf, id, png.to_vec(), name).await.unwrap();
        }
        let before = products.gallery(&session,id).await.unwrap();
        assert_eq!(before.iter().map(|r|r.position).collect::<Vec<_>>(),vec![1,2,3]);
        assert_eq!(products.public_gallery(id).await.unwrap().len(),3);
        assert!(products.public_gallery(other).await.unwrap().is_empty());
        let first=before[0].id;let second=before[1].id;let third=before[2].id;
        products.change_gallery_image(&session,&session.csrf,id,third,Action::Earlier).await.unwrap();
        assert_eq!(products.gallery(&session,id).await.unwrap().iter().map(|r|r.id).collect::<Vec<_>>(),vec![first,third,second]);
        products.change_gallery_image(&session,&session.csrf,id,first,Action::Earlier).await.unwrap(); // boundary is a no-op
        products.change_gallery_image(&session,&session.csrf,id,third,Action::Later).await.unwrap();
        assert!(matches!(products.change_gallery_image(&session,&session.csrf,other,first,Action::Remove).await,Err(ProductError::NotFound)));
        products.change_gallery_image(&session,&session.csrf,id,second,Action::Primary).await.unwrap();
        assert_eq!(products.entry(&session,id).await.unwrap().image_key.as_deref(),Some(before[1].image_key.as_str()));
        assert_eq!(products.gallery(&session,id).await.unwrap()[1].image_key,primary);
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let page=html(request(&app,"GET","/products/gallery","","",false).await).await;
        assert!(page.contains("Foto &#60;script&#62;") || page.contains("Foto &lt;script&gt;"));
        assert!(page.contains("aria-label=\"Immagini del prodotto\""));
        let chosen=html(request(&app,"GET",&format!("/products/gallery?photo={first}"),"","",true).await).await;
        assert!(chosen.contains(&format!("<img src=\"{}\"",before[0].image_url("detail"))));
        for photo in ["-1".to_owned(),"not-a-photo".to_owned(),i64::MAX.to_string()] {
            assert_eq!(request(&app,"GET",&format!("/products/gallery?photo={photo}"),"","",false).await.status(),StatusCode::NOT_FOUND);
        }
        let admin=html(request(&app,"GET",&format!("/admin/catalog/products/{id}?tab=image"),&cookie,"",false).await).await;
        assert!(admin.contains("Galleria immagini"));
        assert_eq!(request(&app,"POST",&format!("/admin/catalog/products/{other}/gallery/{first}"),&cookie,&format!("csrf={}&action=remove",session.csrf),false).await.status(),StatusCode::NOT_FOUND);
        for (csrf,action,status) in [("wrong","remove",StatusCode::FORBIDDEN),(session.csrf.as_str(),"unknown",StatusCode::UNPROCESSABLE_ENTITY)] {
            assert_eq!(request(&app,"POST",&format!("/admin/catalog/products/{id}/gallery/{first}"),&cookie,&format!("csrf={csrf}&action={action}"),false).await.status(),status);
        }
        assert!(matches!(products.add_gallery_image(&session,"wrong",id,png.to_vec(),"x.png").await,Err(ProductError::Access(talocart_platform::technical::AccessError::Forbidden))));
        assert!(matches!(products.add_gallery_image(&session,&session.csrf,id,vec![1,2,3],"x.png").await,Err(ProductError::Image)));
        sqlx::query("UPDATE catalog_categories SET active=false WHERE id=$1").bind(category).execute(&pool).await.unwrap();
        assert!(products.public_gallery(id).await.unwrap().is_empty());
        assert_eq!(request(&app,"GET","/products/gallery","","",false).await.status(),StatusCode::NOT_FOUND);
        sqlx::query("UPDATE catalog_categories SET active=true WHERE id=$1").bind(category).execute(&pool).await.unwrap();
        // References and derivative images survive cleanup while the product is alive.
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '48 hours'").execute(&pool).await.unwrap();
        completed_prune(&media,&pool,false).await;
        for image in &before { assert!(media.read(&image.image_key).await.is_ok()); }
        assert!(media.read(&primary).await.is_ok());
        let removed=&before[2];
        products.change_gallery_image(&session,&session.csrf,id,third,Action::Remove).await.unwrap();
        assert_eq!(products.gallery(&session,id).await.unwrap().iter().map(|r|r.position).collect::<Vec<_>>(),vec![1,2]);
        assert!(media.read(&removed.image_key).await.is_ok()); // grace period
        assert!(sqlx::query_scalar::<_,bool>("SELECT unused_since IS NOT NULL FROM media_objects WHERE key=$1").bind(&removed.image_key).fetch_one(&pool).await.unwrap());
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '48 hours' WHERE unused_since IS NOT NULL").execute(&pool).await.unwrap();
        completed_prune(&media,&pool,false).await;
        assert!(media.read(&removed.image_key).await.is_ok(),"Unlinked gallery sources remain in the library");
        purge_library_sources(&pool,&session,std::slice::from_ref(&removed.image_key)).await;
        completed_prune(&media,&pool,false).await;
        assert!(media.read(&removed.image_key).await.is_err());
        let restarted=Products::new(pool.clone(),Identity::new(pool.clone()),media.clone());
        assert_eq!(restarted.gallery(&session,id).await.unwrap().len(),2);
        products.delete(&session,&session.csrf,id).await.unwrap();
        assert!(products.gallery(&session,id).await.unwrap().is_empty());
        assert!(sqlx::query_scalar::<_,bool>("SELECT unused_since IS NOT NULL FROM media_objects WHERE key=$1").bind(primary).fetch_one(&pool).await.unwrap());
        // Promotion without a primary removes only the promoted gallery row.
        products.add_gallery_image(&session,&session.csrf,other,png.to_vec(),"only.png").await.unwrap();
        let row=products.gallery(&session,other).await.unwrap().remove(0);
        let mut published=values.clone(); published.model="OTHER".into(); published.slug="other".into();
        products.save(&session,&session.csrf,Some(other),&published).await.unwrap();
        let no_primary=html(request(&app,"GET","/products/other","","",false).await).await;
        assert!(no_primary.contains(&format!("<img src=\"{}\"",row.image_url("detail"))),"A gallery-only product shows its first image");
        assert!(no_primary.contains(&format!("id=\"product-photo-{}\"",row.id)));

        products.change_gallery_image(&session,&session.csrf,other,row.id,Action::Primary).await.unwrap();
        assert!(products.gallery(&session,other).await.unwrap().is_empty());
        assert_eq!(products.entry(&session,other).await.unwrap().image_key.as_deref(),Some(row.image_key.as_str()));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn gallery_capacity_concurrency_sql_constraints_and_rollback() {
    use talocart_commerce::products::{GalleryAction as Action, ProductError, Products};
    isolated(|pool,_| async move {
        let identity=installed(&pool).await;let session=taxonomy_session(&identity).await;
        let media=test_media(&pool).await;let products=Products::new(pool.clone(),identity,media.clone());
        let id=products.save(&session,&session.csrf,None,&product_values("CAPACITY","capacity")).await.unwrap();
        let png=include_bytes!("../fixtures/logo.png");
        products.add_gallery_image(&session,&session.csrf,id,png.to_vec(),"gallery.png").await.unwrap();
        let first=products.gallery(&session,id).await.unwrap().remove(0);
        // Direct SQL shares the same limit and constraints; a failed transaction leaves no extra row.
        for position in 2..=11 {
            sqlx::query("INSERT INTO catalog_product_images(product_id,image_key,position) VALUES($1,$2,$3)").bind(id).bind(&first.image_key).bind(position).execute(&pool).await.unwrap();
        }
        let objects_before:i64=sqlx::query_scalar("SELECT count(*) FROM media_objects").fetch_one(&pool).await.unwrap();
        let (a,b)=tokio::join!(products.add_gallery_image(&session,&session.csrf,id,png.to_vec(),"last.png"),products.add_gallery_image(&session,&session.csrf,id,png.to_vec(),"last.png"));
        assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        assert!(matches!(a,Err(ProductError::GalleryFull)) || matches!(b,Err(ProductError::GalleryFull)));
        let rows=products.gallery(&session,id).await.unwrap();assert_eq!(rows.len(),12);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM media_objects").fetch_one(&pool).await.unwrap(),objects_before*2,"Only one additional fixture and its distinct variants are published");
        let extra=sqlx::query("INSERT INTO catalog_product_images(product_id,image_key,position) VALUES($1,$2,12)").bind(id).bind(&first.image_key).execute(&pool).await.unwrap_err();
        assert_eq!(extra.as_database_error().unwrap().constraint(),Some("catalog_product_images_limit"));
        let other=products.save(&session,&session.csrf,None,&product_values("OTHER","other")).await.unwrap();
        assert!(sqlx::query("UPDATE catalog_product_images SET product_id=$1 WHERE id=$2").bind(other).bind(first.id).execute(&pool).await.is_err());
        // Two moves and simultaneous removal remain serialized, with a contiguous order.
        let (a,b)=tokio::join!(products.change_gallery_image(&session,&session.csrf,id,rows[11].id,Action::Earlier),products.change_gallery_image(&session,&session.csrf,id,rows[0].id,Action::Later));
        a.unwrap();b.unwrap();
        let (a,b)=tokio::join!(products.change_gallery_image(&session,&session.csrf,id,first.id,Action::Remove),products.change_gallery_image(&session,&session.csrf,id,first.id,Action::Remove));
        assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);
        assert_eq!(products.gallery(&session,id).await.unwrap().iter().map(|r|r.position).collect::<Vec<_>>(),(1..=11).collect::<Vec<_>>());
        assert!(sqlx::query_scalar::<_,bool>("SELECT unused_since IS NULL FROM media_objects WHERE key=$1").bind(first.image_key).fetch_one(&pool).await.unwrap(),"A remaining shared reference retains its media");
        let mut tx=pool.begin().await.unwrap();
        sqlx::query("UPDATE catalog_product_images SET position=1 WHERE product_id=$1").bind(id).execute(&mut *tx).await.unwrap();
        assert!(tx.commit().await.is_err());
        assert_eq!(products.gallery(&session,id).await.unwrap().len(),11);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn gallery_http_upload_permissions_readonly_and_revoked_session() {
    use talocart_commerce::products::Products;
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool,_| async move {
        let identity=installed(&pool).await;let raw=authenticated(&identity).await;
        let owner=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let products=Products::new(pool.clone(),identity.clone(),test_media(&pool).await);
        let id=products.save(&owner,&owner.csrf,None,&product_values("UPLOAD","upload")).await.unwrap();
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let png=include_bytes!("../fixtures/logo.png");
        for (token,data,expected) in [("wrong",png.as_slice(),StatusCode::FORBIDDEN),(owner.csrf.as_str(),b"not a photo".as_slice(),StatusCode::UNPROCESSABLE_ENTITY),(owner.csrf.as_str(),png.as_slice(),StatusCode::SEE_OTHER)] {
            let mut body=format!("--photo\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{token}\r\n--photo\r\nContent-Disposition: form-data; name=\"image\"; filename=\"../../Photo.webp\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();body.extend_from_slice(data);body.extend_from_slice(b"\r\n--photo--\r\n");
            let r=app.clone().oneshot(Request::builder().method("POST").uri(format!("/admin/catalog/products/{id}/gallery")).header(header::COOKIE,&cookie).header(header::CONTENT_TYPE,"multipart/form-data; boundary=photo").body(Body::from(body)).unwrap()).await.unwrap();
            assert_eq!(r.status(),expected);
            if expected==StatusCode::SEE_OTHER { assert_eq!(r.headers()[header::LOCATION],format!("/admin/catalog/products/{id}?saved=1&tab=image")); }
        }
        let image=products.gallery(&owner,id).await.unwrap().remove(0);
        let role=identity.create_role(&owner,&owner.csrf,"Gallery reader",&[Permission::ProductsRead]).await.unwrap();
        identity.create_administrator(&owner,&owner.csrf,"gallery-reader@example.test","correct horse reader".into(),Role::Custom(role.clone())).await.unwrap();
        let (_,anon)=identity.anonymous().await.unwrap();let reader_raw=identity.login(&anon,&anon.csrf,"gallery-reader@example.test","correct horse reader".into()).await.unwrap();
        let reader=identity.session(&reader_raw).await.unwrap().unwrap();let reader_cookie=format!("talocart_session={reader_raw}");
        assert_eq!(products.gallery(&reader,id).await.unwrap().len(),1);
        let page=html(request(&app,"GET",&format!("/admin/catalog/products/{id}?tab=image"),&reader_cookie,"",false).await).await;
        assert!(page.contains("Galleria immagini"));assert!(!page.contains("Aggiungi alla galleria"));assert!(!page.contains("Usa come principale"));
        assert_eq!(request(&app,"POST",&format!("/admin/catalog/products/{id}/gallery/{}",image.id),&reader_cookie,&format!("csrf={}&action=remove",reader.csrf),false).await.status(),StatusCode::FORBIDDEN);
        identity.update_role(&owner,&owner.csrf,&role,"Gallery reader",&[]).await.unwrap();
        assert!(products.gallery(&reader,id).await.is_err());
        assert_eq!(request(&app,"POST",&format!("/admin/catalog/products/{id}/gallery/{}",image.id),&reader_cookie,&format!("csrf={}&action=remove",reader.csrf),false).await.status(),StatusCode::SEE_OTHER);
        assert_eq!(products.gallery(&owner,id).await.unwrap().len(),1);
    }).await;
}
