//! Explicit route composition; middleware order is part of the HTTP contract.
use super::*;

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .merge(media::routes())
        .merge(mail::routes())
        .merge(maintenance::routes())
        .merge(payments::routes())
        .merge(shipping::routes())
        .merge(order_statuses::routes())
        .merge(customer_groups::routes())
        .merge(admin_customers::routes())
        .merge(customer_fields::routes())
        .merge(coupons::routes())
        .merge(pages::admin_routes())
        .merge(banners::admin_routes())
        .merge(orders::routes())
        .merge(catalog::routes())
        .merge(
            Router::new()
                .merge(taxes::routes())
                .merge(tax_zones::routes())
                .merge(checkout_countries::routes())
                .layer(middleware::from_fn_with_state(
                    state.clone(),
                    fiscal_forms::snapshot,
                )),
        )
        .merge(products::routes())
        .merge(
            state
                .extensions
                .iter()
                .filter(|extension| extension.enabled)
                .fold(Router::new(), |routes, extension| {
                    routes.merge(extension.routes.clone())
                }),
        )
        .merge(variants::routes())
        .merge(attributes::routes())
        .route("/admin", get(admin))
        .route("/admin/operations", get(operations::index))
        .route("/admin/error-logs", get(error_logs::index))
        .route("/admin/updates", get(updates::index).post(updates::install))
        .route("/admin/updates/check", axum::routing::post(updates::check))
        .route("/admin/components", get(ui::page))
        .route("/admin/access", get(no_access))
        .route("/admin/account", get(account::page).post(account::update))
        .route(
            "/admin/account/language",
            axum::routing::post(account::language),
        )
        .route(
            "/admin/account/password",
            axum::routing::post(account::password),
        )
        .route(
            "/admin/account/avatar",
            get(account::avatar)
                .post(account::upload)
                .layer(DefaultBodyLimit::max(
                    talocart_platform::identity::AVATAR_UPLOAD_LIMIT + 16 * 1024,
                )),
        )
        .route(
            "/admin/account/avatar/delete",
            axum::routing::post(account::remove_avatar),
        )
        .route("/admin/roles", get(roles::index))
        .route("/admin/roles/new", get(roles::new_page).post(roles::create))
        .route(
            "/admin/roles/{key}",
            get(roles::edit_page).post(roles::update),
        )
        .route(
            "/admin/roles/{key}/delete",
            axum::routing::post(roles::delete),
        )
        .route("/admin/modules", get(modules::index))
        .merge(packages::routes())
        .route(
            "/admin/settings",
            get(unified_settings::page).post(administration::settings),
        )
        .route(
            "/admin/settings/save",
            axum::routing::post(unified_settings::save),
        )
        .route(
            "/admin/settings/date-time",
            axum::routing::post(administration::regional_settings),
        )
        .merge(storefront::admin_routes())
        .route("/admin/settings/appearance", get(branding::page))
        .route(
            "/admin/settings/branding/{key}",
            axum::routing::post(branding::upload).layer(DefaultBodyLimit::max(
                talocart_platform::identity::BRANDING_UPLOAD_LIMIT + 16 * 1024,
            )),
        )
        .route(
            "/admin/settings/branding/{key}/delete",
            axum::routing::post(branding::remove),
        )
        .route("/admin/users", get(administration::users_page))
        .route(
            "/admin/users/new",
            get(administration::new_user_page).post(administration::create_user),
        )
        .route(
            "/admin/users/{id}",
            get(administration::edit_user_page).post(administration::update_user),
        )
        .route("/admin/logout", axum::routing::post(access::logout))
        .route("/admin/status", get(admin_status))
        .route_layer(middleware::from_fn(access::require_admin));
    let routes = Router::new()
        .merge(protected)
        .merge(state.payment_assets.clone())
        .merge(crate::payment_extensions::native_assets())
        .merge(orders::webhook_routes(&state))
        .merge(storefront::public_routes())
        .route("/setup", get(access::setup_page).post(access::setup))
        .route("/admin/login", get(access::login_page).post(access::login))
        .route("/admin/reset", get(access::reset_page).post(access::reset))
        .route(
            "/admin/language",
            axum::routing::post(i18n::choose_language),
        )
        .route("/branding/{key}", get(branding::image))
        .route("/media/public/{id}/{*path}", get(branding::public_image))
        .route("/media/image/{id}/{profile}", get(media::public_alias))
        .route("/maintenance/status", get(maintenance::status))
        .route("/health/live", get(|| async { "alive\n" }))
        .route("/health/ready", get(ready))
        .route(
            "/favicon.svg",
            asset(
                "image/svg+xml",
                include_bytes!("../../../assets/favicon.svg"),
            ),
        )
        .route(
            "/assets/themes/essential.css",
            asset(
                "text/css; charset=utf-8",
                include_bytes!("../../../themes/essential/assets/store.css"),
            ),
        )
        .route(
            "/assets/themes/default.css",
            asset(
                "text/css; charset=utf-8",
                include_bytes!("../../../themes/default/assets/store.css"),
            ),
        )
        .route(
            "/assets/base.css",
            asset(
                "text/css; charset=utf-8",
                include_bytes!("../../../assets/base.css"),
            ),
        )
        .route(
            "/assets/rich-text.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/vendor/tiptap/rich-text.min.js"),
            ),
        )
        .route(
            "/assets/banners.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/banners.js"),
            ),
        )
        .route(
            "/assets/ui-toasts.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/ui-toasts.js"),
            ),
        )
        .route(
            "/assets/ui-tabs.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/ui-tabs.js"),
            ),
        )
        .route(
            "/assets/ui.css",
            asset(
                "text/css; charset=utf-8",
                include_bytes!("../../../assets/ui.css"),
            ),
        )
        .route(
            "/assets/catalog-pagination.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/catalog-pagination.js"),
            ),
        )
        .route(
            "/assets/product-gallery.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/product-gallery.js"),
            ),
        )
        .route(
            "/assets/product-price.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/product-price.js"),
            ),
        )
        .route(
            "/assets/minicart.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/minicart.js"),
            ),
        )
        .route(
            "/assets/store-language.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/store-language.js"),
            ),
        )
        .route(
            "/assets/settings.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/admin/settings.js"),
            ),
        )
        .route(
            "/assets/media.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/media.js"),
            ),
        )
        .route(
            "/assets/admin.css",
            asset(
                "text/css; charset=utf-8",
                include_bytes!("../../../assets/admin/admin.css"),
            ),
        )
        .route(
            "/assets/category-menu.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/category-menu.js"),
            ),
        )
        .route(
            "/assets/ui-table.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/ui-table.js"),
            ),
        )
        .route(
            "/assets/product-pagination.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/admin/product-pagination.js"),
            ),
        )
        .route(
            "/assets/customer-admin.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/customer-admin.js"),
            ),
        )
        .route(
            "/assets/customer-field-policy.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/customer-field-policy.js"),
            ),
        )
        .route(
            "/assets/checkout-details.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/checkout-details.js"),
            ),
        )
        .route(
            "/assets/payment-status.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/payment-status.js"),
            ),
        )
        .route(
            "/assets/order-receipt.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/order-receipt.js"),
            ),
        )
        .route(
            "/assets/checkout-methods.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/checkout-methods.js"),
            ),
        )
        .route(
            "/assets/product-selection.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/admin/product-selection.js"),
            ),
        )
        .route(
            "/assets/product-bulk.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/admin/product-bulk.js"),
            ),
        )
        .route(
            "/assets/product-tabs.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/admin/product-tabs.js"),
            ),
        )
        .route(
            "/assets/order-editor.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/admin/order-editor.js"),
            ),
        )
        .route(
            "/assets/htmx-2.0.11.min.js",
            asset(
                "text/javascript; charset=utf-8",
                include_bytes!("../../../assets/vendor/htmx-2.0.11.min.js"),
            ),
        )
        .fallback(packages::fallback);
    let public = contextual(public_routes("").merge(store_language::routes()), &state)
        .route_layer(Extension(PublicRoute));
    Router::new()
        .merge(contextual(routes, &state))
        .merge(public)
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(state.clone())
        .layer(middleware::from_fn(request_timeout))
        .layer(middleware::from_fn(security_headers))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<Body>| {
            let request_id = request.headers().get("x-request-id").and_then(|id| id.to_str().ok()).unwrap_or("unknown");
            tracing::info_span!("http", request_id, method = %request.method(), path = trace_path(request.uri().path()))
        }).on_response(tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO)))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

/// Installed by the public route group before language/customer middleware.
/// URL-shape helpers remain presentation helpers, never request capabilities.
#[derive(Clone, Copy)]
pub(super) struct PublicRoute;

fn contextual(routes: Router<AppState>, state: &AppState) -> Router<AppState> {
    routes
        .layer(middleware::from_fn_with_state(
            state.clone(),
            error_pages::context,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            i18n::language_context,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            extension_context::context,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            maintenance::gate,
        ))
}

/// One declaration for the primary URLs and every language prefix.
pub(super) fn public_routes(prefix: &str) -> Router<AppState> {
    let routes = Router::new()
        .merge(cart::routes(prefix))
        .merge(customers::routes(prefix))
        .merge(checkout_details::routes(prefix))
        .route(&format!("{prefix}/"), get(store))
        .route(&format!("{prefix}/pages/{{slug}}"), get(pages::public))
        .route(&format!("{prefix}/catalog"), get(products::public_index))
        .route(
            &format!("{prefix}/catalog/{{slug}}"),
            get(products::public_category_index),
        )
        .route(&format!("{prefix}/search"), get(products::search::submit))
        .route(
            &format!("{prefix}/search/{{*term}}"),
            get(products::search::page),
        )
        .route(&format!("{prefix}/brands"), get(brands::index))
        .route(
            &format!("{prefix}/brands/{{slug}}"),
            get(products::public_brand_index),
        )
        .route(
            &format!("{prefix}/products/{{slug}}"),
            get(products::public_detail).post(products::public_price),
        )
        .route(&format!("{prefix}/status"), get(store_status));
    if prefix.is_empty() {
        routes
    } else {
        routes.route(prefix, get(store))
    }
}

#[derive(Default, serde::Deserialize)]
pub(super) struct ReadyQuery {
    #[serde(default)]
    all_modules: bool,
}
pub(super) async fn ready(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<ReadyQuery>,
) -> Response {
    if !state
        .runtime_health
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return (StatusCode::SERVICE_UNAVAILABLE, "not ready\n").into_response();
    }
    if state.packages.as_ref().is_some_and(|manager| {
        !if query.all_modules {
            manager.runtime_ready()
        } else {
            manager.store_ready()
        }
        .unwrap_or(false)
    }) {
        return (StatusCode::SERVICE_UNAVAILABLE, "not ready\n").into_response();
    }
    match state.check_database().await {
        Ok(()) => (StatusCode::OK, "ready\n").into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "not ready\n").into_response(),
    }
}
