use super::*;
use talocart_commerce::products::{BulkAction, BulkActions, Field, ProductError, ProductSelection};
use talocart_platform::identity::{Permission, Role};

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bulk_category_tree_permissions_http_replacement_and_data_preservation() {
    isolated(|pool, _| async move {
        let (identity, owner, products, _, first, _, _) = variant_fixture(&pool).await;
        let catalog=talocart_commerce::Catalog::new(pool.clone(),identity.clone(),test_media(&pool).await);
        let root=catalog.save(&owner,&owner.csrf,talocart_commerce::Kind::Category,None,&taxonomy_values("Z <root> & name","bulk-root",None)).await.unwrap();
        let child=catalog.save(&owner,&owner.csrf,talocart_commerce::Kind::Category,None,&taxonomy_values("Child","bulk-child",Some(root))).await.unwrap();
        let leaf=catalog.save(&owner,&owner.csrf,talocart_commerce::Kind::Category,None,&taxonomy_values("Leaf","bulk-leaf",Some(child))).await.unwrap();
        let sibling=catalog.save(&owner,&owner.csrf,talocart_commerce::Kind::Category,None,&taxonomy_values("A sibling","bulk-sibling",Some(root))).await.unwrap();
        sqlx::query("UPDATE catalog_categories SET sort_order=9 WHERE id=$1").bind(sibling).execute(&pool).await.unwrap();
        let second=products.save(&owner,&owner.csrf,None,&product_values("CATEGORY-SECOND","category-second")).await.unwrap();
        let untouched=products.save(&owner,&owner.csrf,None,&product_values("KEEP","keep")).await.unwrap();
        let role=identity.create_role(&owner,&owner.csrf,"Category product editor",&[Permission::ProductsWrite]).await.unwrap();
        identity.create_administrator(&owner,&owner.csrf,"category-editor@example.test","correct horse editor".into(),Role::Custom(role)).await.unwrap();
        let (_,anon)=identity.anonymous().await.unwrap();
        let raw=identity.login(&anon,&anon.csrf,"category-editor@example.test","correct horse editor".into()).await.unwrap();
        let editor=identity.session(&raw).await.unwrap().unwrap();
        assert!(!editor.allows(Permission::CategoriesRead));assert!(!editor.allows(Permission::CategoriesWrite));
        let tree=catalog.product_category_tree(&editor).await.unwrap();
        let branch:Vec<_>=tree.iter().filter(|row|[root,child,leaf,sibling].contains(&row.id)).collect();
        assert_eq!(branch.iter().map(|r|r.id).collect::<Vec<_>>(),[root,child,leaf,sibling]);
        assert_eq!(branch.iter().map(|r|r.depth).collect::<Vec<_>>(),[0,1,2,1]);
        assert_eq!(branch[2].path,"Z <root> & name / Child / Leaf");
        let cookie=format!("talocart_session={raw}");let app=router(test_app(pool.clone(),"Shop".into()).await);
        let form=format!("csrf={}&ids={first}&ids={second}&action=category&category_id={leaf}&return_to=%2Fadmin%2Fcatalog%2Fproducts%3Fsort%3Did%26dir%3Ddesc%26per_page%3D10",editor.csrf);
        let preview=request(&app,"POST","/admin/catalog/products/bulk",&cookie,&form,false).await;
        assert_eq!(preview.status(),StatusCode::OK);let body=html(preview).await;
        assert!(body.contains("Assegna categoria"));assert!(body.contains("Albero delle categorie"));
        assert!(body.contains("Z &#60;root&#62; &#38; name"));assert!(!body.contains("<root>"));
        assert!(body.contains("Z &#60;root&#62; &#38; name / Child / Leaf"));
        assert!(body.contains(&format!("name=\"category_id\" value=\"{leaf}\"")));
        assert!(body.contains(" checked"));assert!(!body.contains("<table"));
        let before:Vec<String>=sqlx::query_scalar("SELECT (to_jsonb(p)-'category_id')::text FROM catalog_products p ORDER BY id").fetch_all(&pool).await.unwrap();
        assert_eq!(request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&form,false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&format!("{form}&confirm=true").replacen(&format!("csrf={}",editor.csrf),"csrf=bad",1),false).await.status(),StatusCode::FORBIDDEN);
        for _ in 0..2 {
            let response=request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&format!("{form}&confirm=true"),false).await;
            assert_eq!(response.status(),StatusCode::SEE_OTHER);
            let location=response.headers()[header::LOCATION].to_str().unwrap();
            assert_eq!(location,"/admin/catalog/products?sort=id&dir=desc&per_page=10&bulk_category=1");
            assert!(html(request(&app,"GET",location,&cookie,"",false).await).await.contains("Categoria assegnata ai prodotti selezionati."));
        }
        for id in [first,second] {assert_eq!(products.entry(&owner,id).await.unwrap().category_id,Some(leaf));}
        assert_eq!(products.entry(&owner,untouched).await.unwrap().category_id,None);
        assert_eq!(before,sqlx::query_scalar::<_,String>("SELECT (to_jsonb(p)-'category_id')::text FROM catalog_products p ORDER BY id").fetch_all(&pool).await.unwrap());
        assert_eq!(products.entry(&owner,first).await.unwrap().status,"published");
        assert_eq!(products.entry(&owner,second).await.unwrap().status,"draft");
        assert!(products.public_entry("model").await.is_ok());
        // An active child below an inactive parent remains selectable, as in the single form.
        sqlx::query("UPDATE catalog_categories SET active=false WHERE id=$1").bind(root).execute(&pool).await.unwrap();
        let tree=catalog.product_category_tree(&editor).await.unwrap();
        assert!(tree.iter().any(|row|row.id==root && !row.active));assert!(tree.iter().any(|row|row.id==leaf && row.active));
        products.assign_category_selected(&editor,&editor.csrf,&ProductSelection::new(vec![first,second]).unwrap(),leaf).await.unwrap();
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bulk_category_missing_invalid_inactive_and_concurrent_assignments_are_atomic() {
    isolated(|pool, _| async move {
        let (identity,owner,products,_,published,_,_)=variant_fixture(&pool).await;
        let draft=products.save(&owner,&owner.csrf,None,&product_values("DRAFT","draft")).await.unwrap();
        let existing=products.entry(&owner,published).await.unwrap().category_id.unwrap();
        let inactive:i64=sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,active) VALUES('Inactive','bulk-inactive',false) RETURNING id").fetch_one(&pool).await.unwrap();
        let raw=authenticated(&identity).await;let session=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let mixed=ProductSelection::new(vec![draft,published]).unwrap();
        assert!(matches!(products.assign_category_selected(&owner,&owner.csrf,&mixed,inactive).await,Err(ProductError::Publication)));
        assert_eq!(products.entry(&owner,draft).await.unwrap().category_id,None);
        assert_eq!(products.entry(&owner,published).await.unwrap().category_id,Some(existing));
        for category in [0,-1,i64::MAX] {
            assert!(matches!(products.assign_category_selected(&owner,&owner.csrf,&mixed,category).await,Err(ProductError::Invalid(fields)) if fields==[Field::Category]));
        }
        for category in ["","abc","0","-1","9223372036854775807"] {
            let body=format!("csrf={}&ids={draft}&action=category&confirm=true&category_id={category}",session.csrf);
            let response=request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&body,false).await;
            assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
            assert!(html(response).await.contains("id=\"bulk-category-error\""));
        }
        products.assign_category_selected(&owner,&owner.csrf,&ProductSelection::new(vec![draft]).unwrap(),inactive).await.unwrap();
        assert_eq!(products.entry(&owner,draft).await.unwrap().status,"draft");
        let removed:i64=sqlx::query_scalar("INSERT INTO catalog_categories(name,slug) VALUES('Removed','bulk-removed') RETURNING id").fetch_one(&pool).await.unwrap();
        sqlx::query("DELETE FROM catalog_categories WHERE id=$1").bind(removed).execute(&pool).await.unwrap();
        assert!(products.assign_category_selected(&owner,&owner.csrf,&mixed,removed).await.is_err());
        let missing=ProductSelection::new(vec![draft,i64::MAX]).unwrap();
        assert!(matches!(products.assign_category_selected(&owner,&owner.csrf,&missing,existing).await,Err(ProductError::NotFound)));
        assert_eq!(products.entry(&owner,draft).await.unwrap().category_id,Some(inactive));
        let other:i64=sqlx::query_scalar("INSERT INTO catalog_categories(name,slug) VALUES('Other','bulk-other') RETURNING id").fetch_one(&pool).await.unwrap();
        let (a,b)=tokio::join!(products.assign_category_selected(&owner,&owner.csrf,&mixed,existing),products.assign_category_selected(&owner,&owner.csrf,&mixed,other));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(products.entry(&owner,draft).await.unwrap().category_id,products.entry(&owner,published).await.unwrap().category_id);
        // Non-category actions ignore an unused category field, including in the native no-JS form.
        assert_eq!(request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&format!("csrf={}&ids={draft}&action=draft&confirm=true&category_id=bad",session.csrf),false).await.status(),StatusCode::SEE_OTHER);
        assert!(matches!(BulkActions::new(&[BulkAction::AssignCategory], None), Err(ProductError::Invalid(_))));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn bulk_combined_actions_final_state_atomicity_conflicts_and_replay() {
    isolated(|pool, _| async move {
        let (identity,owner,products,_,published,_,_)=variant_fixture(&pool).await;
        let first=products.save(&owner,&owner.csrf,None,&product_values("COMBINED-FIRST","combined-first")).await.unwrap();
        let second=products.save(&owner,&owner.csrf,None,&product_values("COMBINED-SECOND","combined-second")).await.unwrap();
        let active:i64=sqlx::query_scalar("INSERT INTO catalog_categories(name,slug) VALUES('Target','combined-target') RETURNING id").fetch_one(&pool).await.unwrap();
        let inactive:i64=sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,active) VALUES('Old','combined-old',false) RETURNING id").fetch_one(&pool).await.unwrap();
        let class:i64=sqlx::query_scalar("SELECT tax_class_id FROM catalog_products WHERE id=$1").bind(published).fetch_one(&pool).await.unwrap();
        sqlx::query("UPDATE catalog_products SET category_id=$1,tax_class_id=$2 WHERE id=ANY($3)").bind(inactive).bind(class).bind(vec![first,second]).execute(&pool).await.unwrap();
        let raw=authenticated(&identity).await;let session=identity.session(&raw).await.unwrap().unwrap();
        let cookie=format!("talocart_session={raw}");let app=router(test_app(pool.clone(),"Shop".into()).await);
        let base=format!("csrf={}&ids={first}&ids={second}&confirm=true",session.csrf);
        let snapshot=||sqlx::query_scalar::<_,String>("SELECT to_jsonb(p)::text FROM catalog_products p ORDER BY id").fetch_all(&pool);
        let before=snapshot().await.unwrap();
        for actions in ["", "&action=publish&action=draft", "&action=delete&action=category", "&action=publish&action=publish", "&action=category&action=unknown", "&action=category&action=publish"] {
            let form=format!("{base}{actions}");
            assert_eq!(request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&form,false).await.status(),StatusCode::UNPROCESSABLE_ENTITY,"{actions}");
            assert_eq!(snapshot().await.unwrap(),before);
        }
        // Publication alone fails on the old inactive category, but the combined final state is valid.
        assert_eq!(request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&format!("{base}&action=publish"),false).await.status(),StatusCode::CONFLICT);
        sqlx::query("UPDATE catalog_products SET tax_class_id=NULL WHERE id=$1").bind(second).execute(&pool).await.unwrap();
        let before_failure=snapshot().await.unwrap();
        let combined=format!("{base}&action=category&action=publish&category_id={active}");
        let failure=request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&combined,false).await;
        assert_eq!(failure.status(),StatusCode::CONFLICT);
        let body=html(failure).await;assert!(body.contains("value=\"publish\" data-bulk-action checked"));assert!(body.contains("value=\"category\" data-bulk-action checked"));
        assert_eq!(snapshot().await.unwrap(),before_failure,"Category and status both roll back");
        sqlx::query("UPDATE catalog_products SET tax_class_id=$1 WHERE id=$2").bind(class).bind(second).execute(&pool).await.unwrap();
        let untouched=products.entry(&owner,published).await.unwrap();
        let unrelated_before:Vec<String>=sqlx::query_scalar("SELECT (to_jsonb(p)-'category_id'-'status')::text FROM catalog_products p ORDER BY id").fetch_all(&pool).await.unwrap();
        for _ in 0..2 {
            let response=request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&combined,false).await;
            assert_eq!(response.status(),StatusCode::SEE_OTHER);assert_eq!(response.headers()[header::LOCATION],"/admin/catalog/products?bulk_applied=1");
            for id in [first,second] {let entry=products.entry(&owner,id).await.unwrap();assert_eq!(entry.status,"published");assert_eq!(entry.category_id,Some(active));}
        }
        assert_eq!(products.entry(&owner,published).await.unwrap().category_id,untouched.category_id);
        assert_eq!(unrelated_before,sqlx::query_scalar::<_,String>("SELECT (to_jsonb(p)-'category_id'-'status')::text FROM catalog_products p ORDER BY id").fetch_all(&pool).await.unwrap());
        assert!(products.public_entry("combined-first").await.is_ok());
        // Draft + inactive category is valid because guards check the final state, regardless of submitted order.
        assert_eq!(request(&app,"POST","/admin/catalog/products/bulk/apply",&cookie,&format!("{base}&action=category&action=draft&category_id={inactive}"),false).await.status(),StatusCode::SEE_OTHER);
        for id in [first,second] {let entry=products.entry(&owner,id).await.unwrap();assert_eq!(entry.status,"draft");assert_eq!(entry.category_id,Some(inactive));}
    }).await;
}
