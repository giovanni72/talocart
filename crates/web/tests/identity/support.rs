use super::*;

pub(super) async fn isolated<F, Fut>(run: F)
where
    F: FnOnce(PgPool, PgConnectOptions) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    isolated_at(None, run).await;
}
pub(super) async fn isolated_before<F, Fut>(version: i64, run: F)
where
    F: FnOnce(PgPool, PgConnectOptions) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    isolated_at(Some(version), run).await;
}
async fn isolated_at<F, Fut>(version: Option<i64>, run: F)
where
    F: FnOnce(PgPool, PgConnectOptions) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    isolated_schema_at(version, None, run).await;
}
pub(super) async fn isolated_schema_at<F, Fut>(
    version: Option<i64>,
    module: Option<&'static sqlx::migrate::Migrator>,
    run: F,
) where
    F: FnOnce(PgPool, PgConnectOptions) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let url = talocart_platform::config::environment("TALOCART_TEST_DATABASE_URL")
        .and_then(|value| {
            value.ok_or(talocart_platform::config::ConfigError::Invalid(
                "test environment required",
            ))
        })
        .expect("URL test richiesta");
    static NEXT_SCHEMA: AtomicU64 = AtomicU64::new(0);
    let schema = format!(
        "talocart_identity_{}_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed)
    );
    let owner = PgPool::connect(&url).await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&owner)
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options.clone())
        .await
        .unwrap();
    let test_pool = pool.clone();
    let result = tokio::spawn(async move {
        if let Some(version) = version {
            sqlx::migrate::Migrator {
                migrations: std::borrow::Cow::Owned(
                    database::MIGRATOR
                        .iter()
                        .filter(|m| m.version <= version)
                        .cloned()
                        .collect(),
                ),
                ..sqlx::migrate::Migrator::DEFAULT
            }
            .run(&test_pool)
            .await
            .unwrap();
        } else {
            database::migrate(&test_pool).await.unwrap();
        }
        if let Some(module) = module {
            module
                .run(&test_pool)
                .await
                .expect("Module fixture migrations");
        }
        run(test_pool, options).await;
    })
    .await;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&owner)
        .await
        .unwrap();
    owner.close().await;
    let media_root = std::env::temp_dir().join(&schema);
    if media_root.exists() {
        tokio::fs::remove_dir_all(media_root).await.unwrap();
    }
    result.unwrap();
}

/// Rows of every application table, keyed by the columns that existed at capture time.
pub(super) struct TableSnapshot(Vec<(String, Vec<String>, String)>);

// Later migrations may add columns with defaults: upgrades must preserve the original ones.
pub(super) async fn snapshot_tables(pool: &PgPool) -> TableSnapshot {
    let tables: Vec<String> = sqlx::query_scalar("SELECT table_name::text FROM information_schema.tables WHERE table_schema=current_schema() AND table_type='BASE TABLE' AND table_name<>'_sqlx_migrations' ORDER BY table_name").fetch_all(pool).await.unwrap();
    let mut snapshot = Vec::with_capacity(tables.len());
    for table in tables {
        let columns = table_columns(pool, &table).await;
        let rows = table_rows(pool, &table, &[]).await;
        snapshot.push((table, columns, rows));
    }
    TableSnapshot(snapshot)
}

pub(super) async fn assert_tables_preserved(pool: &PgPool, snapshot: TableSnapshot) {
    for (table, columns, before) in snapshot.0 {
        let added: Vec<String> = table_columns(pool, &table)
            .await
            .into_iter()
            .filter(|column| !columns.contains(column))
            .collect();
        assert_eq!(
            before,
            table_rows(pool, &table, &added).await,
            "preserved {table}"
        );
    }
}

async fn table_columns(pool: &PgPool, table: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT column_name::text FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=$1 ORDER BY column_name").bind(table).fetch_all(pool).await.unwrap()
}

async fn table_rows(pool: &PgPool, table: &str, excluded: &[String]) -> String {
    assert!(
        table.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'),
        "unexpected table name {table}"
    );
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT coalesce(jsonb_agg(r ORDER BY r::text),'[]')::text FROM (SELECT to_jsonb(t)-$1::text[] AS r FROM {table} t) s"))).bind(excluded).fetch_one(pool).await.unwrap()
}

// Publication locks are shared across schemas: retry Busy without weakening cleanup assertions.
pub(super) async fn completed_prune(
    media: &talocart_platform::media::MediaStore,
    pool: &PgPool,
    dry: bool,
) -> talocart_platform::media::PruneReport {
    for _ in 0..200 {
        let report = media.prune(pool, dry).await.unwrap();
        if !report.busy {
            return report;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("Maintenance lock did not become available")
}

pub(super) async fn request(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    body: &str,
    partial: bool,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("hx-request", if partial { "true" } else { "false" })
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Upper bound for one rendered page in tests: generous enough for the largest
/// paginated editors (50 option values with inline icons), still finite.
pub(super) async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

pub(super) fn cookie(response: &Response) -> String {
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

pub(super) fn csrf(html: &str) -> String {
    html.split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

pub(super) async fn test_media(pool: &PgPool) -> talocart_platform::media::MediaStore {
    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(schema.starts_with("talocart_identity_"));
    talocart_platform::media::MediaStore::new(std::env::temp_dir().join(schema))
}

pub(super) async fn test_identity(pool: PgPool) -> Identity {
    let media = test_media(&pool).await;
    Identity::new(pool).with_media(media)
}

pub(super) async fn test_app(pool: PgPool, name: String) -> AppState {
    test_app_config(pool, name, Default::default()).await
}

pub(super) async fn test_app_config(pool: PgPool, name: String, mut config: talocart_web::AppStateConfig) -> AppState {
    if config.media.root() == talocart_platform::media::MediaStore::new("storage/media".into()).root() {
        config.media = test_media(&pool).await;
    }
    if config.mail_key.is_none() {
        config.mail_key=Some(std::sync::Arc::new(talocart_platform::mail::MailKey::from_bytes(&[23;32]).unwrap()));
    }
    AppState::configured(pool, name, config)
}

pub(super) async fn installed(pool: &PgPool) -> Identity {
    let service = test_identity(pool.clone()).await;
    service
        .bootstrap(
            "owner@example.com",
            "correct horse battery".into(),
            "Shop",
            "",
        )
        .await
        .unwrap();
    service
}

/// Historical schemas are seeded directly; current bootstrap requires the current schema.
/// Fixed Argon2id hash of the public synthetic password "correct horse battery".
pub(super) async fn authenticated(service: &Identity) -> String {
    let (_, anon) = service.anonymous().await.unwrap();
    service
        .login(
            &anon,
            &anon.csrf,
            "owner@example.com",
            "correct horse battery".into(),
        )
        .await
        .unwrap()
}

pub(super) fn assert_cookie_lifetime(response: &Response, seconds: i64) {
    let header = response.headers()[header::SET_COOKIE].to_str().unwrap();
    for attribute in [
        "HttpOnly",
        "SameSite=Strict",
        "Secure",
        "Path=/",
        &format!("Max-Age={seconds}"),
    ] {
        assert!(header.contains(attribute));
    }
}

pub(super) async fn login_as(service: &Identity, address: &str) -> String {
    let (_, anon) = service.anonymous().await.unwrap();
    service
        .login(&anon, &anon.csrf, address, "correct horse battery".into())
        .await
        .unwrap()
}

pub(super) fn cookie_from_response(response: &Response) -> String {
    cookie(response)
}

pub(super) fn avatar_fixture() -> Vec<u8> {
    use std::io::Cursor;
    let image = image::RgbImage::from_fn(180, 180, |x, y| {
        image::Rgb([
            ((x * 37 + y * 13) % 256) as u8,
            ((x * 7 + y * 71) % 256) as u8,
            ((x * x + y * y) % 256) as u8,
        ])
    });
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}

pub(super) async fn avatar_upload_request(
    app: &Router,
    cookie: &str,
    csrf: &str,
    data: &[u8],
    extra: bool,
) -> Response {
    let mut body=format!("--boundary\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{csrf}\r\n--boundary\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"../../avatar.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    body.extend(data);
    if extra {
        body.extend(
            b"\r\n--boundary\r\nContent-Disposition: form-data; name=\"administrator_id\"\r\n\r\n1",
        );
    }
    body.extend(b"\r\n--boundary--\r\n");
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/account/avatar")
                .header(header::COOKIE, cookie)
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=boundary",
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

pub(super) async fn branding_upload_request(
    app: &Router,
    cookie: &str,
    csrf: Option<&str>,
    key: &str,
    data: &[u8],
    extra: bool,
) -> Response {
    let mut body = Vec::new();
    if let Some(csrf) = csrf {
        body.extend(
            format!(
                "--boundary\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{csrf}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend(b"--boundary\r\nContent-Disposition: form-data; name=\"image\"; filename=\"../../private.png\"\r\nContent-Type: image/png\r\n\r\n");
    body.extend(data);
    if extra {
        body.extend(
            b"\r\n--boundary\r\nContent-Disposition: form-data; name=\"image\"\r\n\r\nduplicate",
        );
    }
    body.extend(b"\r\n--boundary--\r\n");
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/settings/branding/{key}"))
                .header(header::COOKIE, cookie)
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=boundary",
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

pub(super) async fn image_request(
    app: &Router,
    path: &str,
    cookie: &str,
    method: &str,
    etag: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::IF_NONE_MATCH, etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

pub(super) fn large_image_fixture() -> Vec<u8> {
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1600, 800)
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}

pub(super) fn taxonomy_values(
    name: &str,
    slug: &str,
    parent_id: Option<i64>,
) -> talocart_commerce::Values {
    talocart_commerce::Values {
        name: name.into(),
        slug: slug.into(),
        description: "Descrizione <b>testuale</b>".into(),
        parent_id,
        sort_order: "0".into(),
        active: true,
    }
}

pub(super) async fn taxonomy_session(service: &Identity) -> talocart_platform::identity::Session {
    let raw = authenticated(service).await;
    service.session(&raw).await.unwrap().unwrap()
}

pub(super) fn tax_values(
    name: &str,
    percentage: &str,
    rate_id: Option<i64>,
) -> talocart_commerce::taxes::Values {
    talocart_commerce::taxes::Values {
        name: name.into(),
        description: "Synthetic test".into(),
        percentage: percentage.into(),
        rate_id: rate_id.map(|n| n.to_string()).unwrap_or_default(),
        active: true,
    }
}

pub(super) fn product_values(model: &str, slug: &str) -> talocart_commerce::products::Values {
    talocart_commerce::products::Values {
        name: "Prodotto demo".into(),
        model: model.into(),
        slug: slug.into(),
        price: "12,34".into(),
        stock: "5".into(),
        ..talocart_commerce::products::Values::default()
    }
}

pub(super) async fn product_references(
    pool: &PgPool,
    session: &talocart_platform::identity::Session,
) -> (i64, i64, i64) {
    let identity = Identity::new(pool.clone());
    let catalog =
        talocart_commerce::Catalog::new(pool.clone(), identity.clone(), test_media(pool).await);
    let category = catalog
        .save(
            session,
            &session.csrf,
            talocart_commerce::Kind::Category,
            None,
            &taxonomy_values("Categoria prodotti", "categoria-prodotti", None),
        )
        .await
        .unwrap();
    let brand = catalog
        .save(
            session,
            &session.csrf,
            talocart_commerce::Kind::Brand,
            None,
            &taxonomy_values("Marchio prodotti", "marchio-prodotti", None),
        )
        .await
        .unwrap();
    let taxes = talocart_commerce::taxes::Taxes::new(pool.clone(), identity);
    let rate = taxes
        .save(
            session,
            &session.csrf,
            talocart_commerce::taxes::Kind::Rate,
            None,
            &tax_values("Demo 50", "50", None),
            taxes.revision().await.unwrap(),
        )
        .await
        .unwrap();
    let class = taxes
        .save(
            session,
            &session.csrf,
            talocart_commerce::taxes::Kind::Class,
            None,
            &tax_values("Classe prodotti", "", Some(rate)),
            taxes.revision().await.unwrap(),
        )
        .await
        .unwrap();
    (category, brand, class)
}

pub(super) async fn variant_fixture(
    pool: &PgPool,
) -> (
    Identity,
    talocart_platform::identity::Session,
    talocart_commerce::products::Products,
    talocart_commerce::variants::Variants,
    i64,
    [i64; 2],
    [i64; 4],
) {
    let identity = installed(pool).await;
    let session = taxonomy_session(&identity).await;
    let media = test_media(pool).await;
    let products =
        talocart_commerce::products::Products::new(pool.clone(), identity.clone(), media.clone());
    let variants =
        talocart_commerce::variants::Variants::new(pool.clone(), identity.clone(), media);
    let (category, brand, class) = product_references(pool, &session).await;
    let mut p = product_values("MODEL", "model");
    p.status = "published".into();
    p.category_id = category.to_string();
    p.brand_id = brand.to_string();
    p.tax_class_id = class.to_string();
    let product = products
        .save(&session, &session.csrf, None, &p)
        .await
        .unwrap();
    let color = variants
        .save_option(&session, &session.csrf, None, "Colore")
        .await
        .unwrap();
    let size = variants
        .save_option(&session, &session.csrf, None, "Taglia")
        .await
        .unwrap();
    let mut ids = [0; 4];
    for (i, (option, name)) in [(color, "Rosso"), (color, "Blu"), (size, "S"), (size, "M")]
        .iter()
        .enumerate()
    {
        ids[i] = variants
            .save_value(&session, &session.csrf, *option, None, name)
            .await
            .unwrap();
    }
    (
        identity,
        session,
        products,
        variants,
        product,
        [color, size],
        ids,
    )
}

pub(super) fn variant_values(
    sku: &str,
    ids: &[i64],
    price: &str,
    stock: &str,
) -> talocart_commerce::variants::Values {
    talocart_commerce::variants::Values {
        stock_revision: 0,
        sku: sku.into(),
        price: price.into(),
        price_mode: "inclusive".into(),
        stock: stock.into(),
        active: true,
        value_ids: ids.into(),
    }
}

pub(super) async fn demo_existing_snapshot(pool: &PgPool) -> Vec<String> {
    let mut result = Vec::new();
    for (table, order) in [
        ("catalog_products", "id"),
        ("catalog_categories", "id"),
        ("catalog_brands", "id"),
        ("tax_classes", "id"),
        ("tax_rates", "id"),
        ("catalog_options", "id"),
        ("catalog_option_values", "id"),
        ("catalog_variants", "id"),
        ("catalog_product_options", "product_id,position"),
        ("catalog_variant_values", "variant_id,value_id"),
        ("pricing_settings", "id"),
        ("store_settings", "id"),
    ] {
        result.push(
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT coalesce(json_agg(t ORDER BY {order})::text,'[]') FROM {table} t"
            )))
            .fetch_one(pool)
            .await
            .unwrap(),
        );
    }
    result
}

// Public image aliases and size fallbacks must redirect without caching their mutable destination.
pub(super) async fn image_follow(
    app: &Router,
    path: &str,
    cookie: &str,
    method: &str,
    etag: &str,
) -> Response {
    let mut path = path.to_owned();
    for _ in 0..4 {
        let response = image_request(app, &path, cookie, method, etag).await;
        if response.status() != StatusCode::TEMPORARY_REDIRECT {
            return response;
        }
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        path = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(
            path.starts_with("/media/"),
            "An image alias must stay within local media routes"
        );
    }
    panic!("Unexpected image redirect cycle")
}

// Exercise explicit trash/purge rather than deleting a library registration directly.
pub(super) async fn purge_library_sources(
    pool: &PgPool,
    session: &talocart_platform::identity::Session,
    keys: &[String],
) {
    let media = test_media(pool).await;
    let identity = test_identity(pool.clone()).await;
    let library =
        talocart_platform::media::library::MediaLibrary::new(pool.clone(), identity, media);
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT m.id FROM media_library l JOIN media_objects m ON m.key=l.key WHERE l.key=ANY($1)",
    )
    .bind(keys)
    .fetch_all(pool)
    .await
    .unwrap();
    for id in ids {
        library
            .trash(session, &session.csrf, id, false, false, true)
            .await
            .unwrap();
    }
    library
        .purge(session, &session.csrf, false, true)
        .await
        .unwrap();
}

// Form submission helper for existing integration scenarios. Raw request() remains
// available for stale/missing revision assertions; no production fallback exists.
pub(super) async fn fiscal_request(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    body: &str,
    partial: bool,
) -> Response {
    if method == "POST"
        && (path.starts_with("/admin/settings/taxes") || path == "/admin/settings/countries")
        && !path.ends_with("/preview")
        && !body.contains("revision=")
    {
        let get_path = path.strip_suffix("/delete").unwrap_or(path);
        let page = html(request(app, "GET", get_path, cookie, "", false).await).await;
        let revision = page
            .split("name=\"fiscal_revision\" value=\"")
            .nth(1)
            .or_else(|| page.split("name=\"revision\" value=\"").nth(1))
            .and_then(|s| s.split('"').next())
            .unwrap_or("-1");
        return request(
            app,
            method,
            path,
            cookie,
            &format!("{body}&revision={revision}"),
            partial,
        )
        .await;
    }
    request(app, method, path, cookie, body, partial).await
}

/// Explicit zero-rated fixture for tests concerned with tariff arithmetic.
pub(super) async fn zero_charge_class(pool: &PgPool) {
    sqlx::query("INSERT INTO tax_rates(name,basis_points) VALUES('Charge zero',0)")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tax_classes(name,rate_id) VALUES('Charge zero',1)")
        .execute(pool)
        .await
        .unwrap();
}
