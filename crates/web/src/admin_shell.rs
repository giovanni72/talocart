//! Shared administrator identity and permission-derived navigation.
use super::*;

/// Request-local presentation data shared by full administrator pages.
/// Loading uses owner services and keeps their error/authorization contracts.
/// Flat template fields remain compatible with existing Askama extensions.
pub struct AdminShell<'a> {
    pub store_name: String,
    pub logo: Option<talocart_platform::media::ImageMetadata>,
    pub theme: talocart_platform::storefront::Settings,
    pub nav: AdminNavigation,
    pub account: AccountMenu,
    pub csrf: &'a str,
}
impl<'a> AdminShell<'a> {
    pub async fn load(
        state: &AppState,
        session: &'a Session,
    ) -> Result<Self, talocart_platform::identity::IdentityError> {
        let name = current_name(state).await?;
        let logo = current_logo(state).await?;
        let theme = state.storefront.settings().await?;
        Ok(Self::from_parts(state, session, name, logo, theme))
    }
    /// Settings editors already own a validated theme snapshot; do not reload it.
    pub async fn with_settings(
        state: &AppState,
        session: &'a Session,
        store_name: String,
        theme: talocart_platform::storefront::Settings,
    ) -> Result<Self, talocart_platform::identity::IdentityError> {
        Ok(Self::from_parts(
            state,
            session,
            store_name,
            current_logo(state).await?,
            theme,
        ))
    }
    pub(crate) fn from_parts(
        state: &AppState,
        session: &'a Session,
        store_name: String,
        logo: Option<talocart_platform::media::ImageMetadata>,
        theme: talocart_platform::storefront::Settings,
    ) -> Self {
        Self {
            store_name,
            logo,
            theme,
            nav: state.admin_navigation(session),
            account: AccountMenu::new(session),
            csrf: &session.csrf,
        }
    }
}

pub struct AccountMenu {
    pub name: String,
    pub email: String,
    pub initials: String,
    pub has_avatar: bool,
    pub avatar_version: i64,
}
impl AccountMenu {
    pub fn new(session: &Session) -> Self {
        let email = session.email.clone().unwrap_or_default();
        let name = if session.display_name.is_empty() {
            email.clone()
        } else {
            session.display_name.clone()
        };
        let initials = if session.display_name.is_empty() {
            email.chars().take(2).collect::<String>().to_uppercase()
        } else {
            name.split_whitespace()
                .take(2)
                .filter_map(|part| part.chars().next())
                .collect::<String>()
                .to_uppercase()
        };
        Self {
            name,
            email,
            initials,
            has_avatar: session.has_avatar,
            avatar_version: session.avatar_version,
        }
    }
    #[cfg(test)]
    pub(super) fn example() -> Self {
        Self {
            name: "Test user".into(),
            email: "test@example.test".into(),
            initials: "TU".into(),
            has_avatar: false,
            avatar_version: 0,
        }
    }
}
#[derive(Clone)]
pub struct AdminNavigation {
    pub updates: bool,
    pub error_logs: bool,
    pub application: talocart_platform::version::ApplicationIdentity,
    pub operations: bool,
    pub pages: bool,
    pub customers: bool,
    pub orders: bool,
    pub overview: bool,
    pub settings: bool,
    pub users: bool,
    pub roles: bool,
    pub categories: bool,
    pub brands: bool,
    pub taxes: bool,
    pub products: bool,
    pub extensions: Vec<extensions::AdminLink>,
    pub media: bool,
}
impl AdminNavigation {
    pub fn module_page(&self) -> bool {
        extension_context::module_page() && self.extensions.iter().any(|link| link.is_active(""))
    }
    pub(super) fn new(session: &Session) -> Self {
        Self {
            error_logs: false,
            updates: false,
            application: talocart_platform::version::ApplicationIdentity::current(),
            operations: session.allows(Permission::OperationsRead),
            pages: session.allows(Permission::PagesRead),
            customers: session.allows(Permission::CustomersRead),
            orders: session.allows(Permission::OrdersRead),
            media: session.allows(Permission::MediaRead),
            overview: session.allows(Permission::AdminRead),
            settings: session.allows(Permission::SettingsRead),
            users: session.allows(Permission::UsersRead),
            roles: session.allows(Permission::RolesRead),
            categories: session.allows(Permission::CategoriesRead),
            brands: session.allows(Permission::BrandsRead),
            taxes: session.allows(Permission::TaxesRead),
            products: session.allows(Permission::ProductsRead),
            extensions: Vec::new(),
        }
    }
}
pub(super) fn landing_path(session: &Session) -> &'static str {
    let nav = AdminNavigation::new(session);
    if nav.overview {
        "/admin"
    } else if nav.operations {
        "/admin/operations"
    } else if nav.pages {
        "/admin/pages"
    } else if nav.orders {
        "/admin/orders"
    } else if nav.settings {
        "/admin/settings"
    } else if nav.taxes {
        "/admin/settings/taxes"
    } else if nav.users {
        "/admin/users"
    } else if nav.roles {
        "/admin/roles"
    } else if nav.products {
        "/admin/catalog/products"
    } else if nav.categories {
        "/admin/catalog/categories"
    } else if nav.brands {
        "/admin/catalog/brands"
    } else if nav.media {
        "/admin/media"
    } else {
        "/admin/access"
    }
}
