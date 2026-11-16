use super::*;
use talocart_commerce::attributes::{AttributeError, Attributes, Kind, Values};
use talocart_commerce::products::Products;
use talocart_platform::{
    content::{Draft, Language, Translation},
    locale::Locale,
};

fn values(name: &str, group: i64) -> Values {
    Values {
        name: name.into(),
        group_id: group.to_string(),
        sort_order: "0".into(),
        revision: "0".into(),
        ..Values::default()
    }
}
fn draft(en: &str, it: &str) -> Draft {
    Draft {
        primary: "en".into(),
        rows: vec![
            Translation {
                locale: "en".into(),
                name: en.into(),
                ..Translation::default()
            },
            Translation {
                locale: "it".into(),
                name: it.into(),
                ..Translation::default()
            },
        ],
        ..Draft::default()
    }
}
async fn attribute_fixture(
    pool: &PgPool,
) -> (
    Identity,
    talocart_platform::identity::Session,
    Attributes,
    i64,
    i64,
    i64,
) {
    let (identity, s, _, _, product, _, _) = variant_fixture(pool).await;
    talocart_platform::storefront::Storefront::new(pool.clone())
        .save_languages(&s, &s.csrf, "en", &["en".into(), "it".into()])
        .await
        .unwrap();
    let a = Attributes::new(pool.clone(), identity.clone());
    let group = a
        .save(
            &s,
            &s.csrf,
            Kind::Group,
            None,
            &values("Technical", 0),
            Some(&draft("Technical", "Tecniche")),
        )
        .await
        .unwrap();
    let attribute = a
        .save(
            &s,
            &s.csrf,
            Kind::Attribute,
            None,
            &values("Material", group),
            Some(&draft("Material", "Materiale")),
        )
        .await
        .unwrap();
    (identity, s, a, product, group, attribute)
}
#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn attributes_crud_translations_conflicts_rollback_and_links() {
    isolated(|pool, _| async move {
        let (_, s, a, product, group, attribute) = attribute_fixture(&pool).await;
        let mut input = values("Cotton <script>", 0);
        input.attribute_id = attribute.to_string();
        input.sort_order = "12".into();
        let id = a
            .save_product(
                &s,
                &s.csrf,
                product,
                None,
                &input,
                Some(&draft("Cotton <script>", "Cotone <script>")),
            )
            .await
            .unwrap();
        let italian = a
            .clone()
            .localized(Language::new(Locale::ITALIAN, Locale::ENGLISH));
        let rows = italian.public_values(product).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].attribute_name, "Materiale");
        assert_eq!(rows[0].group_name, "Tecniche");
        assert_eq!(rows[0].name, "Cotone <script>");
        assert!(matches!(
            a.save_product(&s, &s.csrf, product, None, &input, None)
                .await,
            Err(AttributeError::Conflict)
        ));
        assert!(matches!(
            a.delete(&s, &s.csrf, Kind::Attribute, attribute, 0).await,
            Err(AttributeError::InUse)
        ));
        assert!(matches!(
            a.delete(&s, &s.csrf, Kind::Group, group, 0).await,
            Err(AttributeError::InUse)
        ));
        let mut invalid = draft("Changed", "x");
        invalid.rows[1].name = "x".repeat(501);
        assert!(matches!(
            a.save_product(
                &s,
                &s.csrf,
                product,
                Some(id),
                &values("Changed", 0),
                Some(&invalid)
            )
            .await,
            Err(AttributeError::Content(_))
        ));
        assert_eq!(
            a.product_values(&s, product).await.unwrap()[0].name,
            "Cotton <script>"
        );
        // A stale editor cannot overwrite either language or ordering.
        input.name = "Linen".into();
        a.save_product(
            &s,
            &s.csrf,
            product,
            Some(id),
            &input,
            Some(&draft("Linen", "Lino")),
        )
        .await
        .unwrap();
        assert!(matches!(
            a.save_product(
                &s,
                &s.csrf,
                product,
                Some(id),
                &input,
                Some(&draft("Stale", "Vecchio"))
            )
            .await,
            Err(AttributeError::Conflict)
        ));
        assert!(matches!(
            a.remove_product(&s, &s.csrf, product, id, 0).await,
            Err(AttributeError::Conflict)
        ));
        assert_eq!(
            italian.public_values(product).await.unwrap()[0].name,
            "Lino"
        );
        a.remove_product(&s, &s.csrf, product, id, 1).await.unwrap();
        a.delete(&s, &s.csrf, Kind::Attribute, attribute, 0)
            .await
            .unwrap();
        a.delete(&s, &s.csrf, Kind::Group, group, 0).await.unwrap();
        assert!(a.public_values(product).await.unwrap().is_empty());
        let remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM catalog_product_attributes_translations")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, 0);
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn attributes_concurrent_saves_and_duplicate_assignments() {
    isolated(|pool, _| async move {
        let (_, s, a, product, group, attribute) = attribute_fixture(&pool).await;
        let mut input = values("Cotton", 0);
        input.attribute_id = attribute.to_string();
        let (first, second) = tokio::join!(
            a.save_product(&s, &s.csrf, product, None, &input, None),
            a.save_product(&s, &s.csrf, product, None, &input, None)
        );
        assert_ne!(first.is_ok(), second.is_ok());
        assert_eq!(a.product_values(&s, product).await.unwrap().len(), 1);
        let input = values("Fabric", group);
        let (first, second) = tokio::join!(
            a.save(&s, &s.csrf, Kind::Attribute, Some(attribute), &input, None),
            a.save(&s, &s.csrf, Kind::Attribute, Some(attribute), &input, None)
        );
        assert_ne!(first.is_ok(), second.is_ok());
        assert_eq!(
            a.entry(&s, Kind::Attribute, attribute)
                .await
                .unwrap()
                .revision,
            1
        );
        // Translation uniqueness and owner constraints apply even outside HTTP.
        assert!(
            a.save(
                &s,
                &s.csrf,
                Kind::Attribute,
                None,
                &values("Other", group),
                Some(&draft("Other", "Materiale"))
            )
            .await
            .is_err()
        );
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM catalog_attributes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert!(
            sqlx::query(
                "UPDATE catalog_product_attributes SET attribute_id=999 WHERE product_id=$1"
            )
            .bind(product)
            .execute(&pool)
            .await
            .is_err()
        );
    })
    .await;
}
#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn attributes_http_bilingual_preserves_fields_security_visibility_and_base() {
    isolated(|pool,_|async move {
        let (identity,s,a,product,group,attribute)=attribute_fixture(&pool).await;
        let raw=authenticated(&identity).await;let cookie=format!("talocart_session={raw}");let session=identity.session(&raw).await.unwrap().unwrap();
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        for path in ["/admin/catalog/attributes","/admin/catalog/attribute-groups","/admin/catalog/attributes/new","/admin/catalog/attribute-groups/new"] {
            assert_eq!(request(&app,"GET",path,&cookie,"",false).await.status(),StatusCode::OK,"{path}");
            assert_eq!(request(&app,"GET",path,"","",false).await.status(),StatusCode::SEE_OTHER);
        }
        let body=format!("csrf={}&name=Preserved&group_id={group}&sort_order=-1&revision=0&content_primary_locale=en&content_it_name=Conservato",session.csrf);
        let response=request(&app,"POST","/admin/catalog/attributes/new",&cookie,&body,false).await;assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let body=html(response).await;assert!(body.contains("value=\"Preserved\"")&&body.contains("value=\"Conservato\""));assert!(body.contains("aria-invalid=\"true\""));
        assert_eq!(request(&app,"POST","/admin/catalog/attributes/new",&cookie,"csrf=bad&name=Denied&sort_order=0",true).await.status(),StatusCode::FORBIDDEN);
        let path=format!("/admin/catalog/products/{product}/attributes/new");assert_eq!(request(&app,"GET",&path,&cookie,"",false).await.status(),StatusCode::OK);
        let body=format!("csrf={}&name=Cotton+%3Cscript%3E&sort_order=3&attribute_id={attribute}&content_primary_locale=en&content_it_name=Cotone+%3Cscript%3E",session.csrf);
        let r=request(&app,"POST",&path,&cookie,&body,false).await;assert_eq!(r.status(),StatusCode::SEE_OTHER);
        let location=r.headers()[header::LOCATION].to_str().unwrap().to_owned();assert_eq!(request(&app,"GET",&location,&cookie,"",false).await.status(),StatusCode::OK);
        let deletion = format!("{}/delete", location.split('?').next().unwrap());
        assert_eq!(request(&app,"GET",&deletion,&cookie,"",false).await.status(),StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(request(&app,"POST",&deletion,&cookie,&format!("csrf={}&revision=0",session.csrf),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(a.product_values(&s,product).await.unwrap().len(),1,"Deletion without explicit confirmation preserves the assignment");
        for (locale,path,heading,value) in [("en","/products/model","Specifications","Cotton"),("it","/it/products/model","Caratteristiche","Cotone")] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            for partial in [false,true] {
                let body=html(request(&app,"GET",path,"","",partial).await).await;
                assert!(body.contains(heading),"{path}: {body}");assert!(body.contains(&format!("{value} &#60;script&#62;"))||body.contains(&format!("{value} &lt;script&gt;")));assert!(!body.contains("Cotton <script>"));
            }
            let editor=html(request(&app,"GET",&format!("/admin/catalog/products/{product}?tab=attributes"),&cookie,"",false).await).await;
            assert!(editor.contains("data-active-tab=\"attributes\""));assert!(editor.contains("Material"));
        }
        let modules=html(request(&app,"GET","/admin/modules",&cookie,"",false).await).await;assert!(modules.contains("attributi descrittivi"));
        // Assignment identity cannot be used to read or update another product's value.
        let other:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('Other','OTHER-ATTR','other-attr',0) RETURNING id").fetch_one(&pool).await.unwrap();
        let id=a.product_values(&s,product).await.unwrap()[0].id;
        assert_eq!(request(&app,"GET",&format!("/admin/catalog/products/{other}/attributes/{id}"),&cookie,"",false).await.status(),StatusCode::NOT_FOUND);
        assert!(a.remove_product(&s,&s.csrf,other,id,0).await.is_err());
        sqlx::query("UPDATE catalog_products SET status='draft' WHERE id=$1").bind(product).execute(&pool).await.unwrap();assert!(a.public_values(product).await.unwrap().is_empty());
        assert_eq!(request(&app,"GET","/products/model","","",false).await.status(),StatusCode::NOT_FOUND);
    }).await;
}
#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn attributes_language_fallback_change_and_inactive_translation_retention() {
    isolated(|pool, _| async move {
        let (_, s, a, product, _, attribute) = attribute_fixture(&pool).await;
        let mut input = values("Cotton", 0);
        input.attribute_id = attribute.to_string();
        let id = a
            .save_product(
                &s,
                &s.csrf,
                product,
                None,
                &input,
                Some(&draft("Cotton", "")),
            )
            .await
            .unwrap();
        let italian = a
            .clone()
            .localized(Language::new(Locale::ITALIAN, Locale::ENGLISH));
        assert_eq!(
            italian.public_values(product).await.unwrap()[0].name,
            "Cotton"
        );
        let store = talocart_platform::storefront::Storefront::new(pool.clone());
        store
            .save_languages(&s, &s.csrf, "en", &["en".into()])
            .await
            .unwrap();
        input.name = "Wool".into();
        a.save_product(&s, &s.csrf, product, Some(id), &input, None)
            .await
            .unwrap();
        store
            .save_languages(&s, &s.csrf, "it", &["en".into(), "it".into()])
            .await
            .unwrap();
        // An old bilingual browser form cannot save under a new primary language.
        input.revision = "1".into();
        assert!(matches!(
            a.save_product(
                &s,
                &s.csrf,
                product,
                Some(id),
                &input,
                Some(&draft("Stale", "Vecchio"))
            )
            .await,
            Err(AttributeError::Content(
                talocart_platform::content::ContentError::LanguageChanged
            ))
        ));
        let rows = italian.public_values(product).await.unwrap();
        assert_eq!(rows[0].name, "Wool");
        assert_eq!(rows[0].attribute_name, "Materiale");
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn attributes_limits_database_constraints_ordering_and_pagination() {
    isolated(|pool,_|async move {
        let (identity,s,a,product,group,attribute)=attribute_fixture(&pool).await;
        // Bounded registry and assignment limits are enforced for direct SQL too.
        sqlx::query("INSERT INTO catalog_attribute_groups(name) SELECT 'Group '||i FROM generate_series(2,100) i").execute(&pool).await.unwrap();
        assert!(matches!(a.save(&s,&s.csrf,Kind::Group,None,&values("Overflow",0),None).await,Err(AttributeError::Limit)));
        sqlx::query("INSERT INTO catalog_attributes(name,group_id,sort_order) SELECT 'Attribute '||i,$1,i FROM generate_series(2,500) i").bind(group).execute(&pool).await.unwrap();
        assert!(matches!(a.save(&s,&s.csrf,Kind::Attribute,None,&values("Overflow",group),None).await,Err(AttributeError::Limit)));
        sqlx::query("INSERT INTO catalog_product_attributes(product_id,attribute_id,name,sort_order) SELECT $1,a.id,'Value',a.id::int FROM catalog_attributes a ORDER BY a.id LIMIT 100").bind(product).execute(&pool).await.unwrap();
        let mut input=values("Overflow",0);input.attribute_id="101".into();assert!(matches!(a.save_product(&s,&s.csrf,product,None,&input,None).await,Err(AttributeError::Limit)));
        assert_eq!(a.public_values(product).await.unwrap().len(),100);assert_eq!(a.public_values(product).await.unwrap()[0].attribute_id,attribute);
        let cookie=format!("talocart_session={}",authenticated(&identity).await);let app=router(test_app(pool.clone(),"Shop".into()).await);
        let body=html(request(&app,"GET","/admin/catalog/attributes?page=25",&cookie,"",false).await).await;assert_eq!(body.matches("role=\"listitem\"").count(),20);
        assert_eq!(request(&app,"GET","/admin/catalog/attributes?page=0",&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        let body=html(request(&app,"GET","/admin/catalog/attributes?page=9223372036854775807",&cookie,"",false).await).await;assert_eq!(body.matches("role=\"listitem\"").count(),20);
        // Cascading product deletion removes values and translations, never registry entries.
        sqlx::query("DELETE FROM catalog_products WHERE id=$1").bind(product).execute(&pool).await.unwrap();assert!(a.public_values(product).await.unwrap().is_empty());assert_eq!(a.entries(&s,Kind::Attribute).await.unwrap().len(),500);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn attributes_read_only_revocation_and_csrf_cover_every_mutation() {
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let (identity, owner, a, product, group, attribute) = attribute_fixture(&pool).await;
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Specifications reader",
                &[Permission::ProductsRead],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "attributes@example.test",
                "correct horse battery".into(),
                Role::Custom(role.clone()),
            )
            .await
            .unwrap();
        let raw = login_as(&identity, "attributes@example.test").await;
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let mut input = values("Cotton", 0);
        input.attribute_id = attribute.to_string();
        let assignment = a
            .save_product(&owner, &owner.csrf, product, None, &input, None)
            .await
            .unwrap();
        for path in [
            format!("/admin/catalog/attributes/{attribute}"),
            format!("/admin/catalog/attribute-groups/{group}"),
            format!("/admin/catalog/products/{product}/attributes/{assignment}"),
        ] {
            let body = html(request(&app, "GET", &path, &cookie, "", false).await).await;
            assert!(body.contains("readonly"));
            assert!(
                !body
                    .split("id=\"main\"")
                    .nth(1)
                    .unwrap()
                    .split("</main>")
                    .next()
                    .unwrap()
                    .contains("type=\"submit\"")
            );
            for suffix in ["", "/delete"] {
                assert_eq!(
                    request(
                        &app,
                        "POST",
                        &format!("{path}{suffix}"),
                        &cookie,
                        &format!(
                            "csrf={}&name=Denied&sort_order=0&confirm=true&revision=0",
                            reader.csrf
                        ),
                        true
                    )
                    .await
                    .status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
        for path in [
            "/admin/catalog/attributes/new".to_owned(),
            "/admin/catalog/attribute-groups/new".to_owned(),
            format!("/admin/catalog/products/{product}/attributes/new"),
        ] {
            assert_eq!(
                request(&app, "GET", &path, &cookie, "", false)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                request(
                    &app,
                    "POST",
                    &path,
                    &cookie,
                    &format!("csrf={}&name=Denied&sort_order=0", reader.csrf),
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        let raw = authenticated(&identity).await;
        let owner_cookie = format!("talocart_session={raw}");
        for path in [
            format!("/admin/catalog/attributes/{attribute}"),
            format!("/admin/catalog/attribute-groups/{group}"),
            format!("/admin/catalog/products/{product}/attributes/{assignment}"),
        ] {
            for suffix in ["", "/delete"] {
                assert_eq!(
                    request(
                        &app,
                        "POST",
                        &format!("{path}{suffix}"),
                        &owner_cookie,
                        "csrf=bad&name=Denied&sort_order=0&confirm=true&revision=0",
                        true
                    )
                    .await
                    .status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
        identity
            .update_role(
                &owner,
                &owner.csrf,
                &role,
                "No specifications",
                &[Permission::AdminRead],
            )
            .await
            .unwrap();
        assert!(a.entries(&reader, Kind::Attribute).await.is_err());
        assert!(a.entry(&reader, Kind::Attribute, attribute).await.is_err());
        assert!(a.product_value(&reader, product, assignment).await.is_err());
        assert!(a.available_entries(&reader, product).await.is_err());
        let products = Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        assert!(products.editor_entry(&reader, product).await.is_err());

        assert_eq!(
            a.product_values(&owner, product).await.unwrap()[0].name,
            "Cotton"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn attributes_group_and_product_order_are_stable_for_all_variants() {
    isolated(|pool,_|async move {
        let (identity,s,products,variants,product,options,ids)=variant_fixture(&pool).await;
        let a=Attributes::new(pool.clone(),identity.clone());
        variants.configure(&s,&s.csrf,product,&options).await.unwrap();
        let variant=variants.save(&s,&s.csrf,product,None,&variant_values("ATTR-RED-S",&[ids[0],ids[2]],"12","7")).await.unwrap();
        let original_product=products.entry(&s,product).await.unwrap();let original_variant=variants.entry(&s,product,variant).await.unwrap();
        let first=a.save(&s,&s.csrf,Kind::Group,None,&values("First",0),None).await.unwrap();
        let mut input=values("Second",0);input.sort_order="1".into();let second=a.save(&s,&s.csrf,Kind::Group,None,&input,None).await.unwrap();
        let mut expected=vec![];
        for (name,group,attribute_order,value_order) in [("Alpha",first,"90","0"),("Beta",first,"1","2"),("Gamma",second,"0","0")] {
            let mut input=values(name,group);input.sort_order=attribute_order.into();let attribute=a.save(&s,&s.csrf,Kind::Attribute,None,&input,None).await.unwrap();expected.push(attribute);
            input.name=format!("Value {name}");input.attribute_id=attribute.to_string();input.sort_order=value_order.into();a.save_product(&s,&s.csrf,product,None,&input,None).await.unwrap();
        }
        let rows=a.public_values(product).await.unwrap();assert_eq!(rows.iter().map(|r|r.attribute_id).collect::<Vec<_>>(),expected);
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        for path in ["/products/model".to_owned(),format!("/products/model?value_1={}&value_2={}",ids[0],ids[2])] {
            for partial in [false,true] { let body=html(request(&app,"GET",&path,"","",partial).await).await;assert!(body.contains("Value Alpha"));assert!(body.find("Value Alpha").unwrap()<body.find("Value Beta").unwrap());assert!(body.find("Value Beta").unwrap()<body.find("Value Gamma").unwrap()); }
        }
        let product_after=products.entry(&s,product).await.unwrap();let variant_after=variants.entry(&s,product,variant).await.unwrap();
        assert_eq!((original_product.model,original_product.price_cents,original_product.stock),(product_after.model,product_after.price_cents,product_after.stock));
        assert_eq!((original_variant.sku,original_variant.price_cents,original_variant.stock),(variant_after.sku,variant_after.price_cents,variant_after.stock));
        // Indexes for assignments and ownership must be available to the planner.
        for index in ["catalog_product_attributes_product_id_attribute_id_key","catalog_product_attributes_attribute_idx","catalog_attributes_group_idx"] {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_indexes WHERE schemaname=current_schema() AND indexname=$1)").bind(index).fetch_one(&pool).await.unwrap();assert!(exists,"{index}");
        }
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn attributes_edit_identity_and_available_choices_preserve_ownership() {
    isolated(|pool, _| async move {
        let (identity, owner, a, product, group, attribute) = attribute_fixture(&pool).await;
        let second = a.save(&owner, &owner.csrf, Kind::Attribute, None, &values("Weight", group), None).await.unwrap();
        let mut input = values("Cotton", 0);
        input.attribute_id = attribute.to_string();
        let id = a.save_product(&owner, &owner.csrf, product, None, &input, Some(&draft("Cotton", "Cotone"))).await.unwrap();
        assert_eq!(a.entry(&owner, Kind::Attribute, second).await.unwrap().name, "Weight");
        assert!(matches!(a.entry(&owner, Kind::Attribute, i64::MAX).await, Err(AttributeError::NotFound)));
        let choices = a.available_entries(&owner, product).await.unwrap();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, second);
        assert!(matches!(a.available_entries(&owner, i64::MAX).await, Err(AttributeError::NotFound)));
        assert!(matches!(a.product_value(&owner, product, i64::MAX).await, Err(AttributeError::NotFound)));
        assert!(matches!(a.product_value(&owner, i64::MAX, id).await, Err(AttributeError::NotFound)));
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let session = identity.session(&raw).await.unwrap().unwrap();
        let state = test_app(pool.clone(), "Shop".into()).await;
        let app = router(state);
        let path = format!("/admin/catalog/products/{product}/attributes/{id}");
        for locale in ["en", "it"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            for (order, revision, status) in [("-1", "0", StatusCode::UNPROCESSABLE_ENTITY), ("0", "999", StatusCode::CONFLICT)] {
                // A disabled select is omitted natively; a forged identity is equally untrusted.
                for forged in [String::new(), format!("&attribute_id={second}")] {
                    let form = format!("csrf={}&name=Pending&sort_order={order}&revision={revision}&content_primary_locale=en&content_it_name=In+attesa{forged}", session.csrf);
                    let response = request(&app, "POST", &path, &cookie, &form, false).await;
                    assert_eq!(response.status(), status);
                    let body = html(response).await;
                    assert!(body.contains(&format!("value=\"{attribute}\" selected")), "Owned identity survives rejection");
                    assert!(body.contains("value=\"Pending\"") && body.contains("value=\"In attesa\""));
                    assert!(!body.contains(">Weight</option>"));
                }
            }
            let body = html(request(&app, "GET", &format!("/admin/catalog/products/{product}/attributes/new"), &cookie, "", false).await).await;
            assert!(body.contains(">Weight</option>"));
            assert!(!body.contains(">Material</option>"));
        }
        let products = Products::new(pool.clone(), identity.clone(), test_media(&pool).await)
            .localized(Language::new(Locale::ITALIAN, Locale::ENGLISH));
        let editor = products.editor_entry(&owner, product).await.unwrap();
        assert_eq!(editor.product.id, product);
        assert_eq!(editor.attributes.len(), 1);
        assert_eq!(editor.attributes[0].name, "Cotone");
        assert_eq!(editor.attributes[0].attribute_name, "Materiale");
        assert!(products.editor_entry(&owner, i64::MAX).await.is_err());
        let persisted = a.product_value(&owner, product, id).await.unwrap();
        assert_eq!(persisted.attribute_id, attribute);
        assert_eq!(persisted.name, "Cotton");
        assert_eq!(persisted.revision, 0);
    }).await;
}
