use super::*;
use talocart_platform::modules::BASE;

struct PackageView {
    item: talocart_platform::packages::Installed,
    runtime: Option<talocart_platform::packages::RuntimeStatus>,
}
impl std::ops::Deref for PackageView {
    type Target = talocart_platform::packages::Installed;
    fn deref(&self) -> &Self::Target {
        &self.item
    }
}
impl PackageView {
    fn runtime_key(&self) -> &'static str {
        self.runtime.as_ref().map_or("stopped", |r| r.state.key())
    }
    fn runtime_label(&self) -> String {
        i18n::text(&format!("packages-runtime-{}", self.runtime_key()))
    }
    fn runtime_error(&self) -> String {
        self.runtime
            .as_ref()
            .and_then(|r| r.last_error.as_ref())
            .map_or_else(String::new, |e| {
                format!(
                    "{} · {}",
                    i18n::text(&format!("packages-phase-{}", e.phase.key())),
                    i18n::text(&format!("packages-cause-{}", e.cause.key()))
                )
            })
    }
    fn failures(&self) -> u8 {
        self.runtime.as_ref().map_or(0, |r| r.failures)
    }
}

struct Area {
    id: &'static str,
    title: String,
    description: String,
}

struct ExtensionView {
    id: &'static str,
    title: String,
    description: String,
    help: String,
    enabled: bool,
    dependencies: Vec<Area>,
}
fn extensions(state: &AppState) -> Vec<ExtensionView> {
    state
        .extensions
        .iter()
        .filter(|module| module.module.id != BASE.id)
        .map(|extension| {
            let module = extension.module;
            let mut dependencies = Vec::new();
            for dependency in module
                .components
                .iter()
                .flat_map(|component| component.dependencies)
            {
                if !dependencies
                    .iter()
                    .any(|area: &Area| area.id == *dependency)
                {
                    dependencies.push(Area {
                        id: dependency,
                        title: i18n::text(&format!("modules-area-{dependency}")),
                        description: String::new(),
                    });
                }
            }
            ExtensionView {
                id: module.id,
                title: (extension.title)(),
                description: (extension.description)(),
                help: (extension.help)(),
                enabled: extension.enabled,
                dependencies,
            }
        })
        .collect()
}

#[derive(Template)]
#[template(path = "modules.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    areas: Vec<Area>,
    extensions: Vec<ExtensionView>,
    owner: bool,
    packages_available: bool,
    panel_install: bool,
    packages: Vec<PackageView>,
    preview: Option<talocart_platform::packages::Preview>,
    error: String,
    locale: String,
    sdk: &'static str,
    protocol: u32,
    database_contract: u32,
    installed: usize,
    active: usize,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, access::IdentityErrorResponse> {
    state
        .identity
        .authorize(&session, Permission::AdminRead)
        .await?;
    page(&state, &session, None, String::new(), StatusCode::OK).await
}
pub(super) async fn page(
    state: &AppState,
    session: &Session,
    preview: Option<talocart_platform::packages::Preview>,
    error: String,
    status: StatusCode,
) -> Result<Response, access::IdentityErrorResponse> {
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let extensions = extensions(state);
    let packages = state
        .packages
        .as_ref()
        .and_then(|m| m.inventory().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|item| PackageView {
            runtime: state
                .packages
                .as_ref()
                .and_then(|m| m.runtime_status(&item.manifest.id).ok()),
            item,
        })
        .collect::<Vec<_>>();
    let installed = extensions.len() + packages.len();
    let active = extensions.iter().filter(|e| e.enabled).count()
        + packages.iter().filter(|p| p.enabled).count();
    let mut response = render(Page {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "modules",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        extensions,
        owner: state.identity.authorize_owner(session).await.is_ok(),
        packages_available: state.packages.is_some(),
        panel_install: state.panel_install().await?,
        packages,
        installed,
        active,
        preview,
        error,
        locale: i18n::locale().to_owned(),
        sdk: talocart_platform::packages::sdk::VERSION,
        protocol: talocart_platform::packages::protocol::VERSION,
        database_contract: talocart_platform::database::CONTRACT_VERSION,
        areas: BASE
            .components
            .iter()
            .map(|component| Area {
                id: component.id,
                title: i18n::text(&format!("modules-area-{}", component.id)),
                description: i18n::text(&format!("modules-description-{}", component.id)),
            })
            .collect(),
    });
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}
