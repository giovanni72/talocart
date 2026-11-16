//! Home/dashboard presentation and common response rendering.
use super::*;

#[derive(Template)]
#[template(path = "no-access.html")]
pub(super) struct NoAccessPage<'a> {
    pub(super) store_name: &'a str,
    pub(super) logo: Option<talocart_platform::media::ImageMetadata>,
    pub(super) csrf: &'a str,
    pub(super) section: &'static str,
    pub(super) theme: talocart_platform::storefront::Settings,
    pub(super) nav: AdminNavigation,
    pub(super) account: AccountMenu,
}
pub(super) async fn no_access(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, access::IdentityErrorResponse> {
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    Ok(render(NoAccessPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "access",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
    }))
}
#[derive(Template)]
#[template(path = "default/templates/store.html")]
pub(super) struct StorePage<'a> {
    pub(super) search_query: &'a str,
    pub(super) search_action: &'a str,
    pub(super) search_filters: navigation::SearchFilters,
    pub(super) show_status: bool,
    pub(super) theme: talocart_platform::storefront::Settings,
    pub(super) meta_title: String,
    pub(super) meta_description: String,
    pub(super) canonical: String,
    pub(super) featured: Vec<product_card::ProductCard>,
    pub(super) banners: Vec<banners::Slide>,
    pub(super) category_menu: navigation::CategoryMenu,
    pub(super) store_name: &'a str,
    pub(super) logo: Option<talocart_platform::media::ImageMetadata>,
    pub(super) panel: StatusPanel<'a>,
}

#[derive(Template)]
#[template(path = "admin.html")]
pub(super) struct AdminPage<'a> {
    pub(super) theme: talocart_platform::storefront::Settings,
    pub(super) nav: AdminNavigation,
    pub(super) account: AccountMenu,
    pub(super) section: &'static str,
    pub(super) store_name: &'a str,
    pub(super) logo: Option<talocart_platform::media::ImageMetadata>,
    pub(super) panel: StatusPanel<'a>,
    pub(super) show_status: bool,
    pub(super) csrf: &'a str,
    pub(super) groups: Vec<DashboardGroup>,
    pub(super) sales: Option<home_sales::SalesView>,
}

pub(super) struct StatusPanel<'a> {
    pub(super) endpoint: &'a str,
    pub(super) label: &'a str,
    pub(super) detail: &'a str,
    pub(super) checked_at: String,
    pub(super) tone: &'a str,
}

#[derive(Template)]
#[template(path = "status.html")]
pub(super) struct StatusFragment<'a> {
    pub(super) panel: StatusPanel<'a>,
}

impl<'a> StatusPanel<'a> {
    pub(super) fn pending(endpoint: &'a str) -> Self {
        Self {
            endpoint,
            label: "admin-ready-to-check",
            detail: "admin-check-the-connection-and-foundation-status",
            checked_at: String::new(),
            tone: "pending",
        }
    }
}

pub fn render(template: impl Template) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(_) => {
            tracing::error!("Template rendering failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                i18n::text("admin-unable-to-display-the-page"),
            )
                .into_response()
        }
    }
}

pub(super) async fn store(
    State(state): State<AppState>,
) -> Result<Response, access::IdentityErrorResponse> {
    let name = current_name(&state).await?;
    let theme = store_language::settings(&state).await?;
    let featured = storefront::featured(&state, &theme).await?;
    Ok(render(StorePage {
        search_query: "",
        search_action: "/search",
        search_filters: navigation::SearchFilters::default(),
        show_status: false,
        meta_title: theme.title(&name),
        meta_description: theme.description(),
        canonical: theme.canonical(&state.public_origin, &store_language::path("/")),
        theme,
        featured,
        banners: banners::slides(&state).await?,
        category_menu: navigation::CategoryMenu::load(&state, "", false, "", false).await?,
        store_name: &name,
        logo: current_logo(&state).await?,
        panel: StatusPanel::pending("/status"),
    }))
}
pub async fn current_logo(
    state: &AppState,
) -> Result<
    Option<talocart_platform::media::ImageMetadata>,
    talocart_platform::identity::IdentityError,
> {
    state
        .identity
        .branding_metadata(talocart_platform::identity::BrandingKind::Logo)
        .await
}
pub async fn current_name(
    state: &AppState,
) -> Result<String, talocart_platform::identity::IdentityError> {
    Ok(state
        .identity
        .settings()
        .await?
        .map(|s| s.values.name)
        .unwrap_or_else(|| state.store_name.to_string()))
}
pub(super) struct DashboardMetric {
    pub(super) label: &'static str,
    pub(super) value: i64,
    pub(super) href: &'static str,
    group: &'static str,
    /// Pending work only; totals never ask for attention.
    pub(super) attention: bool,
}
impl From<talocart_commerce::overview::Metric> for DashboardMetric {
    fn from(metric: talocart_commerce::overview::Metric) -> Self {
        use talocart_commerce::overview::MetricKind;
        let (label, href, group, pending) = match metric.kind {
            MetricKind::OrdersTotal => ("dash-orders", "/admin/orders", "orders", false),
            MetricKind::OrdersAwaitingPayment => ("dash-unpaid", "/admin/orders", "orders", true),
            MetricKind::OrdersAwaitingShipment => ("dash-fulfill", "/admin/orders", "orders", true),
            MetricKind::ProductsTotal => {
                ("dash-products", "/admin/catalog/products", "catalog", false)
            }
            MetricKind::ProductsPublished => (
                "dash-published",
                "/admin/catalog/products",
                "catalog",
                false,
            ),
            MetricKind::PagesPublished => ("dash-pages", "/admin/pages", "content", false),
        };
        Self {
            label,
            href,
            group,
            attention: pending && metric.value > 0,
            value: metric.value,
        }
    }
}
/// Overview cards by business area, in a fixed order; areas without permitted metrics are omitted.
pub(super) struct DashboardGroup {
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    pub(super) glyph: &'static str,
    pub(super) metrics: Vec<DashboardMetric>,
}
const DASHBOARD_GROUPS: [(&str, &str, &str); 4] = [
    ("orders", "dash-group-orders", "shopping-cart"),
    ("customers", "dash-group-customers", "contact-round"),
    ("catalog", "dash-group-catalog", "package"),
    ("content", "dash-group-content", "file-text"),
];
pub(super) async fn dashboard_groups(
    state: &AppState,
    session: &Session,
) -> Result<Vec<DashboardGroup>, talocart_platform::identity::IdentityError> {
    let mut metrics: Vec<DashboardMetric> =
        talocart_commerce::overview::metrics(&state.pool, &state.identity, session)
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
    if session.allows(Permission::CustomersRead) {
        let (total, recent) = talocart_platform::identity::customer_admin::customer_counts(
            &state.pool,
            &state.identity,
            session,
        )
        .await?;
        for (label, value) in [("dash-customers", total), ("dash-customers-new", recent)] {
            metrics.push(DashboardMetric {
                label,
                value,
                href: "/admin/customers",
                group: "customers",
                attention: false,
            });
        }
    }
    if session.allows(Permission::SettingsRead) {
        let value =
            talocart_platform::mail::failed_email_count(&state.pool, &state.identity, session)
                .await?;
        metrics.push(DashboardMetric {
            label: "dash-mail",
            value,
            href: "/admin/settings/email",
            group: "content",
            attention: value > 0,
        });
    }
    let mut groups: Vec<DashboardGroup> = DASHBOARD_GROUPS
        .into_iter()
        .map(|(id, label, glyph)| DashboardGroup {
            id,
            label,
            glyph,
            metrics: Vec::new(),
        })
        .collect();
    for metric in metrics {
        if let Some(group) = groups.iter_mut().find(|group| group.id == metric.group) {
            group.metrics.push(metric);
        }
    }
    groups.retain(|group| !group.metrics.is_empty());
    Ok(groups)
}
pub(super) async fn admin(
    axum::extract::Query(pairs): axum::extract::Query<Vec<(String, String)>>,
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
) -> Result<Response, access::IdentityErrorResponse> {
    let query = home_sales::SalesQuery::parse(pairs)?;
    let sales = home_sales::load(&state, &session, query).await?;
    // History misses need the whole document. Ordinary filter requests share the
    // same validation and current authorization, without loading cards or shell.
    if headers.get("hx-request").is_some_and(|v| v == "true")
        && headers
            .get("hx-history-restore-request")
            .is_none_or(|v| v != "true")
    {
        if sales.is_none() {
            return Err(talocart_platform::identity::IdentityError::Forbidden.into());
        }
        return Ok(render(home_sales::SalesFragment { sales }));
    }
    let groups = dashboard_groups(&state, &session).await?;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    Ok(render(AdminPage {
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        section: "overview",
        store_name: &name,
        logo: admin_shell.logo,
        panel: StatusPanel::pending("/admin/status"),
        show_status: false,
        csrf: admin_shell.csrf,
        groups,
        sales,
    }))
}
pub(super) async fn store_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, access::IdentityErrorResponse> {
    status_response(state, headers, None).await
}
pub(super) async fn admin_status(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
) -> Result<Response, access::IdentityErrorResponse> {
    status_response(state, headers, Some(session)).await
}

pub(super) async fn status_response(
    state: AppState,
    headers: HeaderMap,
    session: Option<Session>,
) -> Result<Response, access::IdentityErrorResponse> {
    if let Some(session) = &session {
        state
            .identity
            .authorize(session, Permission::AdminRead)
            .await?;
    }
    let is_admin = session.is_some();
    let name = current_name(&state).await?;
    let healthy = state.check_database().await.is_ok();
    let checked_at = match talocart_platform::regional::current_time(&state.pool).await {
        Ok(value) => value,
        Err(_) => i18n::text("regional-clock-unavailable"),
    };
    let panel = StatusPanel {
        endpoint: if is_admin { "/admin/status" } else { "/status" },
        label: if healthy {
            "admin-the-foundations-are-ready"
        } else {
            "admin-connection-needs-checking"
        },
        detail: if healthy {
            "admin-the-database-is-reachable-and-the-schema-is-up-to-date"
        } else {
            "admin-the-database-or-schema-is-unavailable-run-local-diagnostics"
        },
        checked_at,
        tone: if healthy { "ok" } else { "warning" },
    };
    // The header selects presentation only, never permissions or a different operation.
    Ok(
        if headers
            .get("hx-request")
            .is_some_and(|value| value == "true")
        {
            render(StatusFragment { panel })
        } else if let Some(session) = session {
            let admin_shell = AdminShell::with_settings(
                &state,
                &session,
                name,
                state.storefront.settings().await?,
            )
            .await?;
            render(AdminPage {
                theme: admin_shell.theme,
                nav: admin_shell.nav,
                account: admin_shell.account,
                section: "overview",
                store_name: &admin_shell.store_name,
                logo: admin_shell.logo,
                panel,
                show_status: true,
                csrf: admin_shell.csrf,
                groups: dashboard_groups(&state, &session).await?,
                sales: home_sales::load(&state, &session, home_sales::SalesQuery::parse(vec![])?)
                    .await?,
            })
        } else {
            let theme = store_language::settings(&state).await?;
            render(StorePage {
                search_query: "",
                search_action: "/search",
                search_filters: navigation::SearchFilters::default(),
                show_status: true,
                meta_title: theme.title(&name),
                meta_description: theme.description(),
                canonical: theme.canonical(&state.public_origin, &store_language::path("/")),
                theme,
                featured: Vec::new(),
                banners: vec![],
                category_menu: navigation::CategoryMenu::load(&state, "", false, "", false).await?,
                store_name: &name,
                logo: current_logo(&state).await?,
                panel,
            })
        },
    )
}
