mod access;
mod account;
mod administration;
mod branding;
mod brands;
mod cart;
mod catalog;
mod error_logs;
mod error_pages;
mod i18n;
mod images;
mod operations;
mod updates;
pub use i18n::validate_catalogues;
mod admin_customers;
mod attributes;
mod banners;
mod checkout_countries;
mod checkout_details;
mod content;
mod coupons;
mod customer_fields;
mod customer_groups;
mod customers;
mod extension_context;
pub mod extensions;
mod fiscal_forms;
mod mail;
mod maintenance;
mod media;
mod media_picker;
mod modules;
mod navigation;
mod order_statuses;
mod orders;
mod orders_editor;
mod packages;
mod page_preference;
mod pages;
mod pagination;
pub mod payment_extensions;
mod payments;
mod product_card;
mod products;
mod roles;
mod shipping;
mod store_language;
mod store_presentation;
mod storefront;
mod table_sort;
mod tax_coverage;
mod tax_zones;
mod taxes;
#[cfg(test)]
mod test_templates;
mod ui;
mod unified_settings;
mod variants;

use std::{sync::Arc, time::Duration};

use askama::Template;
use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, Extension, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use sqlx::PgPool;
use talocart_platform::{
    database,
    identity::{Identity, Permission, Session},
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

mod admin_shell;
mod home;
mod home_sales;
mod http_policy;
mod indexing;
mod routing;
mod state;
use admin_shell::landing_path;
pub use admin_shell::{AccountMenu, AdminNavigation, AdminShell};
use home::*;
pub use home::{current_logo, current_name, render};
use http_policy::{asset, request_timeout, security_headers, trace_path};
pub use routing::router;
pub use state::{AppState, AppStateConfig};

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn static_assets_revalidate_without_stale_bodies() {
        use tower::ServiceExt;
        let app = Router::new()
            .route("/asset", asset("text/css", b"body{}"))
            .layer(middleware::from_fn(security_headers));
        let first = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/asset")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first
            .headers()
            .get(header::ETAG)
            .expect("static assets need a content validator")
            .clone();
        for value in [
            etag.to_str().unwrap().to_owned(),
            format!("W/{}", etag.to_str().unwrap()),
            format!("\"other\", {}", etag.to_str().unwrap()),
            "*".into(),
        ] {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/asset")
                        .header(header::IF_NONE_MATCH, value)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
            assert_eq!(response.headers()[header::ETAG], etag);
            assert!(
                response
                    .headers()
                    .contains_key(header::CONTENT_SECURITY_POLICY)
            );
            assert!(
                axum::body::to_bytes(response.into_body(), 100)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        let head = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("HEAD")
                    .uri("/asset")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::ETAG], etag);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "6");
        assert!(
            axum::body::to_bytes(head.into_body(), 100)
                .await
                .unwrap()
                .is_empty()
        );
        let changed = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/asset")
                    .header(header::IF_NONE_MATCH, "\"stale\"")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(changed.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(changed.into_body(), 100)
                .await
                .unwrap(),
            "body{}"
        );
        let rebuilt = Router::new()
            .route("/asset", asset("text/css", b"changed"))
            .oneshot(
                axum::http::Request::builder()
                    .uri("/asset")
                    .header(header::IF_NONE_MATCH, &etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rebuilt.status(), StatusCode::OK);
        assert_ne!(rebuilt.headers().get(header::ETAG), Some(&etag));
    }
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    fn app() -> Router {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        router(AppState::configured(
            pool,
            "<script>alert(1)</script>".into(),
            Default::default(),
        ))
    }

    async fn body(response: Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), 200_000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn search_terms_are_not_recorded_in_request_spans() {
        assert_eq!(trace_path("/search/nome-personale"), "/search/{term}");
        assert_eq!(trace_path("/search/secret%2Fterm"), "/search/{term}");
        assert_eq!(
            trace_path("/en/search/secret%2Fterm"),
            "/{locale}/search/{term}"
        );
        assert_eq!(trace_path("/catalog/accessori"), "/catalog/accessori");
    }
    #[tokio::test]
    async fn pages_escape_store_name_and_only_admin_loads_admin_css() {
        let name = "<script>alert(1)</script>";
        let store = body(render(StorePage {
            search_query: "",
            search_action: "/search",
            search_filters: navigation::SearchFilters::default(),
            show_status: false,
            theme: talocart_platform::storefront::Settings::default(),
            meta_title: name.into(),
            meta_description: String::new(),
            canonical: "http://127.0.0.1:3000/".into(),
            featured: vec![],
            banners: vec![],
            category_menu: navigation::CategoryMenu::default(),
            store_name: name,
            logo: None,
            panel: StatusPanel::pending("/status"),
        }))
        .await;
        let admin = body(render(AdminPage {
            nav: AdminNavigation {
                error_logs: false,
                updates: false,
                application: talocart_platform::version::ApplicationIdentity::current(),
                operations: true,
                pages: false,
                customers: false,
                orders: false,
                media: true,
                overview: true,
                settings: true,
                users: true,
                roles: true,
                categories: true,
                brands: true,
                taxes: true,
                products: true,
                extensions: Vec::new(),
            },
            theme: talocart_platform::storefront::Settings::default(),
            account: AccountMenu::example(),
            section: "overview",
            store_name: name,
            logo: None,
            panel: StatusPanel::pending("/admin/status"),
            show_status: false,
            csrf: "test",
            groups: vec![],
            sales: None,
        }))
        .await;
        for (html, is_admin) in [(store, false), (admin, true)] {
            assert!(!html.contains(name));
            assert!(html.contains("&#60;script&#62;"));
            assert_eq!(html.contains("/assets/admin.css"), is_admin);
            if is_admin {
                assert!(html.contains("Il mio account"));
                assert!(!html.contains("Fase 1.2"));
                assert!(!html.contains("id=\"status-result\""));
                let (sidebar, content) = html.split_once("</nav>").unwrap();
                let last_link = sidebar.rsplit("<a ").next().unwrap();
                assert!(last_link.contains("href=\"/admin/components\""));
                assert!(!content.contains("href=\"/admin/components\""));
                assert!(!content.contains("href=\"/admin/operations\""));
                assert!(sidebar.contains("href=\"/admin/operations\""));
            } else {
                assert!(html.contains("href=\"/catalog\""));
            }
        }
    }

    #[tokio::test]
    async fn backend_brand_links_to_the_overview_even_without_overview_permission() {
        for (locale, label) in [
            (talocart_platform::locale::Locale::ITALIAN, "Versione"),
            (talocart_platform::locale::Locale::ENGLISH, "Version"),
        ] {
            for overview in [false, true] {
                let html = i18n::scope(locale, async {
                    body(render(NoAccessPage {
                        store_name: "Test shop",
                        logo: None,
                        csrf: "test",
                        section: "account",
                        theme: talocart_platform::storefront::Settings::default(),
                        account: AccountMenu::example(),
                        nav: AdminNavigation {
                            error_logs: false,
                            updates: false,
                            application: talocart_platform::version::ApplicationIdentity::current(),
                            operations: false,
                            pages: false,
                            customers: false,
                            orders: false,
                            media: true,
                            overview,
                            settings: false,
                            users: false,
                            roles: false,
                            categories: false,
                            brands: false,
                            taxes: false,
                            products: false,
                            extensions: Vec::new(),
                        },
                    }))
                    .await
                })
                .await;
                let (_, footer) = html
                    .split_once("<div class=\"sidebar-bottom\">")
                    .expect("Shared sidebar footer");
                let footer = footer.split("</aside>").next().expect("Sidebar end");
                assert!(footer.contains(&format!(
                    "{label} <span>{}</span>",
                    talocart_platform::version::DISPLAY
                )));
                assert!(footer.contains(&format!(
                    "data-build-version=\"{}\"",
                    talocart_platform::version::INFO.version
                )));
                assert!(!footer.contains("<script"));
                assert!(html.contains("class=\"wordmark\" href=\"/admin\""));
                assert!(!html.contains("class=\"wordmark\" href=\"/\""));
                assert_eq!(html.contains("href=\"/admin/components\""), overview);
            }
        }
    }

    #[tokio::test]
    async fn module_sidebar_shows_host_application_identity_in_both_languages() {
        for (locale, label) in [
            (talocart_platform::locale::Locale::ITALIAN, "Versione"),
            (talocart_platform::locale::Locale::ENGLISH, "Version"),
        ] {
            for overview in [false, true] {
                let html = i18n::scope(locale, async {
                    body(render(NoAccessPage {
                        store_name: "Test shop",
                        logo: None,
                        csrf: "test",
                        section: "account",
                        theme: talocart_platform::storefront::Settings::default(),
                        account: AccountMenu::example(),
                        nav: AdminNavigation {
                            error_logs: false,
                            updates: false,
                            application: talocart_platform::version::ApplicationIdentity {
                                version: "99.1.0".into(),
                                display: "99.1.0+future-host".into(),
                            },
                            operations: false,
                            pages: false,
                            customers: false,
                            orders: false,
                            media: true,
                            overview,
                            settings: false,
                            users: false,
                            roles: false,
                            categories: false,
                            brands: false,
                            taxes: false,
                            products: false,
                            extensions: Vec::new(),
                        },
                    }))
                    .await
                })
                .await;
                let (_, footer) = html
                    .split_once("<div class=\"sidebar-bottom\">")
                    .expect("Shared sidebar footer");
                let footer = footer.split("</aside>").next().expect("Sidebar end");
                assert!(footer.contains(&format!("{label} <span>{}</span>", "99.1.0+future-host")));
                assert!(footer.contains(&format!("data-build-version=\"{}\"", "99.1.0")));
                assert!(!footer.contains("<script"));
                assert!(html.contains("class=\"wordmark\" href=\"/admin\""));
                assert!(!html.contains("class=\"wordmark\" href=\"/\""));
                assert_eq!(html.contains("href=\"/admin/components\""), overview);
            }
        }
    }

    #[tokio::test]
    async fn users_sidebar_groups_links_by_permissions_and_opens_current_area() {
        for (users, roles) in [(false, false), (true, false), (false, true), (true, true)] {
            for section in ["overview", "users", "roles"] {
                let html = body(render(NoAccessPage {
                    store_name: "Test shop",
                    logo: None,
                    csrf: "test",
                    section,
                    theme: talocart_platform::storefront::Settings::default(),
                    account: AccountMenu::example(),
                    nav: AdminNavigation {
                        error_logs: false,
                        updates: false,
                        application: talocart_platform::version::ApplicationIdentity::current(),
                        operations: false,
                        pages: false,
                        customers: false,
                        orders: false,
                        media: true,
                        overview: true,
                        settings: true,
                        users,
                        roles,
                        categories: false,
                        brands: false,
                        taxes: false,
                        products: false,
                        extensions: Vec::new(),
                    },
                }))
                .await;
                assert_eq!(
                    html.contains("<span>Utenti</span></summary>"),
                    users || roles
                );
                assert_eq!(html.contains(">Lista utenti</span></a>"), users);
                assert_eq!(html.contains(">Ruoli</span></a>"), roles);
                assert_eq!(html.contains("href=\"/admin/users\""), users);
                assert_eq!(html.contains("href=\"/admin/roles\""), roles);
                assert!(!html.contains("Anteprima negozio"));
                if users || roles {
                    let (before, children) =
                        html.split_once("<span>Utenti</span></summary>").unwrap();
                    let opening = before.rsplit("<details ").next().unwrap();
                    let children = children.split("</details>").next().unwrap();
                    assert_eq!(opening.contains(" open"), section != "overview");
                    assert_eq!(children.contains(">Lista utenti</span></a>"), users);
                    assert_eq!(children.contains(">Ruoli</span></a>"), roles);
                    assert_eq!(
                        children.contains("aria-current=\"page\""),
                        (section == "users" && users) || (section == "roles" && roles)
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn settings_sidebar_groups_resources_and_highlights_general_tabs() {
        for (settings, taxes, overview) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (false, false, true),
            (true, true, true),
        ] {
            for section in [
                "overview",
                "settings",
                "tax_rates",
                "tax_classes",
                "modules",
            ] {
                let html = body(render(NoAccessPage {
                    store_name: "Test shop",
                    logo: None,
                    csrf: "test",
                    section,
                    theme: talocart_platform::storefront::Settings::default(),
                    account: AccountMenu::example(),
                    nav: AdminNavigation {
                        error_logs: false,
                        updates: false,
                        application: talocart_platform::version::ApplicationIdentity::current(),
                        operations: false,
                        pages: false,
                        customers: false,
                        orders: false,
                        media: true,
                        overview,
                        settings,
                        users: false,
                        roles: false,
                        categories: false,
                        brands: false,
                        taxes,
                        products: true,
                        extensions: Vec::new(),
                    },
                }))
                .await;
                assert_eq!(
                    html.contains("<span>Impostazioni</span></summary>"),
                    settings || taxes || overview
                );
                assert_eq!(html.contains(">Aliquote</span></a>"), taxes);
                assert_eq!(html.contains(">Classi fiscali</span></a>"), taxes);
                assert!(!html.contains(">Prezzi e imposte</span></a>"));
                if settings || taxes || overview {
                    assert!(
                        html.find("<span>Catalogo</span></summary>").unwrap()
                            < html.find("<span>Impostazioni</span></summary>").unwrap(),
                        "Catalogo deve precedere Impostazioni"
                    );
                    let (before, children) = html
                        .split_once("<span>Impostazioni</span></summary>")
                        .unwrap();
                    assert_eq!(
                        before.rsplit("<details ").next().unwrap().contains(" open"),
                        section != "overview"
                    );
                    let children = children.split("</details>").next().unwrap();
                    assert_eq!(children.contains(">Generali</span></a>"), settings || taxes);
                    if settings || taxes {
                        assert!(children.contains(if settings {
                            "href=\"/admin/settings\""
                        } else {
                            "href=\"/admin/settings/taxes\""
                        }));
                    }
                    assert_eq!(
                        children.contains("id=\"sidebar-modules-link\" href=\"/admin/modules\""),
                        overview
                    );
                    assert_eq!(children.contains(">Moduli</span></a>"), overview);
                    assert_eq!(
                        children.contains("aria-current=\"page\""),
                        section == "settings" && (settings || taxes)
                            || taxes && (section == "tax_rates" || section == "tax_classes")
                            || section == "modules" && overview
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn database_failure_is_reported_for_full_pages_and_fragments() {
        for path in ["/", "/status"] {
            for partial in [false, true] {
                let request = Request::builder()
                    .uri(path)
                    .header("HX-Request", if partial { "true" } else { "false" })
                    .body(Body::empty())
                    .unwrap();
                let response = app().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
                assert!(
                    response.headers()[header::VARY]
                        .to_str()
                        .unwrap()
                        .contains("HX-Request")
                );
                let html = body(response).await;
                assert!(html.contains("Servizio temporaneamente non disponibile"));
                assert!(!html.contains("alert(1)"));
                assert!(!html.contains("postgres://"));
            }
        }
    }

    #[tokio::test]
    async fn admin_requires_session_for_pages_and_fragments() {
        for path in [
            "/admin",
            "/admin/modules",
            "/admin/status",
            "/admin/settings",
            "/admin/users",
            "/admin/users/new",
            "/admin/users/1",
        ] {
            for partial in [false, true] {
                let response = app()
                    .oneshot(
                        Request::builder()
                            .uri(path)
                            .header("hx-request", if partial { "true" } else { "false" })
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    if partial {
                        StatusCode::UNAUTHORIZED
                    } else {
                        StatusCode::SEE_OTHER
                    }
                );
                assert_eq!(response.headers()[header::LOCATION], "/admin/login");
                if partial {
                    assert_eq!(response.headers()["hx-redirect"], "/admin/login");
                }
            }
        }
    }

    #[tokio::test]
    async fn draining_host_readiness_rejects_without_waiting_for_storage() {
        use std::{future::Future, task::Poll};
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused@127.0.0.1:1/unavailable_test")
            .unwrap();
        let state = AppState::configured(pool, "Synthetic".into(), Default::default())
            .with_runtime_health(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let response = routing::ready(State(state), axum::extract::Query(Default::default()));
        tokio::pin!(response);
        let result = std::future::poll_fn(|cx| Poll::Ready(response.as_mut().poll(cx))).await;
        assert!(
            matches!(result, Poll::Ready(response) if response.status() == StatusCode::SERVICE_UNAVAILABLE)
        );
    }

    #[tokio::test]
    async fn liveness_is_independent_but_readiness_requires_database() {
        for (path, expected) in [
            ("/health/live", StatusCode::OK),
            ("/health/ready", StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let response = app()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
    }

    #[tokio::test]
    async fn embedded_assets_work_without_filesystem_and_unknown_paths_return_404() {
        for path in [
            "/assets/base.css",
            "/assets/themes/default.css",
            "/assets/themes/essential.css",
            "/assets/ui.css",
            "/assets/ui-toasts.js",
            "/assets/ui-tabs.js",
            "/assets/payment-status.js",
            "/assets/order-receipt.js",
            "/assets/catalog-pagination.js",
            "/assets/admin.css",
            "/assets/product-tabs.js",
            "/assets/order-editor.js",
            "/assets/product-pagination.js",
            "/assets/ui-table.js",
            "/assets/product-selection.js",
            "/assets/product-bulk.js",
            "/assets/htmx-2.0.11.min.js",
        ] {
            let response = app()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::X_CONTENT_TYPE_OPTIONS],
                "nosniff"
            );
            assert!(!body(response).await.is_empty());
        }
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/not-found")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let document = body(response).await;
        assert!(document.contains("<footer class=\"site-footer\""));
        assert!(document.contains("href=\"/catalog\""));
        assert!(!document.contains("<script>alert(1)</script>"));
    }
}
