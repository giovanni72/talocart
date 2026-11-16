use super::taxes::TaxResponseError;
use super::*;
use axum::extract::Query;
use serde::Deserialize;
#[derive(Deserialize, Default)]
pub(super) struct Filter {
    #[serde(default)]
    q: String,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    page: usize,
}
struct Row {
    country: String,
    code: String,
    enabled: bool,
    zone: String,
    zone_id: i64,
    active: bool,
    classes: i64,
    explicit: i64,
    fallback: i64,
}
#[derive(Template)]
#[template(path = "tax-coverage.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    rows: Vec<Row>,
    q: String,
    all: bool,
    pagination: super::pagination::Pagination,
}
pub(super) async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(filter): Query<Filter>,
) -> Result<Response, TaxResponseError> {
    if filter.q.chars().count() > 80
        || filter.q.chars().any(char::is_control)
        || filter.page > 10000
    {
        return Err(talocart_commerce::taxes::TaxError::Invalid(vec![]).into());
    }
    let countries = i18n::code_choices(talocart_platform::geography::COUNTRIES, "country", "");
    let mut rows: Vec<_> = state
        .taxes
        .coverage(&session)
        .await?
        .into_iter()
        .map(|r| Row {
            country: countries
                .iter()
                .find(|c| c.code == r.country)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| r.country.clone()),
            code: r.country,
            enabled: r.enabled,
            zone: r.zone_name.unwrap_or_default(),
            zone_id: r.zone_id.unwrap_or(0),
            active: r.zone_active,
            classes: r.classes,
            explicit: r.explicit,
            fallback: r.classes - r.explicit,
        })
        .filter(|r| {
            (filter.all || r.enabled)
                && (r
                    .country
                    .to_lowercase()
                    .contains(&filter.q.trim().to_lowercase())
                    || r.code
                        .to_lowercase()
                        .contains(&filter.q.trim().to_lowercase()))
        })
        .collect();
    rows.sort_by(|a, b| a.country.cmp(&b.country));
    let total = rows.len();
    let page = filter.page.max(1).min(total.div_ceil(20).max(1));
    let rows = rows.into_iter().skip((page - 1) * 20).take(20).collect();
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    Ok(render(Page {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "tax_zones",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        rows,
        pagination: super::pagination::Pagination::new(
            total as i64,
            page as i64,
            20,
            &i18n::text("admin-coverage"),
            &i18n::text("admin-zone-pages"),
            |page| {
                format!(
                    "/admin/settings/taxes/coverage?q={}&all={}&page={page}",
                    super::catalog::encoded(&filter.q),
                    filter.all
                )
            },
        ),
        q: filter.q,
        all: filter.all,
    }))
}
