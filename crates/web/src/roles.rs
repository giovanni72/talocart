use super::*;
use access::IdentityErrorResponse;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_platform::identity::{IdentityError, RoleDefinition};

type Result<T> = std::result::Result<T, IdentityErrorResponse>;
#[derive(Deserialize, Default)]
pub struct RoleQuery {
    #[serde(default = "first_page")]
    page: i64,
    saved: Option<String>,
    deleted: Option<String>,
}
fn first_page() -> i64 {
    1
}
#[derive(Template)]
#[template(path = "roles.html")]
struct RolesPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    roles: &'a [RoleDefinition],
    pagination: super::pagination::Pagination,
    can_write: bool,
    saved: bool,
    deleted: bool,
}
pub async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<RoleQuery>,
) -> Result<Response> {
    let result = state.identity.roles_counted(&session, query.page).await?;
    let roles = result.entries;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    Ok(render(RolesPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "roles",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        roles: &roles,
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            20,
            &i18n::text("admin-roles"),
            &i18n::text("admin-role-pages"),
            |page| format!("/admin/roles?page={page}"),
        ),
        can_write: session.allows(Permission::RolesWrite),
        saved: query.saved.as_deref() == Some("1"),
        deleted: query.deleted.as_deref() == Some("1"),
    }))
}
struct MatrixRow {
    area: String,
    read_name: &'static str,
    write_name: &'static str,
    read: bool,
    write: bool,
    supports_write: bool,
    grant_read: bool,
    grant_write: bool,
}
fn matrix(role: &RoleDefinition, session: &Session) -> Vec<MatrixRow> {
    [
        ("", "operations_read", "", Permission::OperationsRead, None),
        (
            "",
            "customers_read",
            "customers_write",
            Permission::CustomersRead,
            Some(Permission::CustomersWrite),
        ),
        (
            "",
            "pages_read",
            "pages_write",
            Permission::PagesRead,
            Some(Permission::PagesWrite),
        ),
        (
            "",
            "orders_read",
            "orders_write",
            Permission::OrdersRead,
            Some(Permission::OrdersWrite),
        ),
        (
            "admin-overview",
            "overview_read",
            "",
            Permission::AdminRead,
            None,
        ),
        (
            "admin-settings-3",
            "settings_read",
            "settings_write",
            Permission::SettingsRead,
            Some(Permission::SettingsWrite),
        ),
        (
            "admin-administrators",
            "users_read",
            "users_write",
            Permission::UsersRead,
            Some(Permission::UsersWrite),
        ),
        (
            "admin-prices-and-taxes-2",
            "taxes_read",
            "taxes_write",
            Permission::TaxesRead,
            Some(Permission::TaxesWrite),
        ),
        (
            "admin-media",
            "media_read",
            "media_write",
            Permission::MediaRead,
            Some(Permission::MediaWrite),
        ),
        (
            "admin-products-2",
            "products_read",
            "products_write",
            Permission::ProductsRead,
            Some(Permission::ProductsWrite),
        ),
        (
            "admin-categories-2",
            "categories_read",
            "categories_write",
            Permission::CategoriesRead,
            Some(Permission::CategoriesWrite),
        ),
        (
            "admin-brands-2",
            "brands_read",
            "brands_write",
            Permission::BrandsRead,
            Some(Permission::BrandsWrite),
        ),
        (
            "admin-roles-and-permissions",
            "roles_read",
            "roles_write",
            Permission::RolesRead,
            Some(Permission::RolesWrite),
        ),
    ]
    .into_iter()
    .map(|(area, read_name, write_name, read, write)| MatrixRow {
        area: if read == Permission::OperationsRead {
            i18n::text("ops-title")
        } else if read == Permission::OrdersRead {
            i18n::text("ord-title")
        } else if read == Permission::CustomersRead {
            i18n::text("cg-customers")
        } else if read == Permission::PagesRead {
            i18n::text("pages-title")
        } else {
            i18n::text(area)
        },
        read_name,
        write_name,
        read: role.allows(read),
        write: write.is_some_and(|w| role.allows(w)),
        supports_write: write.is_some(),
        grant_read: session.allows(read),
        grant_write: write.is_some_and(|w| session.allows(w)),
    })
    .collect()
}
#[derive(Template)]
#[template(path = "role-form.html")]
struct RolePage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    role: &'a RoleDefinition,
    rows: Vec<MatrixRow>,
    is_new: bool,
    protected: bool,
    can_write: bool,
    error: &'a str,
}
async fn role_response(
    state: &AppState,
    session: &Session,
    role: &RoleDefinition,
    error: Option<&IdentityError>,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::RolesRead)
        .await?;
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let message = match error {
        Some(IdentityError::Invalid) => i18n::text(
            "admin-the-role-name-must-contain-1-to-80-characters-without-control-characters",
        ),
        Some(source_error) => i18n::identity_error(source_error),
        None => String::new(),
    };
    let response = render(RolePage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "roles",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        role,
        rows: matrix(role, session),
        is_new: role.key.is_empty(),
        protected: role.protected(),
        can_write: session.allows(Permission::RolesWrite)
            && !role.protected()
            && role.grantable_by(session),
        error: &message,
    });
    Ok(administration::form_response(response, error))
}
pub async fn new_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::RolesWrite)
        .await?;
    role_response(
        &state,
        &session,
        &RoleDefinition {
            key: String::new(),
            name: String::new(),
            permissions: Vec::new(),
        },
        None,
    )
    .await
}
pub async fn edit_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(key): Path<String>,
) -> Result<Response> {
    let role = state.identity.role(&session, &key).await?;
    role_response(&state, &session, &role, None).await
}
#[derive(Deserialize)]
pub struct RoleForm {
    csrf: String,
    name: String,
    #[serde(default)]
    pages_read: bool,
    #[serde(default)]
    pages_write: bool,
    #[serde(default)]
    orders_read: bool,
    #[serde(default)]
    customers_read: bool,
    #[serde(default)]
    customers_write: bool,
    #[serde(default)]
    orders_write: bool,
    #[serde(default)]
    overview_read: bool,
    #[serde(default)]
    operations_read: bool,
    #[serde(default)]
    settings_read: bool,
    #[serde(default)]
    settings_write: bool,
    #[serde(default)]
    users_read: bool,
    #[serde(default)]
    users_write: bool,
    #[serde(default)]
    roles_read: bool,
    #[serde(default)]
    roles_write: bool,
    #[serde(default)]
    categories_read: bool,
    #[serde(default)]
    categories_write: bool,
    #[serde(default)]
    brands_read: bool,
    #[serde(default)]
    brands_write: bool,
    #[serde(default)]
    taxes_read: bool,
    #[serde(default)]
    products_read: bool,
    #[serde(default)]
    products_write: bool,
    #[serde(default)]
    media_read: bool,
    #[serde(default)]
    media_write: bool,
    #[serde(default)]
    taxes_write: bool,
}
impl RoleForm {
    fn permissions(&self) -> Vec<Permission> {
        [
            (self.overview_read, Permission::AdminRead),
            (self.operations_read, Permission::OperationsRead),
            (self.pages_read, Permission::PagesRead),
            (self.pages_write, Permission::PagesWrite),
            (self.orders_read, Permission::OrdersRead),
            (self.customers_read, Permission::CustomersRead),
            (self.customers_write, Permission::CustomersWrite),
            (self.orders_write, Permission::OrdersWrite),
            (self.settings_read, Permission::SettingsRead),
            (self.settings_write, Permission::SettingsWrite),
            (self.users_read, Permission::UsersRead),
            (self.users_write, Permission::UsersWrite),
            (self.roles_read, Permission::RolesRead),
            (self.roles_write, Permission::RolesWrite),
            (self.categories_read, Permission::CategoriesRead),
            (self.categories_write, Permission::CategoriesWrite),
            (self.brands_read, Permission::BrandsRead),
            (self.brands_write, Permission::BrandsWrite),
            (self.products_read, Permission::ProductsRead),
            (self.products_write, Permission::ProductsWrite),
            (self.media_read, Permission::MediaRead),
            (self.media_write, Permission::MediaWrite),
            (self.taxes_read, Permission::TaxesRead),
            (self.taxes_write, Permission::TaxesWrite),
        ]
        .into_iter()
        .filter_map(|(selected, p)| selected.then_some(p))
        .collect()
    }
    fn submitted(&self, key: String) -> RoleDefinition {
        RoleDefinition {
            key,
            name: self.name.clone(),
            permissions: self
                .permissions()
                .iter()
                .map(|p| p.key().to_owned())
                .collect(),
        }
    }
}
pub async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<RoleForm>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::RolesWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    match state
        .identity
        .create_role(&session, &form.csrf, &form.name, &form.permissions())
        .await
    {
        Ok(_) => Ok(Redirect::to("/admin/roles?saved=1").into_response()),
        Err(error) => {
            role_response(
                &state,
                &session,
                &form.submitted(String::new()),
                Some(&error),
            )
            .await
        }
    }
}
pub async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(key): Path<String>,
    Form(form): Form<RoleForm>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::RolesWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    match state
        .identity
        .update_role(&session, &form.csrf, &key, &form.name, &form.permissions())
        .await
    {
        Ok(()) => {
            let target = match state
                .identity
                .authorize(&session, Permission::RolesRead)
                .await
            {
                Ok(_) => "/admin/roles?saved=1",
                Err(IdentityError::Forbidden) => "/admin/login",
                Err(error) => return Err(error.into()),
            };
            Ok(Redirect::to(target).into_response())
        }
        Err(error) => {
            // Never echo a submitted owner name/permissions as though the protected role changed.
            let role = if key == "owner" {
                state.identity.role(&session, &key).await?
            } else {
                form.submitted(key)
            };
            role_response(&state, &session, &role, Some(&error)).await
        }
    }
}
#[derive(Deserialize)]
pub struct DeleteForm {
    csrf: String,
}
pub async fn delete(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(key): Path<String>,
    Form(form): Form<DeleteForm>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::RolesWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    match state.identity.delete_role(&session, &form.csrf, &key).await {
        Ok(()) => Ok(Redirect::to("/admin/roles?deleted=1").into_response()),
        Err(error) => {
            let role = state.identity.role(&session, &key).await?;
            role_response(&state, &session, &role, Some(&error)).await
        }
    }
}
