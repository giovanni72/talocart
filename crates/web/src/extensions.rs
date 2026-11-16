//! Version 2 of the HTTP extension composition API, used inside native module processes
//! to build their router on the shared SDK; the production host composes none.
//! Extensions own their routes, authorization/CSRF, templates and translations.
//! Routes are merged inside the core admin authentication and request-locale middleware.
use crate::AppState;
use axum::Router;
use talocart_platform::{identity::Permission, modules::Module};
pub const API_VERSION: u32 = 2;
#[derive(Clone)]
pub struct AdminMenu {
    pub section: &'static str,
    pub href: &'static str,
    pub icon: &'static str,
    pub title: fn() -> String,
    /// The link is visible when the session has at least one permission.
    /// Handlers must independently enforce their exact permission and resource ownership.
    pub permissions: &'static [Permission],
}
#[derive(Clone)]
pub struct AdminLink {
    pub section: String,
    pub href: String,
    pub icon: String,
    pub title: String,
}
impl AdminLink {
    pub fn is_active(&self, section: &str) -> bool {
        crate::extension_context::active(&self.href, section == self.section)
    }
}
#[derive(Clone)]
pub struct WebExtension {
    pub module: Module,
    pub title: fn() -> String,
    pub description: fn() -> String,
    pub help: fn() -> String,
    pub enabled: bool,
    pub menu: Option<AdminMenu>,
    pub routes: Router<AppState>,
    /// Installed schema is checked even while the feature is disabled at runtime.
    pub migrations: Option<&'static sqlx::migrate::Migrator>,
}
pub use crate::access::IdentityErrorResponse;
pub use crate::pagination::Pagination;
pub use crate::{AccountMenu, AdminNavigation, AdminShell, current_logo, current_name, render};
/// Shared presentation primitives; the current locale belongs to the request future.
pub mod i18n {
    pub use crate::i18n::{count, decimal, identity_error, languages, locale, scope, text};
}
