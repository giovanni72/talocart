use super::*;
use axum::extract::Query;
use serde::Deserialize;
#[derive(Deserialize)]
pub struct Filters {
    #[serde(default)]
    area: String,
    #[serde(default = "first")]
    page: i64,
}
fn first() -> i64 {
    1
}
struct Row {
    id: i64,
    date: String,
    resource: String,
    key: String,
    action: String,
    actor: String,
}
struct Area {
    key: &'static str,
    label: String,
}
#[derive(Template)]
#[template(path = "operations.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    rows: Vec<Row>,
    areas: Vec<Area>,
    area: String,
    pagination: pagination::Pagination,
}
pub async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<Filters>,
) -> Result<Response, access::IdentityErrorResponse> {
    let result = talocart_platform::operations::list(
        &state.pool,
        &state.identity,
        &session,
        &q.area,
        q.page,
    )
    .await?;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    let title = i18n::text("ops-title");
    let pagination =
        pagination::Pagination::new(result.total, result.page, 20, &title, &title, |p| {
            format!("/admin/operations?area={}&page={p}", q.area)
        });
    let rows = result
        .entries
        .into_iter()
        .map(|event| Row {
            id: event.id,
            date: event.created_at,
            resource: i18n::text(&format!(
                "ops-resource-{}",
                event.resource.replace('_', "-")
            )),
            key: event.resource_key,
            action: i18n::text(match event.action.as_str() {
                "INSERT" => "ops-insert",
                "DELETE" => "ops-delete",
                _ => "ops-update",
            }),
            actor: event
                .administrator_id
                .map(|id| format!("{} #{id}", i18n::text("ops-actor")))
                .unwrap_or_else(|| i18n::text("ops-unattributed")),
        })
        .collect();
    Ok(render(Page {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "operations",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        rows,
        areas: talocart_platform::operations::AREAS
            .into_iter()
            .map(|key| Area {
                key,
                label: i18n::text(&format!("ops-{key}")),
            })
            .collect(),
        area: q.area,
        pagination,
    }))
}
