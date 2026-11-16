//! Composition state and service accessors; no HTTP handlers or SQL queries.
use super::*;

/// Final host configuration. Supply it before constructing database/media services.
/// Host, standalone and fixtures supply the complete configuration before composition.
pub struct AppStateConfig {
    pub media: talocart_platform::media::MediaStore,
    pub mail_key: Option<Arc<talocart_platform::mail::MailKey>>,
    pub payment_ui: Option<Arc<dyn crate::payment_extensions::PaymentUi>>,
    pub online: Option<talocart_commerce::orders::OnlinePayments>,
    pub packages: Option<talocart_platform::packages::Manager>,
    pub package_install: talocart_platform::config::PackageInstall,
    pub public_origin: String,
    pub secure_cookies: bool,
}
impl Default for AppStateConfig {
    fn default() -> Self {
        Self {
            media: talocart_platform::media::MediaStore::new("storage/media".into()),
            mail_key: None,
            payment_ui: None,
            online: None,
            packages: None,
            package_install: Default::default(),
            public_origin: "http://127.0.0.1:3000".into(),
            secure_cookies: false,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub(super) updates: talocart_platform::updates::Updates,
    pub(super) error_logs: Option<talocart_platform::error_logs::ErrorLogs>,
    pub(super) payment_ui: Option<Arc<dyn crate::payment_extensions::PaymentUi>>,
    pub(super) payment_assets: axum::Router<AppState>,
    pub(super) online: Option<talocart_commerce::orders::OnlinePayments>,
    pub(super) mail_key: Option<Arc<talocart_platform::mail::MailKey>>,
    pub(super) content_primary: Option<talocart_platform::locale::Locale>,
    pub(super) content_active: Vec<String>,
    pub(super) storefront: talocart_platform::storefront::Storefront,
    pub(super) public_origin: Arc<str>,
    pub(super) pool: PgPool,
    pub(super) store_name: Arc<str>,
    pub(super) identity: Identity,
    pub(super) catalog: talocart_commerce::Catalog,
    pub(super) taxes: talocart_commerce::taxes::Taxes,
    pub(super) products: talocart_commerce::products::Products,
    pub(super) variants: talocart_commerce::variants::Variants,
    pub(super) demo: talocart_commerce::demo::DemoSeeder,
    pub(super) extensions: Vec<extensions::WebExtension>,
    pub(super) packages: Option<talocart_platform::packages::Manager>,
    pub(super) package_install: talocart_platform::config::PackageInstall,
    pub(super) application: talocart_platform::version::ApplicationIdentity,
    pub(super) module_database: Option<(database::SchemaContract, database::SchemaContract)>,
    pub(super) media: talocart_platform::media::MediaStore,
    pub(super) secure_cookies: bool,
    pub(super) runtime_health: Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    pub(super) async fn panel_install(
        &self,
    ) -> Result<bool, talocart_platform::identity::IdentityError> {
        Ok(
            talocart_platform::settings::module_install(&self.pool, self.package_install).await?
                == talocart_platform::config::PackageInstall::Panel,
        )
    }

    pub fn admin_navigation(&self, session: &Session) -> AdminNavigation {
        let mut nav = AdminNavigation::new(session);
        nav.error_logs = self.error_logs.is_some() && session.is_owner();
        nav.updates = session.is_owner() && !self.is_standalone_module();
        nav.application = self.application.clone();
        nav.extensions = self
            .extensions
            .iter()
            .filter(|extension| extension.enabled)
            .filter_map(|extension| extension.menu.as_ref())
            .filter(|menu| {
                menu.permissions
                    .iter()
                    .any(|permission| session.allows(*permission))
            })
            .map(|menu| extensions::AdminLink {
                section: menu.section.into(),
                href: menu.href.into(),
                title: (menu.title)(),
                icon: menu.icon.into(),
            })
            .collect();
        if let Some(manager) = &self.packages {
            for item in manager
                .inventory()
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.enabled && p.available())
            {
                if item
                    .manifest
                    .permissions
                    .iter()
                    .filter_map(|p| Permission::parse(p).ok())
                    .any(|p| session.allows(p))
                {
                    nav.extensions.push(extensions::AdminLink {
                        section: item.manifest.id.clone(),
                        href: item.manifest.menu.clone(),
                        icon: "download".into(),
                        title: item.manifest.title.get(i18n::locale()).to_owned(),
                    });
                }
            }
        }
        if self.is_standalone_module() {
            extension_context::apply(&mut nav, session);
        }
        nav
    }

    pub(super) fn customers(&self) -> talocart_platform::identity::Customers {
        talocart_platform::identity::Customers::new(self.pool.clone(), self.identity.clone())
    }
    pub(super) fn localized(&self) -> Self {
        let mut state = self.clone();
        let language = store_language::content_language();
        state.catalog = state.catalog.localized(language);
        state.products = state.products.localized(language);
        state.variants = state.variants.localized(language);
        state
    }
    pub async fn primary_content(
        &self,
    ) -> Result<Self, talocart_platform::identity::IdentityError> {
        if self.content_primary.is_some() {
            return Ok(self.clone());
        }
        let settings = self.storefront.settings().await?;
        let primary = talocart_platform::locale::Locale::parse(&settings.primary_locale)
            .ok_or(talocart_platform::identity::IdentityError::Unavailable)?;
        let language = talocart_platform::content::Language::new(primary, primary);
        let mut state = self.clone();
        state.content_primary = Some(primary);
        state.content_active = settings.active_locales;
        state.catalog = state.catalog.localized(language);
        state.products = state.products.localized(language);
        state.variants = state.variants.localized(language);
        Ok(state)
    }
    pub(super) fn attributes(&self) -> talocart_commerce::attributes::Attributes {
        let service = talocart_commerce::attributes::Attributes::new(
            self.pool.clone(),
            self.identity.clone(),
        );
        let language = self
            .content_primary
            .map_or_else(store_language::content_language, |locale| {
                talocart_platform::content::Language::new(locale, locale)
            });
        service.localized(language)
    }
    pub(super) fn content(&self) -> talocart_commerce::content::Content {
        talocart_commerce::content::Content::new(self.pool.clone(), self.identity.clone())
    }

    /// Compose all services once with the final media root and provider settings.
    pub fn configured(pool: PgPool, store_name: String, config: AppStateConfig) -> Self {
        let identity = Identity::new(pool.clone());
        let media = config.media;
        let identity = identity.with_media(media.clone());
        Self {
            error_logs: None,
            updates: talocart_platform::updates::Updates::default(),
            payment_ui: config.payment_ui,
            payment_assets: axum::Router::new(),
            online: config.online,
            mail_key: config.mail_key,
            content_primary: None,
            content_active: vec![],
            extensions: Vec::new(),
            packages: config.packages,
            package_install: config.package_install,
            application: talocart_platform::version::ApplicationIdentity::current(),
            module_database: None,
            media: media.clone(),
            storefront: talocart_platform::storefront::Storefront::new(pool.clone()),
            public_origin: config.public_origin.into(),
            demo: talocart_commerce::demo::DemoSeeder::new(pool.clone(), media.clone()),
            catalog: talocart_commerce::Catalog::new(pool.clone(), identity.clone(), media.clone()),
            taxes: talocart_commerce::taxes::Taxes::new(pool.clone(), identity.clone()),
            variants: talocart_commerce::variants::Variants::new(
                pool.clone(),
                identity.clone(),
                media.clone(),
            ),
            products: talocart_commerce::products::Products::new(
                pool.clone(),
                identity.clone(),
                media,
            ),
            identity,
            secure_cookies: config.secure_cookies,
            runtime_health: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            pool,
            store_name: store_name.into(),
        }
    }
    /// Compose trusted provider-owned public assets before router construction.
    pub fn with_payment_assets(mut self, routes: axum::Router<AppState>) -> Self {
        self.payment_assets = routes;
        self
    }
    pub(super) fn embedded_payments(&self) -> bool {
        self.payment_ui.as_ref().is_some_and(|ui| ui.embedded())
    }
    pub(super) async fn online_providers(
        &self,
    ) -> Option<talocart_commerce::payments::OnlineProviders> {
        if self.packages.is_none() && !self.online.as_ref().is_some_and(|s| s.multiple_providers())
        {
            return None;
        }
        Some(match &self.online {
            Some(s) => s.available_providers().await,
            None => Default::default(),
        })
    }
    pub(super) async fn online_available(&self) -> bool {
        match &self.online {
            Some(service) => service.accepting().await,
            None => false,
        }
    }
    /// Composition-time registration. One entry per ID; replacing an entry also replaces its routes.
    /// Trusted application code owns route names and validates the inventory before serving.
    pub fn with_extension(mut self, extension: extensions::WebExtension) -> Self {
        self.extensions
            .retain(|existing| existing.module.id != extension.module.id);
        self.extensions.push(extension);
        self
    }
    pub fn standalone_module(
        mut self,
        host: database::SchemaContract,
        required: database::SchemaContract,
        application: talocart_platform::version::ApplicationIdentity,
    ) -> Self {
        self.application = application;
        self.module_database = Some((host, required));
        self
    }
    pub fn is_standalone_module(&self) -> bool {
        self.module_database.is_some()
    }
    pub async fn check_database(&self) -> Result<(), database::DatabaseError> {
        if let Some((host, required)) = &self.module_database {
            database::check_module(&self.pool, host, required).await?;
        } else {
            database::check(&self.pool).await?;
        }
        if let Some(manager) = &self.packages {
            manager.check_database(&self.pool).await?;
        }
        for extension in &self.extensions {
            if let Some(migrations) = extension.migrations {
                database::check_migrations(&self.pool, migrations).await?;
            }
        }
        Ok(())
    }
    /// Host-owned liveness of required background tasks; standalone defaults to ready.
    pub fn with_runtime_health(mut self, health: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.runtime_health = health;
        self
    }
    pub fn with_error_logs(mut self, logs: talocart_platform::error_logs::ErrorLogs) -> Self {
        self.error_logs = Some(logs);
        self
    }
    pub fn with_updates(mut self, updates: talocart_platform::updates::Updates) -> Self {
        self.updates = updates;
        self
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
    pub fn catalog(&self) -> &talocart_commerce::Catalog {
        &self.catalog
    }
    pub fn taxes(&self) -> &talocart_commerce::taxes::Taxes {
        &self.taxes
    }
    pub fn media(&self) -> &talocart_platform::media::MediaStore {
        &self.media
    }
    pub fn storefront(&self) -> &talocart_platform::storefront::Storefront {
        &self.storefront
    }
    pub fn module_inventory(&self) -> Vec<talocart_platform::modules::Module> {
        std::iter::once(talocart_platform::modules::BASE)
            .chain(self.extensions.iter().map(|extension| extension.module))
            .collect()
    }
}
