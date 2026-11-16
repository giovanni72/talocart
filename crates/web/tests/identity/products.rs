use super::*;

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_page_size_preference_survives_return_without_query() {
    isolated(|pool, _| async move {
        let (identity, _, _, _, _, _, _) = variant_fixture(&pool).await;
        let session_cookie = format!("talocart_session={}", authenticated(&identity).await);
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            let response = request(
                &app,
                "GET",
                "/admin/catalog/products?per_page=50",
                &session_cookie,
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let preference = response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .find(|value| value.starts_with("talocart_admin_product_page_size="))
                .expect("explicit page size must persist in an admin preference cookie");
            assert!(preference.contains("Path=/admin"));
            assert!(preference.contains("HttpOnly") && preference.contains("SameSite=Lax"));
            assert!(preference.contains("Max-Age=15552000"));
            let cookie = format!(
                "{session_cookie}; {}; talocart_catalog_page_size=84",
                preference.split(';').next().unwrap()
            );
            let body =
                html(request(&app, "GET", "/admin/catalog/products", &cookie, "", false).await)
                    .await;
            assert!(
                body.contains("value=\"50\" selected"),
                "{locale}: return from editor keeps size"
            );
            assert!(!body.contains("class=\"admin-lead\""));
            let body = html(
                request(
                    &app,
                    "GET",
                    "/admin/catalog/products?per_page=20",
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(body.contains("value=\"20\" selected"), "explicit URL wins");
            assert_eq!(
                request(
                    &app,
                    "GET",
                    "/admin/catalog/products?per_page=84",
                    &cookie,
                    "",
                    false
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
            for value in [
                "invalid",
                "84",
                "0",
                "50; talocart_admin_product_page_size=100",
            ] {
                let cookie = format!("{session_cookie}; talocart_admin_product_page_size={value}");
                let body =
                    html(request(&app, "GET", "/admin/catalog/products", &cookie, "", false).await)
                        .await;
                assert!(
                    body.contains("value=\"20\" selected"),
                    "invalid or ambiguous preference ignored"
                );
            }
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_five_hundred_page_size_preserves_bounds_and_filters() {
    isolated(|pool, _| async move {
        let (identity, _, _, _, _, _, _) = variant_fixture(&pool).await;
        sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents) SELECT 'Large page item ' || lpad(i::text,3,'0'),'LARGE-' || lpad(i::text,3,'0'),'large-page-' || i,100 FROM generate_series(1,515) i")
            .execute(&pool).await.unwrap();
        let cookie = format!("talocart_session={}", authenticated(&identity).await);
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            for (page, count, first, last) in [(1,500,1,500),(2,15,501,515),(99,15,501,515)] {
                let response = fiscal_request(&app,"GET",&format!("/admin/catalog/products?q=Large&type=simple&sort=model&dir=asc&per_page=500&page={page}"),&cookie,"",false).await;
                assert_eq!(response.status(), StatusCode::OK);
                // The shared helper's 200 kB buffer is sized for the smaller pages.
                let body = String::from_utf8(to_bytes(response.into_body(), 2_000_000).await.expect("500-row HTML within 2 MB").to_vec()).expect("UTF-8 product list");
                assert_eq!(body.matches("scope=\"row\"").count(), count);
                assert!(body.contains("value=\"500\" selected"));
                assert!(body.contains(&format!(">LARGE-{first:03}</td>")));
                assert!(body.contains(&format!(">LARGE-{last:03}</td>")));
                assert!(body.contains("type=simple&#38;model=&#38;category=&#38;brand=&#38;sort=model&#38;dir=desc&#38;per_page=500"));
                let label = if locale == "it" { "Prodotti" } else { "Products" };
                assert!(body.contains(&format!("{label} {first}–{last}")));
            }
            for invalid in ["499", "501", "65536"] {
                assert_eq!(fiscal_request(&app,"GET",&format!("/admin/catalog/products?per_page={invalid}"),&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
            }
            let empty = html(fiscal_request(&app,"GET","/admin/catalog/products?q=no-match&per_page=500",&cookie,"",false).await).await;
            assert_eq!(empty.matches("scope=\"row\"").count(), 0);
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_page_sizes_totals_ranges_and_filtered_pages() {
    isolated(|pool, _| async move {
        let (identity, _, _, _, _, _, _) = variant_fixture(&pool).await;
        sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents) SELECT 'Page item ' || lpad(i::text,3,'0'),'PAGE-' || lpad(i::text,3,'0'),'page-' || i,100 FROM generate_series(1,115) i")
            .execute(&pool).await.unwrap();
        let cookie = format!("talocart_session={}", authenticated(&identity).await);
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (size, page, count, range, pages) in [
            (10,1,10,"1–10",12), (10,12,5,"111–115",12),
            (20,1,20,"1–20",6), (20,2,20,"21–40",6), (20,6,15,"101–115",6),
            (50,2,50,"51–100",3), (50,3,15,"101–115",3),
            (100,1,100,"1–100",2), (100,2,15,"101–115",2),
        ] {
            let response = fiscal_request(&app,"GET",&format!("/admin/catalog/products?q=Page&sort=model&dir=asc&per_page={size}&page={page}"),&cookie,"",false).await;
            assert_eq!(response.status(), StatusCode::OK);
            // A full 100-row table includes per-column visibility attributes.
            // Keep the small shared helper bounded and allow this larger response explicitly.
            let body = String::from_utf8(to_bytes(response.into_body(), 1_000_000).await.expect("100-row product table within 1 MB").to_vec()).expect("UTF-8 product table");
            assert_eq!(body.matches("scope=\"row\"").count(), count);
            assert!(body.contains(&format!("Prodotti {range} di 115")));
            assert!(body.contains(&format!("Pagina {page} di {pages}")));
            assert!(body.contains(&format!("value=\"{size}\" selected")));
            let (first, last) = range.split_once('–').unwrap();
            let rows = body.split("<tbody>").nth(1).unwrap().split("</tbody>").next().unwrap();
            assert!(rows.contains(&format!(">PAGE-{:03}</td>", first.parse::<usize>().unwrap())));
            assert!(rows.contains(&format!(">PAGE-{:03}</td>", last.parse::<usize>().unwrap())));
            assert_eq!(body.contains("rel=\"next\""), page < pages);
            assert_eq!(body.contains("rel=\"prev\""), page > 1);
        }
        let default = html(fiscal_request(&app,"GET","/admin/catalog/products?q=Page",&cookie,"",false).await).await;
        assert!(default.contains("Prodotti 1–20 di 115"));
        assert!(default.find("class=\"pagination shared-pagination\"").unwrap() < default.find("class=\"ui-table-region\"").unwrap());
        let filtered = html(fiscal_request(&app,"GET","/admin/catalog/products?q=Page&type=simple&model=PAGE-01&status=draft&sort=model&dir=desc&per_page=10&page=99",&cookie,"",false).await).await;
        assert_eq!(filtered.matches("scope=\"row\"").count(),10);
        assert!(filtered.contains("Prodotti 1–10 di 10"));
        assert!(filtered.contains("Pagina 1 di 1"));
        assert!(filtered.contains("type=simple&#38;model=PAGE-01&#38;category=&#38;brand=&#38;sort=model&#38;dir=asc&#38;per_page=10"));
        for (field,value) in [("q","Page"),("type","simple"),("model","PAGE-01"),("status","draft"),("sort","model"),("dir","desc")] {
            let form = filtered.split("data-product-page-size").nth(1).unwrap().split("</form>").next().unwrap();
            assert!(form.contains(&format!("name=\"{field}\" value=\"{value}\"")));
            assert!(!form.contains("name=\"page\""), "Changing page size starts at page one");
        }
        let empty = html(fiscal_request(&app,"GET","/admin/catalog/products?q=no-matching-product&per_page=50&page=6",&cookie,"",false).await).await;
        assert!(empty.contains("Prodotti 0 di 0"));
        assert!(empty.contains("Pagina 1 di 1"));
        assert!(empty.contains("id=\"product-page-size\""));
        assert!(!empty.contains("rel=\"next\"") && !empty.contains("rel=\"prev\""));
        let htmx = html(fiscal_request(&app,"GET","/admin/catalog/products?q=Page&per_page=10&page=2",&cookie,"",true).await).await;
        assert!(htmx.contains("Prodotti 11–20 di 115") && htmx.contains("Pagina 2 di 12"));
        sqlx::query("DELETE FROM catalog_products WHERE model >= 'PAGE-101' AND model LIKE 'PAGE-%'").execute(&pool).await.unwrap();
        let reduced = html(fiscal_request(&app,"GET","/admin/catalog/products?q=Page&per_page=20&page=6",&cookie,"",false).await).await;
        assert!(reduced.contains("Prodotti 81–100 di 100") && reduced.contains("Pagina 5 di 5"));
        assert_eq!(reduced.matches("scope=\"row\"").count(),20);
        sqlx::query("DELETE FROM catalog_products").execute(&pool).await.unwrap();
        let empty_catalog = html(fiscal_request(&app,"GET","/admin/catalog/products?per_page=100",&cookie,"",false).await).await;
        assert!(empty_catalog.contains("Prodotti 0 di 0") && empty_catalog.contains("Il primo prodotto."));
        for size in ["0","-1","11","101","100000","abc","9223372036854775807"] {
            assert_eq!(fiscal_request(&app,"GET",&format!("/admin/catalog/products?per_page={size}"),&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        }
        assert_eq!(fiscal_request(&app,"GET","/admin/catalog/products?per_page=100&page=9223372036854775807",&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(fiscal_request(&app,"GET","/admin/catalog/products?per_page=100","","",false).await.status(),StatusCode::SEE_OTHER);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_filter_selects_use_existing_ids_and_direct_categories() {
    isolated(|pool, _| async move {
        let (identity, session, products, _, _, _, _) = variant_fixture(&pool).await;
        let category: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug) VALUES ('Same <category> & name','same-category') RETURNING id").fetch_one(&pool).await.unwrap();
        let other_category: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,active) VALUES ('Same <category> & name','other-category',false) RETURNING id").fetch_one(&pool).await.unwrap();
        let child: i64 = sqlx::query_scalar("INSERT INTO catalog_categories(name,slug,parent_id) VALUES ('Child','child-category',$1) RETURNING id").bind(category).fetch_one(&pool).await.unwrap();
        let brand: i64 = sqlx::query_scalar("INSERT INTO catalog_brands(name,slug) VALUES ('Same <brand> & name','same-brand') RETURNING id").fetch_one(&pool).await.unwrap();
        let other_brand: i64 = sqlx::query_scalar("INSERT INTO catalog_brands(name,slug,active) VALUES ('Same <brand> & name extra','other-brand',false) RETURNING id").fetch_one(&pool).await.unwrap();
        for (model, category, brand) in [("SELECT-A",category,brand),("SELECT-B",other_category,brand),("SELECT-C",category,other_brand),("SELECT-D",child,brand)] {
            let mut values = product_values(model, &model.to_lowercase());
            values.category_id = category.to_string(); values.brand_id = brand.to_string();
            products.save(&session,&session.csrf,None,&values).await.unwrap();
        }
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let body = html(fiscal_request(&app,"GET",&format!("/admin/catalog/products?category={category}&brand={brand}"),&cookie,"",false).await).await;
        assert!(body.contains("<select class=\"ui-control ui-control--compact\" id=\"product-category\""), "Category must be an existing-choice select");
        assert!(body.contains("<select class=\"ui-control ui-control--compact\" id=\"product-brand\""), "Brand must be an existing-choice select");
        for (field,id,escaped) in [("category",category,"Same &#60;category&#62; &#38; name"),("brand",brand,"Same &#60;brand&#62; &#38; name")] {
            let choices = body.split(&format!("<select class=\"ui-control ui-control--compact\" id=\"product-{field}\"")).nth(1).unwrap().split("</select>").next().unwrap();
            assert!(choices.contains(&format!("value=\"{id}\" selected")));
            assert!(choices.contains(escaped));
            assert!(choices.contains("disattivat"), "Inactive choices remain available in admin");
        }
        let rows = body.split("<tbody>").nth(1).unwrap().split("</tbody>").next().unwrap();
        assert_eq!(rows.matches("scope=\"row\"").count(),1,"Same names and child categories must not match other IDs");
        assert!(rows.contains(">SELECT-A</td>"));
        let inactive = html(fiscal_request(&app,"GET",&format!("/admin/catalog/products?category={other_category}&brand={brand}"),&cookie,"",false).await).await;
        assert!(inactive.contains(">SELECT-B</td>"));
        assert_eq!(inactive.matches("scope=\"row\"").count(),1);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_header_sorting_orders_all_columns_and_rejects_invalid_keys() {
    isolated(|pool, _| async move {
        let (identity, session, products, variants, product, options, ids) =
            variant_fixture(&pool).await;
        variants
            .configure(&session, &session.csrf, product, &options)
            .await
            .unwrap();
        for (model, combination, price, stock, active) in [
            ("SORT-V1", [ids[0], ids[2]], "8", "3", true),
            ("SORT-V2", [ids[1], ids[2]], "6", "4", true),
            ("SORT-V3", [ids[1], ids[3]], "1", "999", false),
        ] {
            let mut v = variant_values(model, &combination, price, stock);
            v.active = active;
            variants
                .save(&session, &session.csrf, product, None, &v)
                .await
                .unwrap();
        }
        let refs = products.entry(&session, product).await.unwrap();
        let mut created = Vec::new();
        for (name, model, price, stock, status, category, brand) in [
            ("Order A", "Z", "10", "9", "draft", None, None),
            (
                "Order B",
                "A",
                "2",
                "2",
                "published",
                refs.category_id,
                refs.brand_id,
            ),
            (
                "Order C",
                "M",
                "30",
                "0",
                "published",
                refs.category_id,
                None,
            ),
        ] {
            let mut v = product_values(model, &format!("sort-{}", model.to_lowercase()));
            v.name = name.into();
            v.price = price.into();
            v.stock = stock.into();
            v.status = status.into();
            v.category_id = category.map(|id| id.to_string()).unwrap_or_default();
            v.brand_id = brand.map(|id| id.to_string()).unwrap_or_default();
            v.tax_class_id = refs.tax_class_id.unwrap().to_string();
            created.push(
                products
                    .save(&session, &session.csrf, None, &v)
                    .await
                    .unwrap(),
            );
        }
        let [a, b, c] = created.as_slice() else {
            panic!("Three fixtures expected")
        };
        let (a, b, c, v) = (*a, *b, *c, product);
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        for (key, asc, desc) in [
            ("id", vec![v, a, b, c], vec![c, b, a, v]),
            ("image", vec![a, b, c, v], vec![a, b, c, v]),
            ("type", vec![a, b, c, v], vec![v, a, b, c]),
            ("name", vec![a, b, c, v], vec![v, c, b, a]),
            ("model", vec![b, c, v, a], vec![a, v, c, b]),
            ("category", vec![b, c, v, a], vec![b, c, v, a]),
            ("brand", vec![b, v, a, c], vec![b, v, a, c]),
            ("price", vec![b, v, a, c], vec![c, a, v, b]),
            ("variants", vec![v, a, b, c], vec![v, a, b, c]),
            ("stock", vec![c, b, v, a], vec![a, v, b, c]),
            ("status", vec![a, b, c, v], vec![b, c, v, a]),
        ] {
            for (dir, expected, aria) in [("asc", asc, "ascending"), ("desc", desc, "descending")] {
                let response = fiscal_request(
                    &app,
                    "GET",
                    &format!("/admin/catalog/products?sort={key}&dir={dir}"),
                    &cookie,
                    "",
                    false,
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let body = html(response).await;
                assert!(body.contains(&format!("aria-sort=\"{aria}\"")));
                let rows = body
                    .split("<tbody>")
                    .nth(1)
                    .unwrap()
                    .split("</tbody>")
                    .next()
                    .unwrap();
                let actual: Vec<i64> = rows
                    .split("class=\"ui-table__link\" href=\"/admin/catalog/products/")
                    .skip(1)
                    .map(|part| part.split('"').next().unwrap().parse().unwrap())
                    .collect();
                assert_eq!(actual, expected, "{key} {dir}");
                assert!(body.contains(&format!("name=\"sort\" value=\"{key}\"")));
                assert!(body.contains(&format!("name=\"dir\" value=\"{dir}\"")));
            }
        }
        sqlx::query("UPDATE catalog_variants SET active=false WHERE product_id=$1")
            .bind(product)
            .execute(&pool)
            .await
            .unwrap();
        for (key, dir, expected) in [
            ("price", "asc", vec![b, a, c, v]),
            ("price", "desc", vec![c, a, b, v]),
            ("stock", "asc", vec![c, b, a, v]),
            ("stock", "desc", vec![a, b, c, v]),
        ] {
            let body = html(
                fiscal_request(
                    &app,
                    "GET",
                    &format!("/admin/catalog/products?sort={key}&dir={dir}"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            let rows = body
                .split("<tbody>")
                .nth(1)
                .unwrap()
                .split("</tbody>")
                .next()
                .unwrap();
            let actual: Vec<i64> = rows
                .split("class=\"ui-table__link\" href=\"/admin/catalog/products/")
                .skip(1)
                .map(|part| part.split('"').next().unwrap().parse().unwrap())
                .collect();
            assert_eq!(actual, expected, "No active variants: {key} {dir}");
        }
        for suffix in [
            "sort=unknown",
            "sort=name%3BDELETE",
            "dir=invalid",
            "sort=actions",
        ] {
            assert_eq!(
                fiscal_request(
                    &app,
                    "GET",
                    &format!("/admin/catalog/products?{suffix}"),
                    &cookie,
                    "",
                    false
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_column_filters_combine_preserve_pagination_and_empty_controls() {
    isolated(|pool, _| async move {
        let (identity, session, products, variants, product, options, _) =
            variant_fixture(&pool).await;
        variants
            .configure(&session, &session.csrf, product, &options)
            .await
            .unwrap();
        let mut values = product_values("COLUMN-00", "column-00");
        let entry = products.entry(&session, product).await.unwrap();
        values.name = "Column <name> & product".into();
        values.category_id = entry.category_id.unwrap().to_string();
        values.brand_id = entry.brand_id.unwrap().to_string();
        values.tax_class_id = entry.tax_class_id.unwrap().to_string();
        values.status = "published".into();
        for i in 0..21 {
            values.model = format!("COLUMN-{i:02}");
            values.slug = format!("column-{i:02}");
            products
                .save(&session, &session.csrf, None, &values)
                .await
                .unwrap();
        }
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let filters = format!("q=Column&type=simple&model=column-&category={}&brand={}&status=published", entry.category_id.unwrap(), entry.brand_id.unwrap());
        let body = html(
            fiscal_request(
                &app,
                "GET",
                &format!("/admin/catalog/products?{filters}"),
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert_eq!(body.matches("class=\"ui-table__filters\"").count(), 1);
        assert_eq!(body.matches("scope=\"row\"").count(), 20);
        assert!(body.contains("Prodotti 1–20 di 21") && body.contains("Pagina 1 di 2"));
        for field in ["type", "q", "model", "category", "brand", "status"] {
            assert!(
                body.contains(&format!("name=\"{field}\"")),
                "Missing {field}"
            );
        }
        assert!(body.contains("Column &#60;name&#62; &#38; product"));
        assert!(body.contains(&format!(
            "type=simple&#38;model=column-&#38;category={}&#38;brand={}&#38;sort=name&#38;dir=asc&#38;per_page=20&#38;page=2", entry.category_id.unwrap(), entry.brand_id.unwrap()
        )));
        let second = html(
            fiscal_request(
                &app,
                "GET",
                &format!("/admin/catalog/products?{filters}&page=2"),
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert_eq!(second.matches("scope=\"row\"").count(), 1);
        let sorted = html(fiscal_request(&app,"GET",&format!("/admin/catalog/products?{filters}&sort=model&dir=desc"),&cookie,"",false).await).await;
        let sorted_rows = sorted.split("<tbody>").nth(1).unwrap().split("</tbody>").next().unwrap();
        assert!(sorted_rows.find(">COLUMN-20</td>").unwrap() < sorted_rows.find(">COLUMN-01</td>").unwrap());
        assert!(!sorted_rows.contains(">COLUMN-00</td>"));
        assert!(sorted.contains(&format!("brand={}&#38;sort=model&#38;dir=desc&#38;per_page=20&#38;page=2", entry.brand_id.unwrap())));
        let last = html(fiscal_request(&app,"GET",&format!("/admin/catalog/products?{filters}&sort=model&dir=desc&page=2"),&cookie,"",false).await).await;
        let last_rows = last.split("<tbody>").nth(1).unwrap().split("</tbody>").next().unwrap();
        assert_eq!(last_rows.matches("scope=\"row\"").count(),1);
        assert!(last_rows.contains(">COLUMN-00</td>"));

        for suffix in [
            "type=variants&model=column-",
            "model=%25",
            "model=%27%20OR%201%3D1",
            "q=%3Cscript%3E%26",
        ] {
            let empty = html(
                fiscal_request(
                    &app,
                    "GET",
                    &format!("/admin/catalog/products?{suffix}"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(empty.contains("Nessun risultato."));
            assert!(empty.contains("class=\"ui-table__filters\""));
            assert!(empty.contains("Azzera filtri"));
            assert_eq!(empty.matches("scope=\"row\"").count(), 0);
            assert!(!empty.contains("<script>&"));
        }
        let only_variants = html(
            fiscal_request(
                &app,
                "GET",
                "/admin/catalog/products?type=variants",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert_eq!(only_variants.matches("scope=\"row\"").count(), 1);
        for suffix in [
            "type=invalid",
            "status=invalid",
            "model=%0A",
            "brand=%00",
            "category=missing",
            "brand=missing",
            "category=0",
            "brand=-1",
            "category=9223372036854775807",
            "brand=9223372036854775807",
            "brand=9223372036854775808",
            "category=%09",
            "page=0",
        ] {
            assert_eq!(
                fiscal_request(
                    &app,
                    "GET",
                    &format!("/admin/catalog/products?{suffix}"),
                    &cookie,
                    "",
                    false
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        assert_eq!(
            fiscal_request(
                &app,
                "GET",
                "/admin/catalog/products?type=simple",
                "",
                "",
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
async fn product_list_table_shows_management_fields_and_variant_semantics() {
    isolated(|pool, _| async move {
        let (identity, session, products, variants, product, options, ids) =
            variant_fixture(&pool).await;
        variants
            .configure(&session, &session.csrf, product, &options)
            .await
            .unwrap();
        let mut values = product_values("SIMPLE", "private-slug");
        values.name = "A <table> & product".into();
        products
            .save(&session, &session.csrf, None, &values)
            .await
            .unwrap();
        let raw = authenticated(&identity).await;
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let body = html(
            fiscal_request(
                &app,
                "GET",
                "/admin/catalog/products",
                &format!("talocart_session={raw}"),
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(body.contains(
            "<table class=\"ui-table ui-table--fixed product-table\" data-product-selection>"
        ));
        assert!(body.contains("aria-label=\"Elenco prodotti\" tabindex=\"0\""));
        let head = body
            .split("<thead>")
            .nth(1)
            .unwrap()
            .split("</thead>")
            .next()
            .unwrap();
        assert_eq!(head.matches("scope=\"col\"").count(), 14);
        assert!(head.contains("id=\"product-select-all\""));
        assert!(head.contains("Seleziona tutti gli articoli visibili"));
        assert!(head.find(">ID</span>").unwrap() < head.find(">Immagine</span>").unwrap());
        assert!(!head.contains("Slug"));
        let rows = body
            .split("<tbody>")
            .nth(1)
            .unwrap()
            .split("</tbody>")
            .next()
            .unwrap();
        assert_eq!(rows.matches("scope=\"row\"").count(), 2);
        assert_eq!(
            rows.matches("class=\"product-row-select ui-check\"")
                .count(),
            2
        );
        assert!(rows.contains(&format!("type=\"checkbox\" value=\"{product}\"")));
        assert!(rows.contains(&format!("class=\"product-id-cell\">{product}</td>")));
        assert!(body.contains("/assets/product-selection.js"));
        assert!(!rows.contains("private-slug"));
        assert!(!rows.contains("<table> & product"));
        assert!(rows.contains("A &#60;table&#62; &#38; product"));
        assert!(rows.contains("Categoria prodotti"));
        assert!(rows.contains("Marchio prodotti"));
        assert!(!rows.contains("Classe prodotti"));
        assert!(!head.contains("Classe IVA"));
        assert!(head.find(">Immagine</span>").unwrap() < head.find(">Tipo</span>").unwrap());
        assert!(head.find(">Tipo</span>").unwrap() < head.find(">Nome</span>").unwrap());
        assert!(head.find(">Prezzo</span>").unwrap() < head.find(">Varianti</span>").unwrap());
        assert!(head.find(">Varianti</span>").unwrap() < head.find(">Scorte</span>").unwrap());
        assert!(rows.contains("product-image-placeholder"));
        assert!(rows.contains("12,34 €<small>IVA inclusa</small>"));
        assert!(rows.contains(">5</td>"));
        assert_eq!(rows.matches("Per variante</td>").count(), 2);
        assert!(rows.contains(">Varianti</td>"));
        assert!(rows.contains(">Semplice</td>"));
        assert!(rows.contains("Immagine non disponibile"));
        assert!(rows.contains("Pubblicato"));
        assert!(rows.contains("Bozza"));
        assert_eq!(rows.matches("class=\"variant-count\"").count(), 1);
        assert!(rows.contains(&format!(
            "href=\"/admin/catalog/products/{product}/variants\""
        )));
        assert!(rows.contains("varianti di Prodotto demo\">0</a>"));
        let first = variants
            .save(
                &session,
                &session.csrf,
                product,
                None,
                &variant_values("RED-S", &[ids[0], ids[2]], "24,90", "10"),
            )
            .await
            .unwrap();
        let mut inactive = variant_values("BLUE-M", &[ids[1], ids[3]], "26,90", "0");
        inactive.active = false;
        variants
            .save(&session, &session.csrf, product, None, &inactive)
            .await
            .unwrap();
        let updated = html(
            fiscal_request(
                &app,
                "GET",
                "/admin/catalog/products",
                &format!("talocart_session={raw}"),
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(
            updated.contains("varianti di Prodotto demo\">2</a>"),
            "Count includes inactive variants"
        );
        let detail = html(
            fiscal_request(
                &app,
                "GET",
                &format!("/admin/catalog/products/{product}/variants"),
                &format!("talocart_session={raw}"),
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(detail.contains("class=\"ui-table ui-table--fixed variant-table\""));
        for field in [
            "Immagine",
            "Combinazione",
            "SKU",
            "Prezzo",
            "Scorte",
            "Stato",
            "Azioni",
            "RED-S",
            "BLUE-M",
            "24,90",
            "26,90",
            "Disattivata",
        ] {
            assert!(detail.contains(field), "Missing {field}");
        }
        assert!(detail.contains(&format!(
            "/admin/catalog/products/{product}/variants/{first}"
        )));
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn simple_products_crud_price_contract_visibility_references_and_persistence() {
    use talocart_commerce::{
        products::{ProductError, Products},
        taxes::{Preferences, TaxError, Taxes},
    };
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let products = Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let (category, brand, class) = product_references(&pool, &session).await;
        let mut values = product_values(" Demo-1 ", "demo-1");
        let id = products
            .save(&session, &session.csrf, None, &values)
            .await
            .unwrap();
        assert_eq!(products.entry(&session, id).await.unwrap().model, "DEMO-1");
        assert!(products.public_list("", "", 1).await.unwrap().is_empty());
        assert!(matches!(
            products.public_entry("demo-1").await,
            Err(ProductError::NotFound)
        ));
        assert!(matches!(
            products
                .save(
                    &session,
                    &session.csrf,
                    None,
                    &product_values("demo-1", "new-slug")
                )
                .await,
            Err(ProductError::DuplicateModel)
        ));
        assert!(matches!(
            products
                .save(
                    &session,
                    &session.csrf,
                    None,
                    &product_values("NEW", "demo-1")
                )
                .await,
            Err(ProductError::DuplicateSlug)
        ));
        let taxes = Taxes::new(pool.clone(), identity.clone());
        assert!(matches!(
            taxes
                .save_preferences(
                    &session,
                    &session.csrf,
                    &Preferences {
                        currency: "EUR".into(),
                        country: String::new(),
                        mode: "exclusive".into(),
                        default_class_id: String::new()
                    },
                    taxes.revision().await.unwrap()
                )
                .await,
            Err(TaxError::ProductPrices)
        ));
        values.status = "published".into();
        assert!(matches!(
            products
                .save(&session, &session.csrf, Some(id), &values)
                .await,
            Err(ProductError::Invalid(_))
        ));
        values.category_id = category.to_string();
        values.brand_id = brand.to_string();
        values.tax_class_id = class.to_string();
        values.price = "0,05".into();
        values.stock = "0".into();
        products
            .save(&session, &session.csrf, Some(id), &values)
            .await
            .unwrap();
        let entry = products.public_entry("demo-1").await.unwrap();
        let quote = entry.quote().unwrap();
        assert_eq!(
            (
                quote.net().cents(),
                quote.tax().cents(),
                quote.gross().cents()
            ),
            (3, 2, 5)
        );
        assert_eq!(entry.stock, 0);
        assert_eq!(
            products
                .public_list("demo-1", "categoria-prodotti", 1)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            products
                .public_list("", "non-esistente", 1)
                .await
                .unwrap()
                .is_empty()
        );
        // Each publication check starts from the freshly saved stock version.
        values = products.entry(&session, id).await.unwrap().values();
        for (table, reference) in [
            ("catalog_categories", category),
            ("catalog_brands", brand),
            ("tax_classes", class),
        ] {
            let sql = format!("DELETE FROM {table} WHERE id=$1");
            assert!(
                sqlx::query(sqlx::AssertSqlSafe(sql))
                    .bind(reference)
                    .execute(&pool)
                    .await
                    .is_err()
            );
            let sql = format!("UPDATE {table} SET active=false WHERE id=$1");
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(reference)
                .execute(&pool)
                .await
                .unwrap();
            assert!(products.public_list("", "", 1).await.unwrap().is_empty());
            assert!(matches!(
                products.public_entry("demo-1").await,
                Err(ProductError::NotFound)
            ));
            assert!(matches!(
                products
                    .save(&session, &session.csrf, Some(id), &values)
                    .await,
                Err(ProductError::Publication)
            ));
            let sql = format!("UPDATE {table} SET active=true WHERE id=$1");
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(reference)
                .execute(&pool)
                .await
                .unwrap();
        }
        let restarted = Products::new(
            pool.clone(),
            Identity::new(pool.clone()),
            test_media(&pool).await,
        );
        assert_eq!(restarted.entry(&session, id).await.unwrap().price_cents, 5);
        products.delete(&session, &session.csrf, id).await.unwrap();
        assert!(matches!(
            products.entry(&session, id).await,
            Err(ProductError::NotFound)
        ));
        taxes
            .save_preferences(
                &session,
                &session.csrf,
                &Preferences {
                    currency: "EUR".into(),
                    country: String::new(),
                    mode: "exclusive".into(),
                    default_class_id: String::new(),
                },
                taxes.revision().await.unwrap(),
            )
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_first_save_rejects_a_price_mode_changed_since_form_opened() {
    use talocart_commerce::taxes::{Preferences, Taxes};
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let raw=authenticated(&identity).await;let session=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");let app=router(test_app(pool.clone(),"Shop".into()).await);
        let response=fiscal_request(&app,"GET","/admin/catalog/products/new",&cookie,"",false).await;assert_eq!(response.status(),StatusCode::OK);
        Taxes::new(pool.clone(),identity.clone()).save_preferences(&session,&session.csrf,&Preferences{currency:"EUR".into(),country:String::new(),mode:"exclusive".into(),default_class_id:String::new()}, Taxes::new(pool.clone(),identity.clone()).revision().await.unwrap()).await.unwrap();
        let body=format!("csrf={}&name=Demo&model=DEMO&slug=demo&price=12.34&manage_stock=true&stock_revision=0&stock=1&status=draft&price_mode=inclusive",session.csrf);
        let response=fiscal_request(&app,"POST","/admin/catalog/products/new",&cookie,&body,false).await;
        assert_eq!(response.status(),StatusCode::CONFLICT,"Never reinterpret a price entered in the old mode");
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_products").fetch_one(&pool).await.unwrap(),0);
        let body=html(response).await;assert!(body.contains("12.34"));assert!(body.contains("Modalità prezzi o valuta cambiate"));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_http_validation_csrf_public_prices_escaping_and_pagination() {
    use talocart_commerce::{
        products::Products,
        taxes::{Preferences, Taxes},
    };
    isolated(|pool,_|async move {
        let identity=installed(&pool).await; let raw=authenticated(&identity).await;
        let session=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let products=Products::new(pool.clone(),identity.clone(),test_media(&pool).await);
        let (category,brand,class)=product_references(&pool,&session).await;
        Taxes::new(pool.clone(),identity.clone()).save_preferences(&session,&session.csrf,&Preferences{currency:"EUR".into(),country:String::new(),mode:"exclusive".into(),default_class_id:class.to_string()}, Taxes::new(pool.clone(),identity.clone()).revision().await.unwrap()).await.unwrap();
        assert_eq!(fiscal_request(&app,"GET","/admin/catalog/products/new","","",false).await.status(),StatusCode::SEE_OTHER);
        for token in ["","wrong"] {assert_eq!(fiscal_request(&app,"POST","/admin/catalog/products/new",&cookie,&format!("csrf={token}&name=Demo&model=DEMO&slug=demo&price=1&manage_stock=true&stock_revision=0&stock=1&status=draft&price_mode=exclusive"),true).await.status(),StatusCode::FORBIDDEN);}
        let invalid=format!("csrf={}&name=Retained&model=DEMO&slug=demo&price=1.234&manage_stock=true&stock_revision=0&stock=-1&status=draft&price_mode=exclusive",session.csrf);
        let response=fiscal_request(&app,"POST","/admin/catalog/products/new",&cookie,&invalid,true).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);let body=html(response).await;assert!(body.contains("value=\"Retained\""));assert!(body.contains("aria-invalid=\"true\""));
        let body=format!("csrf={}&name=%3Cscript%3Ealert%281%29%3C%2Fscript%3E&description=%3Cb%3EPlain%3C%2Fb%3E&model=demo&slug=demo&price=0.03&manage_stock=true&stock_revision=0&stock=2&status=published&price_mode=exclusive&category_id={category}&brand_id={brand}&tax_class_id={class}",session.csrf);
        let response=fiscal_request(&app,"POST","/admin/catalog/products/new",&cookie,&body,false).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);
        let path=response.headers()[header::LOCATION].to_str().unwrap().split('?').next().unwrap().to_owned();
        let response=fiscal_request(&app,"GET","/products/demo","","",false).await;assert_eq!(response.status(),StatusCode::OK);assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");
        let body=html(response).await;assert!(body.contains("0,05"));assert!(body.contains("&#60;script&#62;"));assert!(body.contains("&lt;b&gt;Plain"));assert!(!body.contains("<script>alert"));
        let duplicate=format!("csrf={}&name=Duplicate&model=DEMO&slug=duplicate&price=1&manage_stock=true&stock_revision=0&stock=1&status=draft&price_mode=exclusive",session.csrf);
        assert_eq!(fiscal_request(&app,"POST","/admin/catalog/products/new",&cookie,&duplicate,false).await.status(),StatusCode::CONFLICT);
        for n in 0..22 {let mut values=product_values(&format!("MODEL-{n}"),&format!("item-{n}"));values.name=format!("Name {n:02}");values.price_mode="exclusive".into();products.save(&session,&session.csrf,None,&values).await.unwrap();}
        for (query,count) in [("",20),("?page=2",3),("?q=MODEL-21",1),("?q=%25",0),("?status=published",1)] {let body=html(fiscal_request(&app,"GET",&format!("/admin/catalog/products{query}"),&cookie,"",false).await).await;assert_eq!(body.matches("scope=\"row\"").count(),count,"{query}");}
        for query in ["?page=0","?status=bad","?q=%00"] {assert_eq!(fiscal_request(&app,"GET",&format!("/admin/catalog/products{query}"),&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);}
        assert_eq!(fiscal_request(&app,"GET","/products/item-0","","",false).await.status(),StatusCode::NOT_FOUND);
        assert_eq!(fiscal_request(&app,"GET","/catalog?page=0","","",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(fiscal_request(&app,"GET","/admin/catalog/products/999999",&cookie,"",false).await.status(),StatusCode::NOT_FOUND);
        let delete=format!("{path}/delete");assert_eq!(fiscal_request(&app,"GET",&delete,&cookie,"",false).await.status(),StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(fiscal_request(&app,"POST",&delete,&cookie,&format!("csrf={}",session.csrf),false).await.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(fiscal_request(&app,"POST",&delete,&cookie,"csrf=bad&confirm=true",false).await.status(),StatusCode::FORBIDDEN);
        assert_eq!(fiscal_request(&app,"POST",&delete,&cookie,&format!("csrf={}&confirm=true",session.csrf),false).await.status(),StatusCode::SEE_OTHER);
        assert_eq!(fiscal_request(&app,"GET","/products/demo","","",false).await.status(),StatusCode::NOT_FOUND);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_permissions_only_editor_readonly_landing_and_revocation() {
    use talocart_commerce::products::{ProductError, Products};
    use talocart_platform::identity::{Permission, Role};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let owner = taxonomy_session(&identity).await;
        let (category, _, class) = product_references(&pool, &owner).await;
        let role = identity
            .create_role(
                &owner,
                &owner.csrf,
                "Product editor",
                &[Permission::ProductsWrite],
            )
            .await
            .unwrap();
        identity
            .create_administrator(
                &owner,
                &owner.csrf,
                "products@example.test",
                "correct horse reader".into(),
                Role::Custom(role.clone()),
            )
            .await
            .unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let anon = fiscal_request(&app, "GET", "/admin/login", "", "", false).await;
        let anon_cookie = self::cookie(&anon);
        let token = csrf(&html(anon).await);
        let response = fiscal_request(
            &app,
            "POST",
            "/admin/login",
            &anon_cookie,
            &format!("csrf={token}&email=products%40example.test&password=correct+horse+reader"),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/admin/catalog/products"
        );
        let cookie = self::cookie(&response);
        let raw = cookie.split_once('=').unwrap().1;
        let writer = identity.session(raw).await.unwrap().unwrap();
        assert!(writer.allows(Permission::ProductsRead));
        assert!(!writer.allows(Permission::TaxesRead));
        assert!(!writer.allows(Permission::CategoriesRead));
        let response = fiscal_request(
            &app,
            "GET",
            "/admin/catalog/products/new",
            &cookie,
            "",
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = html(response).await;
        assert!(body.contains(&format!("value=\"{category}\"")));
        assert!(body.contains(&format!("value=\"{class}\"")));
        let products = Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let id = products
            .save(
                &writer,
                &writer.csrf,
                None,
                &product_values("WRITER", "writer"),
            )
            .await
            .unwrap();
        identity
            .update_role(
                &owner,
                &owner.csrf,
                &role,
                "Product reader",
                &[Permission::ProductsRead],
            )
            .await
            .unwrap();
        assert!(matches!(
            products
                .save(
                    &writer,
                    &writer.csrf,
                    Some(id),
                    &product_values("WRITER", "writer")
                )
                .await,
            Err(ProductError::Access(
                talocart_platform::technical::AccessError::Forbidden
            ))
        ));
        assert_eq!(
            fiscal_request(
                &app,
                "GET",
                "/admin/catalog/products/new",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        let (_, anon) = identity.anonymous().await.unwrap();
        let raw = identity
            .login(
                &anon,
                &anon.csrf,
                "products@example.test",
                "correct horse reader".into(),
            )
            .await
            .unwrap();
        let reader = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        assert!(!reader.allows(Permission::CategoriesRead));
        assert!(!reader.allows(Permission::BrandsRead));
        let list = html(
            fiscal_request(
                &app,
                "GET",
                &format!("/admin/catalog/products?category={category}"),
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(
            list.contains(
                "<select class=\"ui-control ui-control--compact\" id=\"product-category\""
            )
        );
        assert!(list.contains(&format!("value=\"{category}\" selected")));
        let body = html(
            fiscal_request(
                &app,
                "GET",
                &format!("/admin/catalog/products/{id}"),
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(body.contains("readonly"));
        for name in ["minimum_quantity", "packs"] {
            let input = body
                .split(&format!("name=\"{name}\""))
                .nth(1)
                .unwrap()
                .split('>')
                .next()
                .unwrap();
            assert!(
                input.contains("readonly"),
                "Quantity rules are read-only without products.write"
            );
        }
        assert!(!body.contains("Salva prodotto"));
        assert!(!body.contains("type=\"file\""));
        assert_eq!(
            fiscal_request(
                &app,
                "GET",
                "/admin/catalog/products/new",
                &cookie,
                "",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fiscal_request(
                &app,
                "POST",
                &format!("/admin/catalog/products/{id}/image/remove"),
                &cookie,
                &format!("csrf={}", reader.csrf),
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fiscal_request(&app, "GET", "/admin/settings/taxes", &cookie, "", false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_images_portrait_webp_http_cache_collisions_gc_and_concurrent_prices() {
    use talocart_commerce::{
        products::{ProductError, Products},
        taxes::{Preferences, Taxes},
    };
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let raw=authenticated(&identity).await;let session=identity.session(&raw).await.unwrap().unwrap();let cookie=format!("talocart_session={raw}");
        let media=test_media(&pool).await;let products=Products::new(pool.clone(),identity.clone(),media.clone());let taxes=Taxes::new(pool.clone(),identity.clone());
        let race_a=product_values("RACE","race-a");let race_b=product_values("RACE","race-b");
        let (a,b)=tokio::join!(products.save(&session,&session.csrf,None,&race_a),products.save(&session,&session.csrf,None,&race_b));
        assert_eq!(usize::from(a.is_ok())+usize::from(b.is_ok()),1);assert!(matches!(a,Err(ProductError::DuplicateModel)) || matches!(b,Err(ProductError::DuplicateModel)));
        let id=a.or(b).unwrap();products.delete(&session,&session.csrf,id).await.unwrap();
        let mode_values=product_values("MODE","mode");let preferences=Preferences{currency:"EUR".into(),country:String::new(),mode:"exclusive".into(),default_class_id:String::new()};
        let (save,mode)=tokio::join!(products.save(&session,&session.csrf,None,&mode_values),taxes.save_preferences(&session,&session.csrf,&preferences, taxes.revision().await.unwrap()));
        assert_eq!(usize::from(save.is_ok())+usize::from(mode.is_ok()),1,"First product and price mode switch cannot both succeed");
        if let Ok(id)=save {products.delete(&session,&session.csrf,id).await.unwrap();} else {assert!(matches!(save,Err(ProductError::ModeChanged)));}
        taxes.save_preferences(&session,&session.csrf,&Preferences{currency:"EUR".into(),country:String::new(),mode:"inclusive".into(),default_class_id:String::new()}, taxes.revision().await.unwrap()).await.unwrap();
        let id=products.save(&session,&session.csrf,None,&product_values("PHOTO","photo")).await.unwrap();
        let portrait=image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1500,3000,image::Rgba([1,2,3,120])));let mut png=std::io::Cursor::new(Vec::new());portrait.write_to(&mut png,image::ImageFormat::Png).unwrap();
        products.image(&session,&session.csrf,id,Some(png.into_inner()),"Photo Portrait.png").await.unwrap();
        let entry=products.entry(&session,id).await.unwrap();let old=entry.image_key.unwrap();assert_eq!(old,"Photo-Portrait.png");let decoded=image::load_from_memory(&media.read(&old).await.unwrap()).unwrap();assert_eq!((decoded.width(),decoded.height()),(1024,2048));assert!(decoded.has_alpha());
        while media.process_cache_job(&pool).await.unwrap() {}
        let app=router(test_app(pool.clone(),"Shop".into()).await);let image_path=products.entry(&session,id).await.unwrap().image_url("detail");let response=image_follow(&app,&image_path,"","GET","").await;assert_eq!(response.status(),StatusCode::OK);assert!(response.headers()[header::CACHE_CONTROL].to_str().unwrap().contains("must-revalidate"));
        let bytes=to_bytes(response.into_body(),10*1024*1024).await.unwrap();let decoded=image::load_from_memory(&bytes).unwrap();assert_eq!((decoded.width(),decoded.height()),(512,1024));
        let upload=format!("/admin/catalog/products/{id}/image");
        for (token,data,expected) in [("bad",include_bytes!("../fixtures/webp-lossless.webp").as_slice(),StatusCode::FORBIDDEN),(session.csrf.as_str(),include_bytes!("../fixtures/webp-animated.webp").as_slice(),StatusCode::UNPROCESSABLE_ENTITY),(session.csrf.as_str(),b"corrupt".as_slice(),StatusCode::UNPROCESSABLE_ENTITY),(session.csrf.as_str(),include_bytes!("../fixtures/webp-lossless.webp").as_slice(),StatusCode::SEE_OTHER)] {
            let mut body=format!("--photo\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{token}\r\n--photo\r\nContent-Disposition: form-data; name=\"image\"; filename=\"Photo Portrait.webp\"\r\nContent-Type: image/webp\r\n\r\n").into_bytes();body.extend_from_slice(data);body.extend_from_slice(b"\r\n--photo--\r\n");
            let response=app.clone().oneshot(Request::builder().method("POST").uri(&upload).header(header::COOKIE,&cookie).header(header::CONTENT_TYPE,"multipart/form-data; boundary=photo").body(Body::from(body)).unwrap()).await.unwrap();assert_eq!(response.status(),expected);
            if expected!=StatusCode::SEE_OTHER {assert_eq!(products.entry(&session,id).await.unwrap().image_key.as_deref(),Some(old.as_str()));}
        }
        let new=products.entry(&session,id).await.unwrap().image_key.unwrap();assert_ne!(new,old);assert!(new.starts_with("Photo-Portrait-"));assert_eq!(media.read(&new).await.unwrap()[..8],*b"\x89PNG\r\n\x1a\n");
        assert_eq!(completed_prune(&media, &pool, false).await.objects,0);
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '48 hours'").execute(&pool).await.unwrap();assert_eq!(completed_prune(&media,&pool,false).await.objects,0);purge_library_sources(&pool,&session,std::slice::from_ref(&old)).await;assert!(completed_prune(&media, &pool, false).await.objects>=1);assert!(media.read(&new).await.is_ok(),"Live product image survives cleanup");
        products.delete(&session,&session.csrf,id).await.unwrap();assert_eq!(completed_prune(&media, &pool, false).await.objects,0);
        sqlx::query("UPDATE media_objects SET unused_since=now()-interval '48 hours'").execute(&pool).await.unwrap();assert_eq!(completed_prune(&media,&pool,false).await.objects,0);assert!(media.read(&new).await.is_ok());purge_library_sources(&pool,&session,std::slice::from_ref(&new)).await;assert!(completed_prune(&media, &pool, false).await.objects>=1);assert!(media.read(&new).await.is_err());
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_editor_preserves_selected_brand_beyond_bounded_choices() {
    isolated(|pool,_|async move {
        let identity=installed(&pool).await;let raw=authenticated(&identity).await;let session=identity.session(&raw).await.unwrap().unwrap();
        sqlx::query("INSERT INTO catalog_brands(name,slug) SELECT 'Brand '||lpad(n::text,4,'0'),'brand-'||n FROM generate_series(1,1001) n").execute(&pool).await.unwrap();
        let brand:i64=sqlx::query_scalar("SELECT id FROM catalog_brands WHERE slug='brand-1001'").fetch_one(&pool).await.unwrap();
        let products=talocart_commerce::products::Products::new(pool.clone(),identity.clone(),test_media(&pool).await);
        let mut values=product_values("LAST-BRAND","last-brand");values.brand_id=brand.to_string();let id=products.save(&session,&session.csrf,None,&values).await.unwrap();
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        let body=html(fiscal_request(&app,"GET",&format!("/admin/catalog/products/{id}"),&format!("talocart_session={raw}"),"",false).await).await;
        let brands=body.split("name=\"brand_id\"").nth(1).unwrap().split("</select>").next().unwrap();
        assert!(brands.contains(&format!("value=\"{brand}\" selected")),"Saving an unchanged editor must preserve the existing brand");
        assert!(brands.matches("<option").count()<=1002,"Selector stays bounded including the empty option");
        let list=html(fiscal_request(&app,"GET",&format!("/admin/catalog/products?brand={brand}"),&format!("talocart_session={raw}"),"",false).await).await;
        let choices=list.split("<select class=\"ui-control ui-control--compact\" id=\"product-brand\"").nth(1).unwrap().split("</select>").next().unwrap();
        assert!(choices.contains(&format!("value=\"{brand}\" selected")),"The list retains a selected brand outside the first thousand choices");
        assert!(choices.matches("<option").count()<=1002);
        assert!(list.contains(">LAST-BRAND</td>"));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_tabs_preserve_validation_context_and_action_redirects() {
    use talocart_commerce::products::Products;
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let products = Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let id = products.save(&session, &session.csrf, None, &product_values("TABS", "tabs")).await.unwrap();
        let path = format!("/admin/catalog/products/{id}");
        let payload = |name: &str, mode: &str| format!("csrf={}&editor_tab=related&name={name}&model=TABS&slug=tabs&price=12.34&manage_stock=true&stock_revision=0&stock=5&status=draft&price_mode=inclusive&currency=EUR&related_fallback={mode}", session.csrf);
        let response = fiscal_request(&app, "POST", &path, &cookie, &payload("Updated", "brand"), false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(products.related_editor(&session, id, "").await.unwrap().fallback, "brand");
        let response = fiscal_request(&app, "POST", &path, &cookie, &payload("", "category"), false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = html(response).await;
        assert!(body.contains("value=\"category\" selected"));
        assert!(body.contains("name=\"related_fallback\" form=\"product-editor-form\""));
        assert_eq!(products.related_editor(&session, id, "").await.unwrap().fallback, "brand");
        assert_eq!(products.entry(&session, id).await.unwrap().name, "Updated");
        let response = fiscal_request(&app, "POST", &path, &cookie, &payload("Invalid", "invalid"), false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(products.entry(&session, id).await.unwrap().name, "Updated");
        for key in ["general", "commerce", "organization", "attributes", "image", "variants", "related", "delete"] {
            let response = fiscal_request(&app, "GET", &format!("{path}?tab={key}"), &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = html(response).await;
            assert!(body.contains(&format!("data-active-tab=\"{key}\"")));
            assert!(body.contains("/assets/product-tabs.js"));
            assert_eq!(body.matches("class=\"ui-tabs__tab\" id=\"tab-").count(), 8);
            assert!(body.contains(&format!("id=\"tab-{key}\" href=\"#panel-{key}\" aria-current=\"page\"")), "The shared tab menu marks the server-selected section");
            assert!(!body.contains("data-ui-tabs-initial"), "Without an error the address may choose the section");
            assert_eq!(body.matches("data-product-panel=").count(), 8);
            for tag in body.split('<').filter(|tag| tag.split('>').next().is_some_and(|opening| opening.contains("data-product-panel="))) {
                let opening = tag.split('>').next().unwrap();
                assert!(!opening.contains(" hidden"), "No-JS fallback keeps every product panel available");
            }
        }
        let body = html(fiscal_request(&app, "GET", &format!("{path}?tab=unknown"), &cookie, "", false).await).await;
        assert!(body.contains("data-active-tab=\"general\""));
        let body = html(fiscal_request(&app, "GET", "/admin/catalog/products/new", &cookie, "", false).await).await;
        assert_eq!(body.matches("class=\"ui-tabs__tab\" id=\"tab-").count(), 3);
        for (price, state, expected) in [("1.234", "draft", "commerce"), ("1", "invalid", "organization")] {
            let body = format!("csrf={}&editor_tab=general&name=Retained&model=TABS&slug=tabs&price={price}&manage_stock=true&stock_revision=0&stock=5&status={state}&price_mode=inclusive", session.csrf);
            let response = fiscal_request(&app, "POST", &path, &cookie, &body, false).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body = html(response).await;
            assert!(body.contains(&format!("data-active-tab=\"{expected}\"")));
            assert!(body.contains(&format!("data-ui-tabs-initial=\"panel-{expected}\"")), "A server error reopens its section before the address hash");
            assert!(body.contains("value=\"Retained\""));
        }
        let body = format!("csrf={}&editor_tab=organization&name=Updated&model=TABS&slug=tabs&price=12.34&manage_stock=true&stock_revision=0&stock=5&status=draft&price_mode=inclusive", session.csrf);
        let response = fiscal_request(&app, "POST", &path, &cookie, &body, false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], format!("{path}?saved=1&tab=organization"));
        assert_eq!(fiscal_request(&app, "POST", &path, &cookie, &body.replace("&manage_stock=true&stock_revision=0", ""), false).await.status(), StatusCode::CONFLICT, "Old forms without a stock version must reload");
        assert_eq!(products.entry(&session,id).await.unwrap().name,"Updated");
        for tab in ["attributes", "image", "variants", "delete"] {
            let body = format!("csrf={}&editor_tab={tab}&name=Updated&model=TABS&slug=tabs&price=12.34&manage_stock=true&stock_revision=0&stock=5&status=draft&price_mode=inclusive", session.csrf);
            let response = fiscal_request(&app, "POST", &path, &cookie, &body, false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(response.headers()[header::LOCATION], format!("{path}?saved=1&tab={tab}"));
        }
        let response=fiscal_request(&app,"POST",&format!("{path}/image/remove"),&cookie,&format!("csrf={}",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION],format!("{path}?saved=1&tab=image"));
        let response=fiscal_request(&app,"POST",&format!("{path}/options"),&cookie,&format!("csrf={}&option_1=&option_2=&option_3=",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION],format!("{path}?saved=1&tab=variants"));
        let response=fiscal_request(&app,"POST",&format!("{path}/delete"),&cookie,&format!("csrf={}",session.csrf),false).await;
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        assert!(html(response).await.contains("data-active-tab=\"delete\""));
        assert_eq!(fiscal_request(&app,"POST",&path,&cookie,"csrf=wrong&name=X&model=TABS&slug=tabs&price=1&manage_stock=true&stock_revision=0&stock=5&status=draft&price_mode=inclusive&editor_tab=image",false).await.status(),StatusCode::FORBIDDEN);
        assert_eq!(products.entry(&session,id).await.unwrap().name,"Updated");
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn configured_product_editor_preserves_parent_commercial_values() {
    isolated(|pool, _| async move {
        let (identity, session, products, variants, product, options, ids) = variant_fixture(&pool).await;
        sqlx::query("UPDATE administrators SET locale='it'").execute(&pool).await.unwrap();
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let http_session = identity.session(&raw).await.unwrap().unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let path = format!("/admin/catalog/products/{product}");
        let original = products.entry(&session, product).await.unwrap();
        variants.configure(&session, &session.csrf, product, &options).await.unwrap();
        let body = html(fiscal_request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(!body.contains("id=\"product-price\""), "Configured parents have no editable price");
        assert!(!body.contains("id=\"product-stock\""), "Configured parents have no editable stock");
        assert!(body.contains("data-variant-commerce-summary"));
        assert!(body.contains("Nessuna variante attiva"), "Summary: {}", body.split("data-variant-commerce-summary").nth(1).unwrap().split("</div>\n").next().unwrap());
        // A form opened while configured must require explicit values if it becomes simple.
        variants.configure(&session, &session.csrf, product, &[]).await.unwrap();
        let converted = products.entry(&session, product).await.unwrap();
        assert_eq!(converted.status, "draft");
        assert!(!products.public_visible("model").await.unwrap(), "Conserved parent values are not published by conversion");
        let mut reviewed = converted.values();
        reviewed.price = "25".into(); reviewed.stock = "8".into(); reviewed.status = "published".into();
        products.save(&session, &session.csrf, Some(product), &reviewed).await.unwrap();
        assert!(products.public_visible("model").await.unwrap());
        variants.configure(&session, &session.csrf, product, &[]).await.unwrap();
        assert_eq!(products.entry(&session, product).await.unwrap().status, "published", "Saving an already simple configuration preserves publication");
        let mut restore = original.values();
        restore.stock_revision = products.entry(&session, product).await.unwrap().stock_revision;
        products.save(&session, &session.csrf, Some(product), &restore).await.unwrap();
        let original = products.entry(&session, product).await.unwrap();
        let missing = format!("csrf={}&name=Updated&model=MODEL&slug=model&manage_stock=true&stock_revision={}&status=draft&price_mode=inclusive", http_session.csrf, original.stock_revision);
        assert_eq!(fiscal_request(&app, "POST", &path, &cookie, &missing, false).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let retained = products.entry(&session, product).await.unwrap();
        assert_eq!((retained.price_cents, retained.stock), (original.price_cents, original.stock));
        variants.configure(&session, &session.csrf, product, &options).await.unwrap();
        let first = variants.save(&session, &session.csrf, product, None, &variant_values("SUMMARY-ONE", &[ids[0], ids[2]], "20", "7")).await.unwrap();
        let mut off = variant_values("SUMMARY-OFF", &[ids[0], ids[3]], "1", "999");
        off.active = false;
        variants.save(&session, &session.csrf, product, None, &off).await.unwrap();
        let body = html(fiscal_request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(body.contains("data-variant-price>20,00 €</dd>"));
        assert!(body.contains("data-variant-stock>7</dd>"));
        variants.save(&session, &session.csrf, product, None, &variant_values("SUMMARY-TWO", &[ids[1], ids[2]], "30", "0")).await.unwrap();
        let body = html(fiscal_request(&app, "GET", &path, &cookie, "", false).await).await;
        assert!(body.contains("data-variant-price>20,00 – 30,00 €</dd>"));
        assert!(body.contains("data-variant-stock>7</dd>"));
        for extra in ["", "&price=0&stock=0", "&price=invalid&stock=-1"] {
            let body = format!("csrf={}&editor_tab=commerce&name=Updated&model=MODEL&slug=model&manage_stock=true&stock_revision={}&minimum_quantity=2&packs=3&status=draft&price_mode=inclusive{extra}", http_session.csrf, original.stock_revision);
            assert_eq!(fiscal_request(&app, "POST", &path, &cookie, &body, false).await.status(), StatusCode::SEE_OTHER);
            let saved = products.entry(&session, product).await.unwrap();
            assert_eq!((saved.price_cents, saved.stock, saved.stock_revision), (original.price_cents, original.stock, original.stock_revision));
            assert_eq!((saved.minimum_quantity, saved.packs), (2, 3));
            let variant = variants.entry(&session, product, first).await.unwrap();
            assert_eq!((variant.price_cents, variant.stock), (2000, 7));
        }
        // A stale simple-product form cannot change the newly configured parent's price or stock.
        let mut stale = original.values();
        stale.price = "99".into(); stale.stock = "999".into();
        products.save(&session, &session.csrf, Some(product), &stale).await.unwrap();
        let saved = products.entry(&session, product).await.unwrap();
        assert_eq!((saved.price_cents, saved.stock), (original.price_cents, original.stock));
        let invalid = format!("csrf={}&name=&model=MODEL&slug=model&manage_stock=true&stock_revision={}&status=draft&price_mode=inclusive", http_session.csrf, original.stock_revision);
        let response = fiscal_request(&app, "POST", &path, &cookie, &invalid, false).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(html(response).await.contains("data-variant-commerce-summary"));
        let simple = products.save(&session, &session.csrf, None, &product_values("SIMPLE", "simple")).await.unwrap();
        let body = html(fiscal_request(&app, "GET", &format!("/admin/catalog/products/{simple}"), &cookie, "", false).await).await;
        assert!(body.contains("id=\"product-price\"")); assert!(body.contains("id=\"product-stock\""));
        let missing = format!("csrf={}&name=Simple&model=SIMPLE&slug=simple&manage_stock=true&stock_revision=0&status=draft&price_mode=inclusive", http_session.csrf);
        assert_eq!(fiscal_request(&app, "POST", &format!("/admin/catalog/products/{simple}"), &cookie, &missing, false).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn queued_product_write_rechecks_revocation_after_identity_lock() {
    use std::time::Duration;
    use talocart_commerce::products::{ProductError, Products};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let products = Products::new(pool.clone(), identity, test_media(&pool).await);
        let original = product_values("QUEUED", "queued");
        let id = products.save(&session, &session.csrf, None, &original).await.unwrap();
        let mut revocation = pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *revocation).await.unwrap();
        sqlx::query("SELECT id FROM identity_state WHERE id=1 FOR UPDATE").fetch_one(&mut *revocation).await.unwrap();
        let service = products.clone();
        let old_session = session.clone();
        let mut changed = original.clone(); changed.name = "Must not be saved".into();
        let write = tokio::spawn(async move { service.save(&old_session, &old_session.csrf, Some(id), &changed).await });
        // Observe a real blocked backend before revoking; scheduling delays alone prove nothing.
        let blocked = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND wait_event_type='Lock')")
                    .bind(blocker).fetch_one(&pool).await.unwrap();
                if waiting { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await;
        if blocked.is_err() { write.abort(); revocation.rollback().await.unwrap(); panic!("Product write did not wait on identity lock"); }
        // Reads remain possible while a different transaction holds the authorization lock.
        assert_eq!(products.entry(&session, id).await.unwrap().name, original.name);
        sqlx::query("DELETE FROM admin_sessions WHERE token_hash=$1").bind(&session.token_hash).execute(&mut *revocation).await.unwrap();
        revocation.commit().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), write).await.unwrap().unwrap();
        assert!(matches!(result, Err(ProductError::Access(talocart_platform::technical::AccessError::Forbidden))));
        let name: String = sqlx::query_scalar("SELECT name FROM catalog_products WHERE id=$1").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(name, original.name);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn public_brand_filter_preserves_selection_beyond_first_thousand() {
    isolated(|pool, _| async move {
        installed(&pool).await;
        sqlx::query("INSERT INTO catalog_brands(name,slug) SELECT 'Brand '||lpad(n::text,4,'0'),'brand-'||n FROM generate_series(1,1001) n")
            .execute(&pool).await.unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let page = html(fiscal_request(&app, "GET", "/catalog?brand=brand-1001", "", "", false).await).await;
        assert!(page.contains("value=\"brand-1001\" selected"), "The active brand must remain selected even outside the bounded choices");
        assert_eq!(page.matches("<option value=\"brand-").count(), 1001);
        let page = html(fiscal_request(&app, "GET", "/catalog?brand=brand-1", "", "", false).await).await;
        assert_eq!(page.matches("<option value=\"brand-").count(), 1000, "An existing choice must not be duplicated");
        sqlx::query("UPDATE catalog_brands SET active=false WHERE slug='brand-1001'").execute(&pool).await.unwrap();
        let page = html(fiscal_request(&app, "GET", "/catalog?brand=brand-1001", "", "", false).await).await;
        assert!(!page.contains("<option value=\"brand-1001\""), "Inactive brands stay excluded");
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_purchase_quantities_defaults_validation_persistence_and_availability() {
    use talocart_commerce::products::{Field, ProductError, Products, QuantityError};
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let products = Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let new = html(fiscal_request(&app, "GET", "/admin/catalog/products/new", &cookie, "", false).await).await;
        for field in ["minimum_quantity", "packs"] {
            assert!(new.contains(&format!("name=\"{field}\" value=\"1\"")));
        }
        let (category, brand, class) = product_references(&pool, &session).await;
        let body = format!("csrf={}&editor_tab=commerce&name=Pack+product&model=PACK&slug=pack&price=12.34&price_mode=inclusive&manage_stock=true&stock_revision=0&stock=11&status=published&category_id={category}&brand_id={brand}&tax_class_id={class}&minimum_quantity=10&packs=6", session.csrf);
        let response = fiscal_request(&app, "POST", "/admin/catalog/products/new", &cookie, &body, false).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let id = products.list(&session, "PACK", "", 1).await.unwrap()[0].id;
        let path = format!("/admin/catalog/products/{id}");
        let entry = products.entry(&session, id).await.unwrap();
        assert_eq!((entry.minimum_quantity, entry.packs), (10, 6));
        assert_eq!(entry.values().packs, "6");
        let rules = entry.purchase_quantities().unwrap();
        assert_eq!(rules.validate(10), Err(QuantityError::NotPackMultiple));
        assert!(rules.validate(12).is_ok());
        assert!(rules.validate(24).is_ok());
        assert!(!entry.available());
        let default: (i32, i32) = sqlx::query_as("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('SQL defaults','SQL-DEFAULTS','sql-defaults',0) RETURNING minimum_quantity,packs").fetch_one(&pool).await.unwrap();
        assert_eq!(default, (1, 1));
        for (column, field) in [("minimum_quantity", Field::MinimumQuantity), ("packs", Field::Packs)] {
            for bad in [0, -1, 1000000] {
                let sql = format!("UPDATE catalog_products SET {column}=$1 WHERE id=$2");
                let error = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(bad).bind(id).execute(&pool).await.unwrap_err();
                assert!(matches!(ProductError::from(error), ProductError::Invalid(fields) if fields == [field]));
            }
            let sql = format!("UPDATE catalog_products SET {column}=NULL WHERE id=$1");
            assert!(sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id).execute(&pool).await.is_err());
        }
        for htmx in [false, true] {
            for (invalid, error_id) in [("minimum_quantity=0&packs=6", "minimum-quantity-error"), ("minimum_quantity=10&packs=1.5", "packs-error"), ("minimum_quantity=&packs=", "packs-error"), ("minimum_quantity=2147483648&packs=1000000", "packs-error")] {
                let changed = body.replace("minimum_quantity=10&packs=6", invalid).replace("name=Pack+product", "name=Retained");
                let response = fiscal_request(&app, "POST", &path, &cookie, &changed, htmx).await;
                assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
                let html = html(response).await;
                assert!(html.contains("data-active-tab=\"commerce\""));
                assert!(html.contains(&format!("id=\"{error_id}\"")));
                assert!(html.contains("aria-invalid=\"true\""));
                assert!(html.contains("value=\"Retained\""));
                assert_eq!(products.entry(&session, id).await.unwrap().name, "Pack product");
            }
        }
        assert_eq!(fiscal_request(&app, "POST", &path, &cookie, &body.replace(&session.csrf, "wrong"), false).await.status(), StatusCode::FORBIDDEN);
        for (stock, available) in [(11, false), (12, true), (17, true), (18, true)] {
            let revision = products.entry(&session, id).await.unwrap().stock_revision;
            let fresh = body.replace("stock_revision=0", &format!("stock_revision={revision}"));
            let response = fiscal_request(&app, "POST", &path, &cookie, &fresh.replace("stock=11", &format!("stock={stock}")), false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let public = products.public_entry("pack").await.unwrap();
            assert_eq!((public.minimum_quantity, public.packs, public.price_cents), (10, 6, 1234));
            assert_eq!(public.available(), available);
            assert_eq!(products.public_filtered("PACK", "", "", true, 1).await.unwrap().len(), usize::from(available));
            let detail = html(fiscal_request(&app, "GET", "/products/pack", "", "", false).await).await;
            assert!(detail.contains("Quantità minima acquistabile: 10 pezzi. Packs: 6 pezzi per confezione."));
            assert!(detail.contains("Primo acquisto: 12 pezzi."));
            assert!(detail.contains(if available { ">Disponibile</p>" } else { ">Non disponibile</p>" }));
        }
        // A new service instance reads both fields from PostgreSQL, not in-memory defaults.
        let reopened = Products::new(pool.clone(), Identity::new(pool.clone()), test_media(&pool).await);
        let entry = reopened.entry(&session, id).await.unwrap();
        assert_eq!((entry.minimum_quantity, entry.packs, entry.stock), (10, 6, 18));
        let changed = body.replace("minimum_quantity=10&packs=6", "minimum_quantity=5&packs=1")
            .replace("stock_revision=0", &format!("stock_revision={}", entry.stock_revision))
            .replace("stock=11", &format!("stock={}", entry.stock));
        assert_eq!(fiscal_request(&app, "POST", &path, &cookie, &changed, false).await.status(), StatusCode::SEE_OTHER);
        assert_eq!(products.public_entry("pack").await.unwrap().purchase_quantities().unwrap().first_quantity(), 5);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_purchase_quantities_apply_to_selected_variants_without_summing_stock() {
    isolated(|pool, _| async move {
        let (identity, session, products, variants, product, options, ids) =
            variant_fixture(&pool).await;
        let mut values = products.entry(&session, product).await.unwrap().values();
        values.minimum_quantity = "10".into();
        values.packs = "6".into();
        values.stock = "999".into();
        products
            .save(&session, &session.csrf, Some(product), &values)
            .await
            .unwrap();
        variants
            .configure(&session, &session.csrf, product, &options)
            .await
            .unwrap();
        let first = variants
            .save(
                &session,
                &session.csrf,
                product,
                None,
                &variant_values("PACK-RED", &[ids[0], ids[2]], "20", "11"),
            )
            .await
            .unwrap();
        variants
            .save(
                &session,
                &session.csrf,
                product,
                None,
                &variant_values("PACK-BLUE", &[ids[1], ids[2]], "30", "11"),
            )
            .await
            .unwrap();
        let mut inactive = variant_values("PACK-OFF", &[ids[0], ids[3]], "1", "999");
        inactive.active = false;
        variants
            .save(&session, &session.csrf, product, None, &inactive)
            .await
            .unwrap();
        assert!(!products.public_entry("model").await.unwrap().available());
        assert!(
            products
                .public_filtered("MODEL", "", "", true, 1)
                .await
                .unwrap()
                .is_empty(),
            "Two partial stocks and inactive stock cannot form a purchasable pack"
        );
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let red = format!("/products/model?value_1={}&value_2={}", ids[0], ids[2]);
        let before = html(fiscal_request(&app, "GET", &red, "", "", false).await).await;
        assert!(before.contains(">Non disponibile</p>"));
        variants
            .save(
                &session,
                &session.csrf,
                product,
                Some(first),
                &variant_values("PACK-RED", &[ids[0], ids[2]], "20", "12"),
            )
            .await
            .unwrap();
        assert_eq!(
            products
                .public_filtered("MODEL", "", "", true, 1)
                .await
                .unwrap()
                .len(),
            1
        );
        for htmx in [false, true] {
            let red = html(fiscal_request(&app, "GET", &red, "", "", htmx).await).await;
            assert!(red.contains(">Disponibile</p>"));
            assert!(red.contains("Primo acquisto: 12 pezzi."));
            assert!(red.contains("Packs: 6 pezzi per confezione."));
            assert!(red.contains("20,00 €"), "Variant price remains unitary");
            let blue = html(
                fiscal_request(
                    &app,
                    "GET",
                    &format!("/products/model?value_1={}&value_2={}", ids[1], ids[2]),
                    "",
                    "",
                    htmx,
                )
                .await,
            )
            .await;
            assert!(blue.contains(">Non disponibile</p>"));
        }
        drop(identity);
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_columns_persist_per_account_and_enforce_http_contract() {
    use talocart_platform::identity::{PRODUCT_COLUMNS, Permission, ProductColumns, Role};
    isolated(|pool, _| async move {
        let (identity, owner, _, _, product, _, _) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        assert_eq!(identity.product_columns(&session).await.unwrap().count(), 14);
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let body = html(fiscal_request(&app,"GET","/admin/catalog/products",&cookie,"",false).await).await;
            assert!(body.contains(if locale == "it" {">Colonne<"} else {">Columns<"}));
            assert_eq!(body.matches("name=\"columns\"").count(),13);
            assert!(!body.contains("data-product-column=\"model\" hidden"));
        }
        for invalid in ["&columns=unknown", "&columns=name&administrator_id=2", "&columns=name&csrf=duplicate"] {
            let response=fiscal_request(&app,"POST","/admin/catalog/products/columns",&cookie,&format!("csrf={}{}",session.csrf,invalid),false).await;
            assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
        }
        assert_eq!(fiscal_request(&app,"POST","/admin/catalog/products/columns",&cookie,"csrf=bad&columns=name",false).await.status(),StatusCode::FORBIDDEN);
        let body=format!("csrf={}&columns=name&columns=image&return_to=%2Fadmin%2Fcatalog%2Fproducts%3Fq%3DModel%26page%3D2",session.csrf);
        let saved=fiscal_request(&app,"POST","/admin/catalog/products/columns",&cookie,&body,false).await;
        assert_eq!(saved.status(),StatusCode::SEE_OTHER);
        assert_eq!(saved.headers()[header::LOCATION],"/admin/catalog/products?q=Model&page=2");
        let body=html(fiscal_request(&app,"GET","/admin/catalog/products",&cookie,"",false).await).await;
        assert!(body.contains("data-product-column=\"model\" hidden"));
        assert!(!body.contains("data-product-column=\"name\" hidden"));
        assert!(body.contains(&format!("/admin/catalog/products/{product}")));
        let empty=html(fiscal_request(&app,"GET","/admin/catalog/products?q=not-found",&cookie,"",false).await).await;
        assert!(empty.contains("colspan=\"3\" data-product-empty"));
        let fresh=identity.session(&authenticated(&identity).await).await.unwrap().unwrap();
        assert_eq!(identity.product_columns(&fresh).await.unwrap().count(),3,"new session reads persisted account preference");
        let role=identity.create_role(&owner,&owner.csrf,"Product viewer",&[Permission::ProductsRead]).await.unwrap();
        identity.create_administrator(&owner,&owner.csrf,"viewer@example.test","correct horse reader".into(),Role::Custom(role.clone())).await.unwrap();
        let (_,anon)=identity.anonymous().await.unwrap();
        let reader_raw=identity.login(&anon,&anon.csrf,"viewer@example.test","correct horse reader".into()).await.unwrap();
        let reader=identity.session(&reader_raw).await.unwrap().unwrap();
        assert_eq!(identity.product_columns(&reader).await.unwrap().count(),14,"other administrator keeps defaults");
        identity.save_product_columns(&reader,&reader.csrf,&ProductColumns::from_visible(&["model".into()]).unwrap()).await.unwrap();
        assert_eq!(identity.product_columns(&fresh).await.unwrap().count(),3,"other account cannot change owner preference");
        let redirect=fiscal_request(&app,"POST","/admin/catalog/products/columns",&cookie,&format!("csrf={}&columns=name&return_to=https%3A%2F%2Fevil.example",session.csrf),false).await;
        assert_eq!(redirect.headers()[header::LOCATION],"/admin/catalog/products");
        identity.update_role(&owner,&owner.csrf,&role,"No products",&[Permission::AdminRead]).await.unwrap();
        assert!(matches!(identity.save_product_columns(&reader,&reader.csrf,&ProductColumns::from_visible(&PRODUCT_COLUMNS.map(str::to_owned)).unwrap()).await,Err(talocart_platform::identity::IdentityError::Forbidden)));
        sqlx::query("UPDATE admin_sessions SET expires_at=now()-interval '1 second' WHERE token_hash=$1").bind(&fresh.token_hash).execute(&pool).await.unwrap();
        assert!(identity.save_product_columns(&fresh,&fresh.csrf,&ProductColumns::from_visible(&["name".into()]).unwrap()).await.is_err());
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_columns_initial_schema_defaults_and_validation() {
    isolated(|pool, _| async move {
        let (identity, session, _, _, product, _, _) = variant_fixture(&pool).await;
        let before: Vec<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        talocart_platform::database::MIGRATOR
            .run(&pool)
            .await
            .unwrap();
        assert_eq!(
            identity.product_columns(&session).await.unwrap().count(),
            14
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT id FROM catalog_products WHERE id=$1")
                .bind(product)
                .fetch_one(&pool)
                .await
                .unwrap(),
            product
        );
        let after: Vec<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(before, after);
        assert!(
            sqlx::query("UPDATE administrators SET product_hidden_columns=ARRAY['unknown']")
                .execute(&pool)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_modified_column_uses_real_history_regional_format_and_preserves_preferences() {
    isolated(|pool, _| async move {
        let (identity, session, _, _, product, _, _) = variant_fixture(&pool).await;
        sqlx::query("UPDATE administrators SET product_hidden_columns=ARRAY['model','brand']").execute(&pool).await.unwrap();
        let before:Vec<(i64,Vec<u8>)>=sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version").fetch_all(&pool).await.unwrap();
        talocart_platform::database::MIGRATOR.run(&pool).await.unwrap();
        let columns=identity.product_columns(&session).await.unwrap();
        assert!(columns.visible("modified_at"));assert!(!columns.visible("model"));assert!(!columns.visible("brand"));assert_eq!(columns.count(),12);
        let after:Vec<(i64,Vec<u8>)>=sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version").fetch_all(&pool).await.unwrap();assert_eq!(before,after);
        sqlx::query("UPDATE platform_regional_settings SET time_zone='America/New_York',date_format='ymd'").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO operation_events(created_at,area,resource,resource_key,action) VALUES('2099-01-15 12:30:00+00','catalog','catalog_products',$1,'UPDATE')").bind(product.to_string()).execute(&pool).await.unwrap();
        // A later translation is also a product modification; unrelated resources are excluded.
        sqlx::query("INSERT INTO operation_events(created_at,area,resource,resource_key,action) VALUES('2099-01-16 12:45:00+00','catalog','catalog_products_translations',$1,'UPDATE'),('2099-01-17 12:00:00+00','catalog','catalog_brands',$1,'UPDATE')").bind(product.to_string()).execute(&pool).await.unwrap();
        // A synthetic row without an audit event must not invent a timestamp.
        sqlx::query("ALTER TABLE catalog_products DISABLE TRIGGER operation_event").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('No history','NO-HISTORY','no-history',100)").execute(&pool).await.unwrap();
        sqlx::query("ALTER TABLE catalog_products ENABLE TRIGGER operation_event").execute(&pool).await.unwrap();
        let cookie=format!("talocart_session={}",authenticated(&identity).await);
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        for locale in ["it","en"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let body=html(fiscal_request(&app,"GET","/admin/catalog/products",&cookie,"",false).await).await;
            assert!(body.contains(if locale=="it" {">Ultima modifica<"} else {">Last modified<"}));
            assert!(body.contains("2099-01-16</span><span>07:45:00"));
            assert!(!body.contains("2099-01-17"));
            let legacy=html(fiscal_request(&app,"GET","/admin/catalog/products?q=No%20history",&cookie,"",false).await).await;
            assert!(legacy.contains("class=\"product-date-cell\"><span class=\"muted\">—</span>"));
            assert!(body.contains("data-product-column=\"model\" hidden"));
            assert!(!body.contains("data-product-column=\"modified_at\" hidden"));
            let head=body.split("<thead>").nth(1).unwrap().split("</tr>").next().unwrap();
            assert!(head.find("data-product-column=\"modified_at\"").unwrap()<head.find("data-product-column=\"actions\"").unwrap());
        }
        let mut plan_connection = pool.acquire().await.unwrap();
        sqlx::query("SET enable_seqscan=off").execute(&mut *plan_connection).await.unwrap();
        let plan:Vec<String>=sqlx::query_scalar("EXPLAIN SELECT created_at FROM operation_events WHERE resource IN ('catalog_products','catalog_products_translations') AND resource_key=$1 ORDER BY created_at DESC,id DESC LIMIT 1").bind(product.to_string()).fetch_all(&mut *plan_connection).await.unwrap();
        assert!(plan.join(" ").contains("product_modified_events_idx"));
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_modified_sort_is_chronological_before_pagination_with_ties_and_missing_dates() {
    use talocart_commerce::products::{AdminFilters, AdminOrder, AdminPageSize};
    isolated(|pool,_| async move {
        let (identity,session,products,_,_,_,_)=variant_fixture(&pool).await;
        let mut known=Vec::new();
        for index in 1..=23_i32 {
            let name=format!("Chrono {:02}",24-index);
            let id:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES($1,$2,$3,100) RETURNING id").bind(&name).bind(format!("CHRONO-{index}")).bind(format!("chrono-{index}")).fetch_one(&pool).await.unwrap();
            let day=if index==6 {5} else {index};
            sqlx::query("INSERT INTO operation_events(created_at,area,resource,resource_key,action) VALUES('2099-01-25 12:30:00+00'::timestamptz+make_interval(days=>$1),'catalog','catalog_products',$2,'UPDATE')").bind(day).bind(id.to_string()).execute(&pool).await.unwrap();
            known.push((id,day,name));
        }
        sqlx::query("ALTER TABLE catalog_products DISABLE TRIGGER operation_event").execute(&pool).await.unwrap();
        let missing:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('Chrono missing','CHRONO-MISSING','chrono-missing',100) RETURNING id").fetch_one(&pool).await.unwrap();
        sqlx::query("ALTER TABLE catalog_products ENABLE TRIGGER operation_event").execute(&pool).await.unwrap();
        let cookie=format!("talocart_session={}",authenticated(&identity).await);
        let app=router(test_app(pool.clone(),"Shop".into()).await);
        for (zone,style) in [("Europe/Rome","dmy"),("America/New_York","mdy"),("UTC","ymd")] {
            sqlx::query("UPDATE platform_regional_settings SET time_zone=$1,date_format=$2").bind(zone).bind(style).execute(&pool).await.unwrap();
            for direction in ["asc","desc"] {
                let mut expected=known.clone();
                expected.sort_by(|a,b| {let dates=if direction=="asc" {a.1.cmp(&b.1)} else {b.1.cmp(&a.1)};dates.then_with(||a.2.cmp(&b.2)).then_with(||a.0.cmp(&b.0))});
                let mut expected:Vec<i64>=expected.iter().map(|v|v.0).collect();expected.push(missing);
                let mut actual=Vec::new();
                for page in 1..=3 {
                    let result=products.admin_filtered(&session,&AdminFilters{q:"Chrono",order:AdminOrder::parse("modified_at",direction).unwrap(),page_size:AdminPageSize::parse_products("10").unwrap(),..Default::default()},page).await.unwrap();
                    assert_eq!(result.total,24);assert_eq!(result.page,page);
                    assert!(result.entries.iter().all(|entry|!entry.modified_at.contains(zone)));
                    actual.extend(result.entries.iter().map(|entry|entry.id));
                }
                assert_eq!(actual,expected,"raw instants with deterministic ties and nulls last, {zone}/{style}/{direction}");
                for locale in ["it","en"] {
                    sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
                    let body=html(fiscal_request(&app,"GET",&format!("/admin/catalog/products?q=Chrono&sort=modified_at&dir={direction}&per_page=10&page=2"),&cookie,"",false).await).await;
                    assert!(body.contains(&format!("aria-sort=\"{}\"",if direction=="asc" {"ascending"} else {"descending"})));
                    assert!(body.contains("sort=modified_at"));
                    assert!(body.contains("class=\"ui-date-time\""));
                    assert!(!body.contains("Europe/Rome")&&!body.contains("America/New_York"));
                }
            }
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_numeric_filters_match_sort_semantics_and_preserve_navigation() {
    use talocart_commerce::products::{AdminFilters, AdminOrder, AdminPageSize};
    isolated(|pool,_| async move {
        let (identity,session,products,variants,product,options,ids)=variant_fixture(&pool).await;
        variants.configure(&session,&session.csrf,product,&options).await.unwrap();
        sqlx::query("UPDATE catalog_products SET name='Numeric variants',price_cents=999999,stock=999 WHERE id=$1").bind(product).execute(&pool).await.unwrap();
        for (model,combo,price,stock,active) in [("NUM-V1",[ids[0],ids[2]],"6","3",true),("NUM-V2",[ids[1],ids[2]],"8","4",true),("NUM-V3",[ids[1],ids[3]],"1","999",false)] {
            let mut v=variant_values(model,&combo,price,stock);v.active=active;
            variants.save(&session,&session.csrf,product,None,&v).await.unwrap();
        }
        sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents,stock) SELECT 'Numeric '||i,'NUM-'||i,'num-'||i,i*100,i FROM generate_series(0,22) i").execute(&pool).await.unwrap();
        for (price,price_op,stock,stock_op,kind,count) in [
            ("6","eq","7","eq","variants",1), ("1","eq","999","eq","variants",0),
            ("6.00","eq","6","eq","simple",1), ("6,00","eq","","","",2),
            ("10","gt","20","lt","simple",9), ("0","eq","0","eq","simple",1),
            ("","","0","lt","",0), ("92233720368547758.07","gt","","","",0),
        ] {
            let result=products.admin_filtered(&session,&AdminFilters{q:"Numeric",price,price_op,stock,stock_op,kind,..Default::default()},1).await.unwrap();assert_eq!(result.total,count,"{price}/{price_op}/{stock}/{stock_op}/{kind}");
        }
        let cookie=format!("talocart_session={}",authenticated(&identity).await);let app=router(test_app(pool.clone(),"Shop".into()).await);
        for locale in ["it","en"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let body=html(fiscal_request(&app,"GET","/admin/catalog/products?q=Numeric&type=simple&price=1&price_op=gt&stock=22&stock_op=lt&sort=price&dir=desc&per_page=10&page=2",&cookie,"",false).await).await;
            assert_eq!(body.matches("scope=\"row\"").count(),10);
            assert!(body.contains("price=1&#38;price_op=gt&#38;stock=22&#38;stock_op=lt"));
            for (name,value) in [("price","1"),("price_op","gt"),("stock","22"),("stock_op","lt")] {
                let form=body.split("data-product-page-size").nth(1).unwrap().split("</form>").next().unwrap();assert!(form.contains(&format!("name=\"{name}\" value=\"{value}\"")));
            }
            assert!(body.contains(if locale=="it" {"Confronto prezzo"}else{"Price comparison"}));
        }
        let filters=AdminFilters{q:"Numeric",kind:"simple",price:"1",price_op:"gt",stock:"22",stock_op:"lt",order:AdminOrder::parse("price","desc").unwrap(),page_size:AdminPageSize::parse_products("10").unwrap(),..Default::default()};
        let result=products.admin_filtered(&session,&filters,2).await.unwrap();assert_eq!(result.total,20);assert_eq!(result.entries.iter().map(|e|e.stock).collect::<Vec<_>>(),(2..=11).rev().collect::<Vec<_>>());
        // No active variants means a missing effective value, never a fallback to the parent.
        sqlx::query("UPDATE catalog_variants SET active=false WHERE product_id=$1").bind(product).execute(&pool).await.unwrap();
        assert_eq!(products.admin_filtered(&session,&AdminFilters{kind:"variants",stock:"999",..Default::default()},1).await.unwrap().total,0);
        for query in ["price=-1","price=1e2","price=92233720368547758.08","stock=-1","stock=1.5","stock=9223372036854775808","price_op=eq%3BSELECT%201","stock_op=gte","modified_date=2027-02-29"] {
            assert_eq!(fiscal_request(&app,"GET",&format!("/admin/catalog/products?{query}"),&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY,"{query}");
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_modified_day_filter_uses_latest_event_and_local_dst_boundaries() {
    use talocart_commerce::products::{AdminFilters, AdminOrder};
    isolated(|pool,_| async move {
        let (identity,session,products,_,_,_,_)=variant_fixture(&pool).await;
        let mut known=Vec::new();
        for (i,instant) in ["2027-03-27T22:59:59Z","2027-03-27T23:00:00Z","2027-03-28T21:59:59Z","2027-03-28T22:00:00Z","2027-10-30T22:00:00Z","2027-10-31T22:59:59Z","2027-10-31T23:00:00Z"].iter().enumerate() {
            let id:i64=sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES($1,$2,$3,100) RETURNING id").bind(format!("Day {i}")).bind(format!("DAY-{i}")).bind(format!("day-{i}")).fetch_one(&pool).await.unwrap();
            sqlx::query("INSERT INTO operation_events(created_at,area,resource,resource_key,action) VALUES($1::text::timestamptz,'catalog','catalog_products',$2,'UPDATE')").bind(instant).bind(id.to_string()).execute(&pool).await.unwrap();known.push(id);
        }
        // An earlier matching event cannot make a product whose latest event is outside match.
        sqlx::query("INSERT INTO operation_events(created_at,area,resource,resource_key,action) VALUES('2027-03-28T12:00:00Z','catalog','catalog_products',$1,'UPDATE')").bind(known[4].to_string()).execute(&pool).await.unwrap();
        sqlx::query("ALTER TABLE catalog_products DISABLE TRIGGER operation_event").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('Day missing','DAY-MISSING','day-missing',100)").execute(&pool).await.unwrap();
        sqlx::query("ALTER TABLE catalog_products ENABLE TRIGGER operation_event").execute(&pool).await.unwrap();
        for (zone,date,indices) in [("Europe/Rome","2027-03-28",vec![1,2]),("UTC","2027-03-28",vec![2,3]),("Europe/Rome","2027-10-31",vec![4,5]),("UTC","2027-10-31",vec![5,6]),("Europe/Rome","2027-04-01",vec![])] {
            sqlx::query("UPDATE platform_regional_settings SET time_zone=$1").bind(zone).execute(&pool).await.unwrap();
            for sort in ["model","modified_at"] {
                let result=products.admin_filtered(&session,&AdminFilters{q:"Day",modified_date:date,order:AdminOrder::parse(sort,"asc").unwrap(),..Default::default()},1).await.unwrap();
                assert_eq!(result.entries.iter().map(|e|e.id).collect::<Vec<_>>(),indices.iter().map(|&i|known[i]).collect::<Vec<_>>(),"{zone}/{date}/{sort}");
            }
        }
        let cookie=format!("talocart_session={}",authenticated(&identity).await);let app=router(test_app(pool.clone(),"Shop".into()).await);
        for locale in ["it","en"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let body=html(fiscal_request(&app,"GET","/admin/catalog/products?q=Day&modified_date=2027-03-28&sort=modified_at&dir=desc",&cookie,"",false).await).await;
            assert!(body.contains("name=\"modified_date\" value=\"2027-03-28\""));assert!(body.contains("modified_date=2027-03-28"));
            assert!(body.contains(if locale=="it" {"Filtra per ultima modifica"} else {"Filter by last modified"}));
        }
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_id_filter_is_exact_combines_and_preserves_http_navigation() {
    use talocart_commerce::products::{AdminFilters, AdminOrder};
    isolated(|pool,_| async move {
        let (identity,session,products,_,product,_,_)=variant_fixture(&pool).await;
        let prefix_id=product*10;
        sqlx::query("INSERT INTO catalog_products(id,name,model,slug,price_cents) OVERRIDING SYSTEM VALUE VALUES($1,'Prefix ID product','PREFIX-ID','prefix-id',100)").bind(prefix_id).execute(&pool).await.unwrap();
        for sort in ["id","price","modified_at"] {
            let result=products.admin_filtered(&session,&AdminFilters{id:Some(product),order:AdminOrder::parse(sort,"desc").unwrap(),..Default::default()},99).await.unwrap();
            assert_eq!(result.total,1);assert_eq!(result.page,1);assert_eq!(result.entries[0].id,product);
        }
        for filters in [AdminFilters{id:Some(i64::MAX),..Default::default()},AdminFilters{id:Some(product),q:"no-such-name",..Default::default()},AdminFilters{id:Some(product),stock:"9223372036854775807",..Default::default()}] {
            assert_eq!(products.admin_filtered(&session,&filters,1).await.unwrap().total,0);
        }
        let cookie=format!("talocart_session={}",authenticated(&identity).await);let app=router(test_app(pool.clone(),"Shop".into()).await);
        for locale in ["it","en"] {
            sqlx::query("UPDATE administrators SET locale=$1").bind(locale).execute(&pool).await.unwrap();
            let body=html(fiscal_request(&app,"GET",&format!("/admin/catalog/products?id={product}&sort=id&dir=desc&page=99&per_page=10"),&cookie,"",false).await).await;
            assert_eq!(body.matches("scope=\"row\"").count(),1);
            assert!(body.contains(&format!("class=\"product-id-cell\">{product}</td>")));assert!(!body.contains(&format!("class=\"product-id-cell\">{prefix_id}</td>")));
            assert!(body.contains(if locale=="it" {"Filtra per ID"}else{"Filter by ID"}));
            assert!(body.contains(&format!("id={product}&#38;page=1")));
            let form=body.split("data-product-page-size").nth(1).unwrap().split("</form>").next().unwrap();assert!(form.contains(&format!("name=\"id\" value=\"{product}\"")));
            assert!(body.contains("name=\"id\" value="));
            let empty=html(fiscal_request(&app,"GET","/admin/catalog/products?id=9223372036854775807",&cookie,"",false).await).await;
            assert_eq!(empty.matches("scope=\"row\"").count(),0);assert!(empty.contains(if locale=="it" {"Nessun risultato"}else{"No results"}));
        }
        sqlx::query("UPDATE administrators SET product_hidden_columns=ARRAY['id']").execute(&pool).await.unwrap();
        let hidden=html(fiscal_request(&app,"GET",&format!("/admin/catalog/products?id={product}"),&cookie,"",false).await).await;
        assert!(hidden.contains("data-product-column=\"id\" hidden"));assert_eq!(hidden.matches("scope=\"row\"").count(),1);
        for bad in ["0","-1","%2B1","1.5","1e2","%201%20","9223372036854775808","1%3BSELECT%201","abc","%EF%BC%91"] {
            assert_eq!(fiscal_request(&app,"GET",&format!("/admin/catalog/products?id={bad}"),&cookie,"",false).await.status(),StatusCode::UNPROCESSABLE_ENTITY,"{bad}");
        }
        assert_eq!(fiscal_request(&app,"GET",&format!("/admin/catalog/products?id={product}"),"","",false).await.status(),StatusCode::SEE_OTHER);
    }).await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_actions_and_empty_selection_keep_filters_accessible() {
    isolated(|pool, _| async move {
        let (identity, _, _, _, _, _, _) = variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let response = fiscal_request(
            &app,
            "POST",
            "/admin/catalog/products/columns",
            &cookie,
            &format!("csrf={}", session.csrf),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(identity.product_columns(&session).await.unwrap().count(), 1);
        let body = html(
            fiscal_request(
                &app,
                "GET",
                "/admin/catalog/products?q=missing",
                &cookie,
                "",
                false,
            )
            .await,
        )
        .await;
        assert!(!body.contains("data-product-column=\"actions\" hidden"));
        assert!(!body.contains("value=\"actions\""));
        assert!(body.contains("colspan=\"1\" data-product-empty"));
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated PostgreSQL"]
async fn import_writer_scopes_permissions_and_preserves_caller_receipt_rollback() {
    isolated(|pool, _| async move {
        use talocart_commerce::imports as intake;
        use talocart_platform::{content::Translation, identity::Permission};
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.expect("lookup owner").expect("owner session");
        let actor = session.administrator_id.expect("authenticated owner ID");
        let brand:i64=sqlx::query_scalar("INSERT INTO catalog_brands(name,slug,active,sort_order) VALUES('Intake','intake',true,0) RETURNING id").fetch_one(&pool).await.expect("brand fixture");
        sqlx::query("CREATE TABLE synthetic_intake_receipts(product_id bigint NOT NULL REFERENCES catalog_products(id))").execute(&pool).await.expect("caller-owned receipt fixture");
        let input=intake::Product {
            name:" Intake product ".into(),model:"intake".into(),slug:" intake-product ".into(),description:" Body ".into(),price_cents:1200,stock:5,minimum:1,packs:1,
            brand_id:brand,category_id:None,tax_class_id:None,
            translations:vec![Translation{locale:"it".into(),name:"Intake product".into(),..Default::default()}],axes:vec![],variants:vec![],
        };
        let mut tx=pool.begin().await.expect("caller transaction");
        {
            let mut writer=intake::Writer::authorize_in(&mut tx,actor,Permission::BrandsWrite).await.expect("brand-scoped writer");
            assert!(matches!(writer.create_product(&input).await,Err(intake::Error::Access(talocart_platform::technical::AccessError::Forbidden))),"an owner cannot silently broaden the writer scope");
        }
        let product=intake::Writer::authorize_in(&mut tx,actor,Permission::ProductsWrite).await.expect("product-scoped writer").create_product(&input).await.expect("draft intake");
        let stored:(String,String,String,String)=sqlx::query_as("SELECT model,name,slug,description FROM catalog_products WHERE id=$1").bind(product).fetch_one(&mut *tx).await.expect("canonical stored values");
        assert_eq!(stored,("INTAKE".into(),"Intake product".into(),"intake-product".into(),"Body".into()));
        sqlx::query("INSERT INTO synthetic_intake_receipts(product_id) VALUES($1)").bind(product).execute(&mut *tx).await.expect("receipt in the same transaction");
        tx.rollback().await.expect("caller rollback");
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM catalog_products WHERE id=$1").bind(product).fetch_one(&pool).await.expect("product rolled back"),0);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM synthetic_intake_receipts").fetch_one(&pool).await.expect("receipt rolled back"),0);
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn products_rich_description_round_trip_sanitization_and_plain_seo() {
    use talocart_commerce::products::Products;
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let products = Products::new(pool.clone(), identity.clone(), test_media(&pool).await);
        let (_, _, class) = product_references(&pool, &session).await;
        let mut values = product_values("RICH", "rich-product");
        values.status = "published".into();
        values.tax_class_id = class.to_string();
        values.description = talocart_platform::rich_text::submitted("<h2>Features</h2><p>Soft <strong>cotton</strong> &amp; comfort.</p><img src=x onerror='evil()'><script>evil()</script>", "html");
        let id = products.save(&session, &session.csrf, None, &values).await.unwrap();
        let stored = products.entry(&session, id).await.unwrap();
        assert!(stored.description.contains("<strong>cotton</strong>"));
        assert!(!stored.description.contains("evil"));
        let app = router(test_app(pool.clone(), "Rich products".into()).await);
        let body = html(request(&app, "GET", "/products/rich-product", "", "", false).await).await;
        assert!(body.contains("<strong>cotton</strong>"));
        assert!(body.contains("name=\"description\" content=\"Features Soft cotton &#38; comfort.\""), "plain SEO metadata");
        assert!(!body.contains(talocart_platform::rich_text::PREFIX));
        let legacy = "Literal <strong>cotton</strong> & details.\nSecond line.";
        let mut edit = stored.values(); edit.description = legacy.into();
        products.save(&session, &session.csrf, Some(id), &edit).await.unwrap();
        assert_eq!(products.entry(&session, id).await.unwrap().description, legacy);
        let body = html(request(&app, "GET", "/products/rich-product", "", "", false).await).await;
        assert!(body.contains("&lt;strong&gt;cotton&lt;/strong&gt;"));
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn products_rich_form_accepts_encoded_long_text_and_bounds_requests() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Rich products".into()).await);
        let description = format!("<p>{}</p>", "è".repeat(3_900));
        let encoded = format!("%3Cp%3E{}%3C%2Fp%3E", "%C3%A8".repeat(3_900));
        let form = format!("csrf={}&name=Long&model=LONG&slug=long-rich&price=12.34&manage_stock=true&stock_revision=0&stock=5&status=draft&price_mode=inclusive&description_format=html&description={encoded}", session.csrf);
        assert!(form.len() > 16 * 1024);
        assert_eq!(request(&app, "POST", "/admin/catalog/products/new", &cookie, &form, false).await.status(), StatusCode::SEE_OTHER);
        let id: i64 = sqlx::query_scalar("SELECT id FROM catalog_products WHERE slug='long-rich'").fetch_one(&pool).await.unwrap();
        let path = format!("/admin/catalog/products/{id}");
        assert_eq!(request(&app, "POST", &path, &cookie, &form, false).await.status(), StatusCode::SEE_OTHER);
        let stored: String = sqlx::query_scalar("SELECT description FROM catalog_products WHERE id=$1").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(talocart_platform::rich_text::source(&stored), description);
        let excessive = format!("padding={}", "x".repeat(128 * 1024));
        for path in ["/admin/catalog/products/new", &path] {
            assert_eq!(request(&app, "POST", path, &cookie, &excessive, false).await.status(), StatusCode::PAYLOAD_TOO_LARGE);
        }
    }).await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn product_list_description_tooltip_decodes_rich_text_and_escapes_literal_text() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let cookie = format!("talocart_session={raw}");
        let products = talocart_commerce::products::Products::new(
            pool.clone(),
            identity,
            test_media(&pool).await,
        );
        let mut values = product_values("TOOLTIP", "tooltip-product");
        let rich = talocart_platform::rich_text::submitted(
            "<p>Soft <strong>cotton</strong> &amp; &lt;script&gt;literal&lt;/script&gt;</p>",
            "html",
        );
        values.description = rich.clone();
        let id = products
            .save(&session, &session.csrf, None, &values)
            .await
            .unwrap();
        let app = router(test_app(pool.clone(), "Tooltip".into()).await);
        for (description, expected) in [
            (
                rich.as_str(),
                "Soft cotton &#38; &#60;script&#62;literal&#60;/script&#62; ",
            ),
            (
                "Literal <strong>cotton</strong> & details.",
                "Literal &#60;strong&#62;cotton&#60;/strong&#62; &#38; details.",
            ),
        ] {
            let mut edit = products.entry(&session, id).await.unwrap().values();
            edit.description = description.into();
            products
                .save(&session, &session.csrf, Some(id), &edit)
                .await
                .unwrap();
            for locale in ["en", "it"] {
                sqlx::query("UPDATE administrators SET locale=$1")
                    .bind(locale)
                    .execute(&pool)
                    .await
                    .unwrap();
                let response =
                    request(&app, "GET", "/admin/catalog/products", &cookie, "", false).await;
                assert_eq!(response.status(), StatusCode::OK);
                let body = html(response).await;
                let marker = format!("id=\"product-description-{id}\" role=\"tooltip\">");
                let tooltip = body
                    .split_once(&marker)
                    .unwrap()
                    .1
                    .split_once("</div>")
                    .unwrap()
                    .0;
                assert_eq!(
                    tooltip, expected,
                    "Localized tooltip must contain escaped readable text"
                );
            }
        }
    })
    .await;
}

#[tokio::test]
#[ignore = "Richiede TALOCART_TEST_DATABASE_URL verso un database di test dedicato"]
async fn product_editor_visibility_and_reused_variant_count_match_public_contract() {
    use talocart_commerce::products::ProductError;
    isolated(|pool, _| async move {
        let (identity, session, products, variants, product, options, ids) =
            variant_fixture(&pool).await;
        let raw = authenticated(&identity).await;
        let cookie = format!("talocart_session={raw}");
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let entry = products.entry(&session, product).await.unwrap();
        let category = entry.category_id.unwrap();
        let brand = entry.brand_id.unwrap();
        for locale in ["it", "en"] {
            sqlx::query("UPDATE administrators SET locale=$1")
                .bind(locale)
                .execute(&pool)
                .await
                .unwrap();
            for (change, id, visible) in [
                (
                    "UPDATE catalog_products SET status='published' WHERE id=$1",
                    product,
                    true,
                ),
                (
                    "UPDATE catalog_products SET status='draft' WHERE id=$1",
                    product,
                    false,
                ),
                (
                    "UPDATE catalog_products SET status='published' WHERE id=$1",
                    product,
                    true,
                ),
                (
                    "UPDATE catalog_categories SET active=false WHERE id=$1",
                    category,
                    false,
                ),
                (
                    "UPDATE catalog_categories SET active=true WHERE id=$1",
                    category,
                    true,
                ),
                (
                    "UPDATE catalog_brands SET active=false WHERE id=$1",
                    brand,
                    false,
                ),
                (
                    "UPDATE catalog_brands SET active=true WHERE id=$1",
                    brand,
                    true,
                ),
            ] {
                sqlx::query(change).bind(id).execute(&pool).await.unwrap();
                assert_eq!(products.public_visible("model").await.unwrap(), visible);
                assert_eq!(products.public_entry("model").await.is_ok(), visible);
                let response = request(
                    &app,
                    "GET",
                    &format!("/admin/catalog/products/{product}"),
                    &cookie,
                    "",
                    false,
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    html(response).await.contains("product-public-link"),
                    visible
                );
            }
        }
        for slug in ["missing", "' OR true --"] {
            assert!(!products.public_visible(slug).await.unwrap());
        }
        variants
            .configure(&session, &session.csrf, product, &options)
            .await
            .unwrap();
        assert!(!products.public_visible("model").await.unwrap());
        for (n, active) in [true, false].into_iter().enumerate() {
            let mut values =
                variant_values(&format!("EDITOR-{n}"), &[ids[0], ids[2 + n]], "1", "0");
            values.active = active;
            variants
                .save(&session, &session.csrf, product, None, &values)
                .await
                .unwrap();
        }
        let editor = products.editor_entry(&session, product).await.unwrap();
        assert_eq!(
            editor.product.variant_count, 2,
            "Include inactive variants too"
        );
        let configuration = variants.configuration(&session, product).await.unwrap();
        assert_eq!(configuration.count, editor.product.variant_count);
        assert_eq!(
            variants
                .configuration_axes(&session, product)
                .await
                .unwrap()
                .len(),
            configuration.axes.len()
        );
        assert!(
            products.public_visible("model").await.unwrap(),
            "Zero stock does not hide an active variant"
        );
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
                    &format!("/admin/catalog/products/{product}?tab=variants"),
                    &cookie,
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(
                body.contains(" — 2 "),
                "Variant summary must reuse the complete count"
            );
            assert!(body.contains("product-public-link"));
        }
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("/admin/catalog/products/{product}"),
                "",
                "",
                false
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        sqlx::query("UPDATE administrators SET active=false")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            variants
                .configuration_axes(&session, product)
                .await
                .is_err(),
            "Axes must check current authorization"
        );
        pool.close().await;
        assert!(
            matches!(
                products.public_visible("model").await,
                Err(ProductError::Technical(_))
            ),
            "Database errors must not become hidden products"
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn related_products_precedence_visibility_permissions_and_http() {
    isolated(|pool, _| async move {
        let identity = installed(&pool).await;
        let raw = authenticated(&identity).await;
        let session = identity.session(&raw).await.unwrap().unwrap();
        let products = talocart_commerce::products::Products::new(
            pool.clone(),
            identity.clone(),
            test_media(&pool).await,
        );
        let (category, brand, class) = product_references(&pool, &session).await;
        let mut ids = Vec::new();
        for n in 0..15 {
            let mut values = product_values(&format!("RELATED-{n}"), &format!("related-{n}"));
            values.status = "published".into();
            values.category_id = category.to_string();
            values.brand_id = brand.to_string();
            values.tax_class_id = class.to_string();
            ids.push(
                products
                    .save(&session, &session.csrf, None, &values)
                    .await
                    .unwrap(),
            );
        }
        let source = ids[0];
        for query in ["", "   "] {
            assert!(
                products
                    .related_editor(&session, source, query)
                    .await
                    .unwrap()
                    .candidates
                    .is_empty()
            );
        }
        assert_eq!(
            products
                .related_editor(&session, source, "RELATED-1")
                .await
                .unwrap()
                .candidates
                .len(),
            6
        );

        assert!(products.public_related(source).await.unwrap().is_empty());
        sqlx::query("UPDATE catalog_products SET brand_id=NULL WHERE id=$1 OR id=$2")
            .bind(ids[1])
            .bind(ids[3])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE catalog_products SET category_id=NULL WHERE id=$1 OR id=$2")
            .bind(ids[2])
            .bind(ids[3])
            .execute(&pool)
            .await
            .unwrap();
        for mode in ["category", "brand", "either"] {
            products
                .change_related(&session, &session.csrf, source, "fallback", 0, mode)
                .await
                .unwrap();
            let rows = products.public_related(source).await.unwrap();
            assert_eq!(rows.len(), 4);
            assert!(rows.iter().all(|row| row.id != source));
            assert!(!rows.iter().any(|row| row.id == ids[3]));
            assert_eq!(rows.iter().any(|row| row.id == ids[1]), mode != "brand");
            assert_eq!(rows.iter().any(|row| row.id == ids[2]), mode != "category");
        }
        sqlx::query("UPDATE catalog_products SET category_id=$1,brand_id=$2 WHERE id=ANY($3)")
            .bind(category)
            .bind(brand)
            .bind(&ids)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            products
                .change_related(&session, "bad-csrf", source, "add", ids[1], "")
                .await
                .is_err()
        );
        assert!(
            products
                .change_related(&session, &session.csrf, source, "add", source, "")
                .await
                .is_err()
        );
        assert!(
            products
                .change_related(&session, &session.csrf, source, "fallback", 0, "invalid")
                .await
                .is_err()
        );
        products
            .change_related(&session, &session.csrf, source, "add", ids[1], "")
            .await
            .unwrap();
        let rows = products.public_related(source).await.unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [ids[1]]);
        assert!(
            products.public_related(ids[1]).await.unwrap().is_empty(),
            "links are directional"
        );
        sqlx::query("UPDATE catalog_products SET status='draft' WHERE id=$1")
            .bind(ids[1])
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            products.public_related(source).await.unwrap().is_empty(),
            "hidden explicit links never activate fallback"
        );
        products
            .change_related(&session, &session.csrf, source, "remove", ids[1], "")
            .await
            .unwrap();
        assert_eq!(products.public_related(source).await.unwrap().len(), 4);
        for &target in &ids[1..13] {
            products
                .change_related(&session, &session.csrf, source, "add", target, "")
                .await
                .unwrap();
        }
        assert_eq!(
            products
                .public_related(source)
                .await
                .unwrap()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            ids[2..6]
        );
        assert!(
            products
                .change_related(&session, &session.csrf, source, "add", ids[13], "")
                .await
                .is_err()
        );
        products
            .change_related(&session, &session.csrf, source, "remove", ids[5], "")
            .await
            .unwrap();
        products
            .change_related(&session, &session.csrf, source, "add", ids[13], "")
            .await
            .unwrap();
        let app = router(test_app(pool.clone(), "Shop".into()).await);
        let cookie = format!("talocart_session={raw}");
        let route = format!("/admin/catalog/products/{source}/related");
        for (locale, heading) in [("it", "Prodotti correlati"), ("en", "Related products")] {
            identity
                .update_locale(&session, &session.csrf, locale)
                .await
                .unwrap();
            let response = request(&app, "GET", &route, &cookie, "", false).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(
                response.headers()[header::LOCATION]
                    .to_str()
                    .unwrap()
                    .ends_with("#panel-related")
            );
            let response = request(
                &app,
                "GET",
                &format!("/admin/catalog/products/{source}?tab=related&related_q=RELATED"),
                &cookie,
                "",
                false,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = html(response).await;
            assert!(body.contains(heading));
            assert!(body.contains("id=\"panel-related\""));
            assert!(
                body.find("id=\"tab-variants\"").unwrap()
                    < body.find("id=\"tab-related\"").unwrap()
            );
            assert!(
                body.find("id=\"tab-related\"").unwrap() < body.find("id=\"tab-delete\"").unwrap()
            );
            let body = html(
                request(
                    &app,
                    "GET",
                    &format!(
                        "{}/products/related-0",
                        if locale == "it" { "" } else { "/en" }
                    ),
                    "",
                    "",
                    false,
                )
                .await,
            )
            .await;
            assert!(body.contains("id=\"related-title\""));
        }
        assert_eq!(
            request(
                &app,
                "POST",
                &route,
                &cookie,
                "csrf=wrong&action=fallback&fallback=none",
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let response = request(
            &app,
            "POST",
            &route,
            &cookie,
            &format!(
                "csrf={}&action=fallback&fallback=none&related_q=RELATED",
                session.csrf
            ),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(
            response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .contains("related_q=RELATED#panel-related")
        );
        sqlx::query("UPDATE administrators SET active=false")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            products
                .change_related(&session, &session.csrf, source, "remove", ids[1], "")
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn related_products_upgrade_preserves_catalog() {
    isolated_before(2, |pool, _| async move {
        let identity = installed(&pool).await;
        let session = taxonomy_session(&identity).await;
        let products = talocart_commerce::products::Products::new(
            pool.clone(),
            identity,
            test_media(&pool).await,
        );
        // Construct the historical fixture using its own schema, before new columns exist.
        let id: i64 = sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents) VALUES('Preserved','PRESERVED','preserved',0) RETURNING id")
            .fetch_one(&pool).await.unwrap();
        database::migrate(&pool).await.unwrap();
        database::migrate(&pool).await.unwrap();
        assert_eq!(
            products.entry(&session, id).await.unwrap().model,
            "PRESERVED"
        );
        assert_eq!(
            products
                .related_editor(&session, id, "")
                .await
                .unwrap()
                .fallback,
            "none"
        );
        assert!(products.public_related(id).await.unwrap().is_empty());
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn unmanaged_stock_preserves_quantities_rules_visibility_and_stale_edit_protection() {
    isolated(|pool, _| async move {
        let (_, session, products, variants, product, options, ids) = variant_fixture(&pool).await;
        let original = products.entry(&session, product).await.unwrap();
        assert!(original.manage_stock);
        let mut values = original.values();
        values.manage_stock = false;
        values.stock = "forged".into();
        values.minimum_quantity = "10".into();
        values.packs = "6".into();
        products
            .save(&session, &session.csrf, Some(product), &values)
            .await
            .unwrap();
        let saved = products.entry(&session, product).await.unwrap();
        assert_eq!(saved.stock, original.stock);
        assert_eq!(saved.stock_revision, original.stock_revision + 1);
        assert!(!saved.manage_stock);
        assert!(matches!(
            products
                .save(&session, &session.csrf, Some(product), &original.values())
                .await,
            Err(talocart_commerce::products::ProductError::StockChanged)
        ));
        let carts = talocart_commerce::cart::Carts::new(pool.clone());
        let (_, guest) = carts.create().await.unwrap();
        for quantity in [1, 10, 13] {
            assert!(matches!(
                carts
                    .add(
                        &guest,
                        &guest.csrf,
                        0,
                        (product, 0),
                        quantity,
                        talocart_platform::content::Language::new(
                            talocart_platform::locale::Locale::parse("it").unwrap(),
                            talocart_platform::locale::Locale::parse("it").unwrap()
                        )
                    )
                    .await,
                Err(talocart_commerce::cart::CartError::Quantity)
            ));
        }
        carts
            .add(
                &guest,
                &guest.csrf,
                0,
                (product, 0),
                900,
                talocart_platform::content::Language::new(
                    talocart_platform::locale::Locale::parse("it").unwrap(),
                    talocart_platform::locale::Locale::parse("it").unwrap(),
                ),
            )
            .await
            .unwrap();
        for language in [
            talocart_platform::content::Language::new(
                talocart_platform::locale::Locale::parse("it").unwrap(),
                talocart_platform::locale::Locale::parse("it").unwrap(),
            ),
            talocart_platform::content::Language::new(
                talocart_platform::locale::Locale::parse("en").unwrap(),
                talocart_platform::locale::Locale::parse("it").unwrap(),
            ),
        ] {
            let localized = products.localized(language);
            assert!(localized.public_entry("model").await.unwrap().available());
            assert!(
                localized
                    .public_page(
                        "",
                        "",
                        "",
                        true,
                        1,
                        talocart_commerce::products::CatalogPageSize::default()
                    )
                    .await
                    .unwrap()
                    .entries
                    .iter()
                    .any(|entry| entry.id == product && entry.available())
            );
        }
        variants
            .configure(&session, &session.csrf, product, &options)
            .await
            .unwrap();
        assert!(
            !products.public_visible("model").await.unwrap(),
            "No active combination still hides the product"
        );
        let variant = variants
            .save(
                &session,
                &session.csrf,
                product,
                None,
                &variant_values("UNLIMITED", &[ids[0], ids[2]], "20", "99"),
            )
            .await
            .unwrap();
        assert_eq!(
            variants
                .entry(&session, product, variant)
                .await
                .unwrap()
                .stock,
            0
        );
        sqlx::query("UPDATE catalog_variants SET stock=7 WHERE id=$1")
            .bind(variant)
            .execute(&pool)
            .await
            .unwrap();
        let mut v = variants
            .entry(&session, product, variant)
            .await
            .unwrap()
            .values("inclusive");
        v.stock = "invalid".into();
        variants
            .save(&session, &session.csrf, product, Some(variant), &v)
            .await
            .unwrap();
        assert_eq!(
            variants
                .entry(&session, product, variant)
                .await
                .unwrap()
                .stock,
            7
        );
        assert!(products.public_entry("model").await.unwrap().available());
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires dedicated test PostgreSQL"]
async fn initial_schema_defines_managed_stock_and_repeated_migration_preserves_it() {
    isolated_before(1, |pool, _| async move {
        let product: i64 = sqlx::query_scalar("INSERT INTO catalog_products(name,model,slug,price_cents,stock) VALUES('Initial','INITIAL','initial',1000,10) RETURNING id")
            .fetch_one(&pool).await.unwrap();
        let managed: bool = sqlx::query_scalar("SELECT manage_stock FROM catalog_products WHERE id=$1")
            .bind(product)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(managed);
        let before = snapshot_tables(&pool).await;
        database::migrate(&pool).await.unwrap();
        database::migrate(&pool).await.unwrap();
        assert_tables_preserved(&pool, before).await;
        let stored: (bool, i32) =
            sqlx::query_as("SELECT manage_stock,stock FROM catalog_products WHERE id=$1")
                .bind(product)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, (true, 10));
        database::check(&pool).await.unwrap();
    })
    .await;
}
