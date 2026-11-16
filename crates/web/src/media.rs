use super::*;
use axum::{
    extract::{Form, Multipart, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use talocart_platform::media::library::{
    Asset, Filter, Folder, FolderBranch, LibraryError, MediaLibrary, Usage,
};

type Result<T> = std::result::Result<T, MediaResponseError>;
pub struct MediaResponseError(LibraryError);
impl From<LibraryError> for MediaResponseError {
    fn from(error: LibraryError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for MediaResponseError {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
impl From<talocart_platform::media::MediaError> for MediaResponseError {
    fn from(error: talocart_platform::media::MediaError) -> Self {
        Self(error.into())
    }
}
fn status(error: &LibraryError) -> StatusCode {
    match error {
        LibraryError::Identity(error) => access::IdentityErrorResponse::status_for(error),
        LibraryError::NotFound => StatusCode::NOT_FOUND,
        LibraryError::Collision | LibraryError::InUse => StatusCode::CONFLICT,
        LibraryError::Media(
            talocart_platform::media::MediaError::Storage
            | talocart_platform::media::MediaError::Busy,
        ) => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
impl IntoResponse for MediaResponseError {
    fn into_response(self) -> Response {
        (status(&self.0), i18n::library_error(&self.0)).into_response()
    }
}
fn library(state: &AppState) -> MediaLibrary {
    MediaLibrary::new(
        state.pool.clone(),
        state.identity.clone(),
        state.media.clone(),
    )
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/media", get(page).post(super::media_picker::choose))
        .route(
            "/admin/media/picker/upload",
            post(super::media_picker::upload).layer(DefaultBodyLimit::max(110 * 1024 * 1024)),
        )
        .route(
            "/admin/media/picker/folders",
            post(super::media_picker::create_folder),
        )
        .route(
            "/admin/media/picker/{id}",
            post(super::media_picker::detail),
        )
        .route(
            "/admin/media/tree/{id}",
            get(folder_branch).post(draft_folder_branch),
        )
        .route(
            "/admin/media/upload",
            post(upload).layer(DefaultBodyLimit::max(110 * 1024 * 1024)),
        )
        .route("/admin/media/folders", post(create_folder))
        .route("/admin/media/folders/{id}", post(change_folder))
        .route("/admin/media/{id}", get(detail).post(change_asset))
        .route("/admin/media/{id}/preview", get(preview))
        .route("/admin/media/select", post(select))
        .route("/admin/media/purge", post(purge))
        .route("/admin/media/cache", post(cache))
        .route("/admin/media/cache/status", get(cache_status))
        .route("/admin/media/{id}/download", get(download))
}
#[derive(Clone, Default, Deserialize)]
pub struct MediaQuery {
    #[serde(default)]
    pub target: String,
    pub folder: Option<i64>,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub usage: String,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub folders_page: i64,
    #[serde(default)]
    pub trash: bool,
    #[serde(default)]
    pub personal: bool,
    #[serde(default)]
    pub saved: bool,
    pub selected: Option<i64>,
    #[serde(default)]
    pub uses_page: i64,
}
impl MediaQuery {
    pub fn detail_url(&self, id: i64, page: i64, uses_page: i64) -> String {
        self.url(page, self.folder)
            .replacen("/admin/media?", &format!("/admin/media/{id}?"), 1)
            + &format!("&uses_page={}", uses_page.max(1))
    }
    fn filter(&self) -> Filter {
        Filter {
            images_only: !self.target.is_empty(),
            folder: self.folder,
            query: self.q.clone(),
            format: self.format.clone(),
            usage: self.usage.clone(),
            order: self.sort.clone(),
            page: self.page,
            trash: self.trash,
            personal: self.personal,
        }
    }
    pub fn url(&self, page: i64, folder: Option<i64>) -> String {
        format!(
            "/admin/media?target={}&{}q={}&format={}&usage={}&sort={}&page={}&personal={}&trash={}&folders_page={}",
            catalog::encoded(&self.target),
            folder.map(|id| format!("folder={id}&")).unwrap_or_default(),
            catalog::encoded(&self.q),
            self.format,
            self.usage,
            self.sort,
            page.max(1),
            self.personal,
            self.trash,
            self.folders_page.max(1)
        )
    }
}
// No arbitrary return URL/action from the browser. Destination identity is a typed allowlist.
#[derive(Clone, Copy)]
enum Target {
    Library,
    Product(i64),
    Gallery(i64),
    Variant(i64, i64),
    Brand(i64),
    Logo,
    Favicon,
    Avatar,
}
impl Target {
    fn parse(s: &str) -> std::result::Result<Self, LibraryError> {
        let parts = s.split(':').collect::<Vec<_>>();
        let id = |v: &str| {
            v.parse::<i64>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or(LibraryError::NotFound)
        };
        Ok(match parts.as_slice() {
            [""] => Self::Library,
            ["product", v] => Self::Product(id(v)?),
            ["gallery", v] => Self::Gallery(id(v)?),
            ["variant", p, v] => Self::Variant(id(p)?, id(v)?),
            ["brand", v] => Self::Brand(id(v)?),
            ["logo"] => Self::Logo,
            ["favicon"] => Self::Favicon,
            ["avatar"] => Self::Avatar,
            _ => return Err(LibraryError::NotFound),
        })
    }
    fn back(self) -> String {
        match self {
            Self::Library => "/admin/media".into(),
            Self::Product(id) | Self::Gallery(id) => {
                format!("/admin/catalog/products/{id}?tab=image")
            }
            Self::Variant(p, id) => format!("/admin/catalog/products/{p}/variants/{id}"),
            Self::Brand(id) => format!("/admin/catalog/brands/{id}"),
            Self::Logo | Self::Favicon => "/admin/settings/appearance".into(),
            Self::Avatar => "/admin/account".into(),
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Library => "admin-media-library",
            Self::Product(_) => "admin-main-image",
            Self::Gallery(_) => "admin-product-gallery",
            Self::Variant(_, _) => "admin-variant-image",
            Self::Brand(_) => "admin-brand-logo",
            Self::Logo => "admin-store-logo",
            Self::Favicon => "admin-favicon-title",
            Self::Avatar => "admin-personal-image",
        }
    }
    async fn authorize(self, state: &AppState, session: &Session) -> Result<()> {
        match self {
            Self::Avatar => {
                state.identity.account(session).await?;
            }
            Self::Library => {
                state
                    .identity
                    .authorize(session, Permission::MediaRead)
                    .await?;
            }
            Self::Logo | Self::Favicon => {
                state
                    .identity
                    .authorize(session, Permission::SettingsWrite)
                    .await?;
                state
                    .identity
                    .authorize(session, Permission::MediaRead)
                    .await?;
            }
            Self::Brand(_) => {
                state
                    .identity
                    .authorize(session, Permission::BrandsWrite)
                    .await?;
                state
                    .identity
                    .authorize(session, Permission::MediaRead)
                    .await?;
            }
            _ => {
                state
                    .identity
                    .authorize(session, Permission::ProductsWrite)
                    .await?;
                state
                    .identity
                    .authorize(session, Permission::MediaRead)
                    .await?;
            }
        }
        Ok(())
    }
}
#[derive(Template)]
#[template(path = "media-library.html")]
struct MediaPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    query: &'a MediaQuery,
    title: &'static str,
    back: String,
    picker: bool,
    draft: super::media_picker::Draft,
    can_write: bool,
    tree: Vec<FolderTreeEntry>,
    folder: Option<Folder>,
    assets: Vec<Asset>,
    pagination: super::pagination::Pagination,
    selected_asset: Option<Asset>,
    root_url: String,
    saved: bool,
    error: String,
    selected: i64,
    notice: String,
    cache_status: String,
}
struct FolderTreeLink {
    id: i64,
    name: String,
    href: String,
    load_url: String,
    has_children: bool,
    expanded: bool,
    active: bool,
}
enum FolderTreeEntry {
    Node(FolderTreeLink),
    CloseBranch,
    Pagination(super::pagination::Pagination, i64),
}
#[derive(Template)]
#[template(path = "media-folder-branch.html")]
struct FolderBranchPage {
    tree: Vec<FolderTreeEntry>,
    draft: super::media_picker::Draft,
}
fn folder_tree_entries(
    q: &MediaQuery,
    parent: Option<i64>,
    branches: Vec<FolderBranch>,
) -> Vec<FolderTreeEntry> {
    fn append(
        q: &MediaQuery,
        parent: Option<i64>,
        branches: &mut std::collections::HashMap<Option<i64>, FolderBranch>,
        entries: &mut Vec<FolderTreeEntry>,
    ) {
        let Some(branch) = branches.remove(&parent) else {
            return;
        };
        for node in branch.entries.entries {
            let expanded = branches.contains_key(&Some(node.id));
            let mut query = q.clone();
            query.folders_page = 1;
            let load_url = query.url(q.page, q.folder).replacen(
                "/admin/media?",
                &format!("/admin/media/tree/{}?", node.id),
                1,
            );
            entries.push(FolderTreeEntry::Node(FolderTreeLink {
                id: node.id,
                name: node.name,
                href: query.url(1, Some(node.id)),
                load_url,
                has_children: node.has_children,
                expanded,
                active: q.folder == Some(node.id),
            }));
            if node.has_children {
                if expanded {
                    append(q, Some(node.id), branches, entries);
                }
                entries.push(FolderTreeEntry::CloseBranch);
            }
        }
        let pagination = super::pagination::Pagination::new(
            branch.entries.total,
            branch.entries.page,
            100,
            &i18n::text("admin-folders"),
            &i18n::text("admin-folder-pages"),
            |page| {
                let mut query = q.clone();
                query.folders_page = page;
                query.url(1, parent)
            },
        );
        // A small branch needs neither counters nor disabled pagination controls.
        if pagination.pages > 1 {
            entries.push(FolderTreeEntry::Pagination(pagination, parent.unwrap_or(0)));
        }
    }
    let mut branches = branches.into_iter().map(|b| (b.parent, b)).collect();
    let mut entries = Vec::new();
    append(q, parent, &mut branches, &mut entries);
    entries
}
async fn folder_branch(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(q): Query<MediaQuery>,
) -> Result<Response> {
    let target = Target::parse(&q.target)?;
    target.authorize(&state, &session).await?;
    if q.personal && !matches!(target, Target::Avatar | Target::Library) {
        return Err(LibraryError::Destination.into());
    }
    let branches = library(&state)
        .folder_tree(&session, Some(id), q.folders_page, q.personal)
        .await?;
    Ok(render(FolderBranchPage {
        tree: folder_tree_entries(&q, Some(id), branches),
        draft: Default::default(),
    }))
}
async fn draft_folder_branch(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response> {
    let draft = super::media_picker::Draft::parse(pairs)?;
    draft.authorize(&state, &session).await?;
    let q = draft.query()?;
    let branches = library(&state)
        .folder_tree(&session, Some(id), q.folders_page, false)
        .await?;
    Ok(render(FolderBranchPage {
        tree: folder_tree_entries(&q, Some(id), branches),
        draft,
    }))
}
async fn page_response(
    state: &AppState,
    session: &Session,
    q: &MediaQuery,
    failure: Option<&LibraryError>,
    notice: String,
) -> Result<Response> {
    page_response_with_draft(state, session, q, failure, notice, Default::default()).await
}
pub(super) async fn draft_page(
    state: &AppState,
    session: &Session,
    draft: super::media_picker::Draft,
) -> Result<Response> {
    draft_page_with_failure(state, session, draft, None).await
}
pub(super) async fn draft_page_with_failure(
    state: &AppState,
    session: &Session,
    mut draft: super::media_picker::Draft,
    failure: Option<&LibraryError>,
) -> Result<Response> {
    let query = draft.query()?;
    draft.prepare();
    page_response_with_draft(state, session, &query, failure, String::new(), draft).await
}
async fn page_response_with_draft(
    state: &AppState,
    session: &Session,
    q: &MediaQuery,
    failure: Option<&LibraryError>,
    notice: String,
    draft: super::media_picker::Draft,
) -> Result<Response> {
    let target = Target::parse(&q.target)?;
    target.authorize(state, session).await?;
    if q.personal && !matches!(target, Target::Avatar | Target::Library) {
        return Err(LibraryError::Destination.into());
    }
    let lib = library(state);
    let personal = q.personal;
    let folder = match q.folder {
        Some(id) => Some(lib.folder(session, id, personal).await?),
        None => None,
    };
    let branches = lib
        .folder_tree(session, q.folder, q.folders_page, personal)
        .await?;
    let tree = folder_tree_entries(q, None, branches);
    let result = lib.assets_counted(session, &q.filter()).await?;
    let assets = result.entries;
    let mut selected_asset = None;
    if let Some(id) = q.selected
        && !assets.iter().any(|a| a.id == id)
    {
        let asset = lib.asset(session, id, personal).await?;
        if !asset.trashed
            && asset.owner_id
                == if personal {
                    session.administrator_id
                } else {
                    None
                }
        {
            selected_asset = Some(asset);
        }
    }
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let image_picker = !matches!(target, Target::Library);
    let pagination_label = i18n::text(if image_picker {
        "admin-images"
    } else {
        "admin-files"
    });
    let pagination_aria = i18n::text(if image_picker {
        "admin-image-pagination"
    } else {
        "admin-file-pagination"
    });

    let response = render(MediaPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "media",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        query: q,
        title: if draft.active() {
            if draft.prefix == "banner" {
                "banners-choose"
            } else {
                "settings-media-title"
            }
        } else {
            target.title()
        },
        back: target.back(),
        picker: !matches!(target, Target::Library),
        can_write: personal || session.allows(Permission::MediaWrite),
        tree,
        root_url: {
            let mut query = q.clone();
            query.folders_page = 1;
            query.url(1, None)
        },
        folder,
        assets,
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            40,
            &pagination_label,
            &pagination_aria,
            |page| q.url(page, q.folder),
        ),
        selected_asset,
        saved: q.saved,
        error: if draft.upload_failed {
            i18n::text("media-picker-upload-invalid")
        } else {
            failure.map(i18n::library_error).unwrap_or_default()
        },
        draft,
        selected: q.selected.unwrap_or(0),
        notice,
        cache_status: cache_text(state.media.cache_report_public(&state.pool).await?),
    });
    Ok(match failure {
        Some(source_error) => (status(source_error), response).into_response(),
        None => response,
    })
}
async fn page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<MediaQuery>,
) -> Result<Response> {
    page_response(&state, &session, &q, None, String::new()).await
}
#[derive(Default, Deserialize)]
struct ActionForm {
    csrf: String,
    #[serde(default)]
    target: String,
    folder: Option<i64>,
    #[serde(default)]
    personal: bool,
    #[serde(default)]
    name: String,
    #[serde(default)]
    destination: String,

    #[serde(default)]
    action: String,
    #[serde(default)]
    confirm: bool,
}
impl ActionForm {
    fn query(&self) -> MediaQuery {
        MediaQuery {
            target: self.target.clone(),
            folder: self.folder,
            personal: self.personal,
            ..Default::default()
        }
    }
}
async fn create_folder(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<ActionForm>,
) -> Result<Response> {
    let result = library(&state)
        .create_folder(&session, &f.csrf, f.folder, &f.name, f.personal)
        .await;
    match result {
        Ok(id) => Ok(Redirect::to(&f.query().url(1, Some(id))).into_response()),
        Err(error) => {
            page_response(&state, &session, &f.query(), Some(&error), String::new()).await
        }
    }
}
async fn upload(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    mut multipart: Multipart,
) -> Result<Response> {
    let mut q = MediaQuery::default();
    let mut csrf = None;
    let mut files = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| talocart_platform::media::MediaError::Invalid)?
    {
        match field.name() {
            Some("csrf") if csrf.is_none() => {
                let value = field
                    .text()
                    .await
                    .map_err(|_| talocart_platform::media::MediaError::Invalid)?;
                session.check_csrf(&value)?;
                csrf = Some(value);
            }
            Some("target") => {
                q.target = field
                    .text()
                    .await
                    .map_err(|_| talocart_platform::media::MediaError::Invalid)?
            }
            Some("folder") => {
                let value = field
                    .text()
                    .await
                    .map_err(|_| talocart_platform::media::MediaError::Invalid)?;
                q.folder = if value.is_empty() {
                    None
                } else {
                    Some(value.parse().map_err(|_| LibraryError::NotFound)?)
                };
            }
            Some("personal") => {
                q.personal = field
                    .text()
                    .await
                    .map_err(|_| talocart_platform::media::MediaError::Invalid)?
                    == "true"
            }
            Some("images") => {
                if files.len() >= 10 {
                    return Err(talocart_platform::media::MediaError::Invalid.into());
                }
                let name = field.file_name().unwrap_or("image.png").to_owned();
                let raw = field
                    .bytes()
                    .await
                    .map_err(|_| talocart_platform::media::MediaError::Invalid)?;
                files.push((name, raw.to_vec()));
            }
            _ => return Err(talocart_platform::media::MediaError::Invalid.into()),
        }
    }
    let csrf = csrf.ok_or(talocart_platform::identity::IdentityError::Forbidden)?;
    Target::parse(&q.target)?
        .authorize(&state, &session)
        .await?;
    if files.is_empty() {
        return page_response(
            &state,
            &session,
            &q,
            Some(&LibraryError::Media(
                talocart_platform::media::MediaError::Invalid,
            )),
            String::new(),
        )
        .await;
    }
    let mut completed = 0;
    let mut failures = Vec::new();
    for (name, raw) in files {
        match library(&state)
            .upload(&session, &csrf, q.folder, raw, &name, q.personal)
            .await
        {
            Ok(id) => {
                q.selected = Some(id);
                completed += 1;
            }
            Err(source_error) => failures.push(source_error),
        }
    }
    q.saved = completed > 0;
    if let Some(error) = failures.first() {
        return page_response(
            &state,
            &session,
            &q,
            Some(error),
            format!(
                "Caricati {completed} file. {} file rifiutati.",
                failures.len()
            ),
        )
        .await;
    }
    let mut url = q.url(1, q.folder);
    url.push_str(&format!("&saved=true&selected={}", q.selected.unwrap_or(0)));
    Ok(Redirect::to(&url).into_response())
}
#[derive(Template)]
#[template(path = "media-detail.html")]
struct DetailPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    asset: Asset,
    usages: Vec<Usage>,
    pagination: super::pagination::Pagination,
    picker: bool,
    target: String,
    can_write: bool,
    personal: bool,
    error: String,
    saved: bool,
    back: String,
    query: &'a MediaQuery,
    draft: super::media_picker::Draft,
}
async fn detail_response(
    state: &AppState,
    session: &Session,
    id: i64,
    q: &MediaQuery,
    failure: Option<&LibraryError>,
) -> Result<Response> {
    detail_response_with_draft(state, session, id, q, failure, Default::default()).await
}
pub(super) async fn draft_detail(
    state: &AppState,
    session: &Session,
    id: i64,
    q: &MediaQuery,
    draft: super::media_picker::Draft,
) -> Result<Response> {
    detail_response_with_draft(state, session, id, q, None, draft).await
}
async fn detail_response_with_draft(
    state: &AppState,
    session: &Session,
    id: i64,
    q: &MediaQuery,
    failure: Option<&LibraryError>,
    draft: super::media_picker::Draft,
) -> Result<Response> {
    if !q.target.is_empty() {
        Target::parse(&q.target)?.authorize(state, session).await?;
    }
    let lib = library(state);
    let asset = lib.asset(session, id, q.personal).await?;
    if draft.active() && (asset.trashed || !asset.is_image() || asset.owner_id.is_some()) {
        return Err(LibraryError::NotFound.into());
    }
    let result = lib
        .usages_counted(session, id, q.personal, q.uses_page)
        .await?;
    let usages = result.entries;
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let response = render(DetailPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "media",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        asset,
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            100,
            &i18n::text("admin-usages"),
            &i18n::text("admin-usage-pages"),
            |page| q.detail_url(id, q.page, page),
        ),
        picker: !q.target.is_empty(),
        target: q.target.clone(),
        usages,
        can_write: !draft.active() && (q.personal || session.allows(Permission::MediaWrite)),
        personal: q.personal,
        error: failure.map(i18n::library_error).unwrap_or_default(),
        saved: q.saved,
        back: q.url(q.page, q.folder),
        query: q,
        draft,
    });
    Ok(match failure {
        Some(source_error) => (status(source_error), response).into_response(),
        None => response,
    })
}
async fn detail(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(q): Query<MediaQuery>,
) -> Result<Response> {
    detail_response(&state, &session, id, &q, None).await
}
// Image endpoints resolve the archive from the authorized source, never a browser scope flag.
async fn authorized_image_asset(state: &AppState, session: &Session, id: i64) -> Result<Asset> {
    let lib = library(state);
    if session.allows(Permission::MediaRead) {
        match lib.asset(session, id, false).await {
            Ok(asset) => return Ok(asset),
            Err(LibraryError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(lib.asset(session, id, true).await?)
}
async fn preview(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    method: axum::http::Method,
) -> Result<Response> {
    let asset = authorized_image_asset(&state, &session, id).await?;
    if !asset.is_image() {
        return Err(LibraryError::NotFound.into());
    }
    let file = state.media.preview_file(&state.pool, &asset.key).await?;
    let Some(file) = file else {
        return Ok(images::fallback());
    };
    let ready = file.key != asset.key;
    let size = file.byte_size;
    let cached = images::matches_etag(&headers, &format!("\"media-{}\"", file.id));
    let bytes = if cached || method == axum::http::Method::HEAD {
        Vec::new()
    } else {
        state.media.read_registered(&state.pool, &file.key).await?
    };
    let mut response = images::response(
        talocart_platform::media::MediaImage {
            metadata: file.metadata(),
            png: bytes,
        },
        &headers,
        false,
    );
    response.headers_mut().insert(
        "x-media-ready",
        HeaderValue::from_static(if ready { "true" } else { "false" }),
    );
    if method == axum::http::Method::HEAD && !cached {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    }
    Ok(response)
}
async fn change_asset(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<ActionForm>,
) -> Result<Response> {
    let lib = library(&state);
    let result = match f.action.as_str() {
        "move" => {
            lib.move_asset(&session, &f.csrf, id, &f.destination, &f.name, f.personal)
                .await
        }
        "trash" => {
            lib.trash(&session, &f.csrf, id, false, f.personal, f.confirm)
                .await
        }
        "restore" => {
            lib.trash(&session, &f.csrf, id, true, f.personal, false)
                .await
        }
        "copy" => {
            match lib
                .copy_named(
                    &session,
                    &f.csrf,
                    id,
                    lib.destination(&session, &f.destination, f.personal)
                        .await?
                        .id,
                    &f.name,
                    f.personal,
                )
                .await
            {
                Ok(new_id) => {
                    return Ok(Redirect::to(&format!(
                        "/admin/media/{new_id}?personal={}&saved=true",
                        f.personal
                    ))
                    .into_response());
                }
                Err(error) => Err(error),
            }
        }
        _ => Err(LibraryError::Name),
    };
    match result {
        Ok(()) => Ok(Redirect::to(&format!(
            "/admin/media/{id}?personal={}&saved=true",
            f.personal
        ))
        .into_response()),
        Err(error) => detail_response(&state, &session, id, &f.query(), Some(&error)).await,
    }
}
async fn change_folder(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<ActionForm>,
) -> Result<Response> {
    let lib = library(&state);
    let result = match f.action.as_str() {
        "move" => {
            lib.move_folder(
                &session,
                &f.csrf,
                id,
                if f.destination.is_empty() {
                    None
                } else {
                    Some(
                        lib.destination(&session, &f.destination, f.personal)
                            .await?
                            .id,
                    )
                },
                &f.name,
                f.personal,
            )
            .await
        }
        "delete" => {
            lib.delete_folder(&session, &f.csrf, id, f.personal, f.confirm)
                .await
        }
        _ => Err(LibraryError::Name),
    };
    match result {
        Ok(()) => Ok(Redirect::to(&f.query().url(1, None)).into_response()),
        Err(error) => {
            page_response(&state, &session, &f.query(), Some(&error), String::new()).await
        }
    }
}
#[derive(Deserialize)]
struct SelectForm {
    csrf: String,
    target: String,
    #[serde(default)]
    personal: bool,
    #[serde(default)]
    ids: String,
}
async fn select(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<SelectForm>,
) -> Result<Response> {
    session.check_csrf(&f.csrf)?;
    let target = Target::parse(&f.target)?;
    target.authorize(&state, &session).await?;
    let ids = f
        .ids
        .split(',')
        .map(|v| v.parse::<i64>().map_err(|_| LibraryError::NotFound))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if ids.is_empty()
        || ids.len() > 12
        || ids.iter().any(|id| *id <= 0)
        || !matches!(target, Target::Gallery(_)) && ids.len() != 1
    {
        return Err(LibraryError::Name.into());
    }
    let result = match target {
        Target::Product(p) => state
            .products
            .select_image(&session, &f.csrf, p, ids[0], false)
            .await
            .map_err(product_error),
        Target::Gallery(p) => state
            .products
            .select_gallery_images(&session, &f.csrf, p, &ids)
            .await
            .map_err(product_error),
        Target::Variant(p, v) => state
            .variants
            .select_image(&session, &f.csrf, p, v, ids[0])
            .await
            .map_err(variant_error),
        Target::Brand(b) => state
            .catalog
            .select_brand_logo(&session, &f.csrf, b, ids[0])
            .await
            .map_err(catalog_error),
        Target::Logo | Target::Favicon => state
            .identity
            .select_branding(
                &session,
                &f.csrf,
                if matches!(target, Target::Logo) {
                    talocart_platform::identity::BrandingKind::Logo
                } else {
                    talocart_platform::identity::BrandingKind::Favicon
                },
                ids[0],
            )
            .await
            .map_err(LibraryError::Identity),
        Target::Avatar => state
            .identity
            .select_avatar(&session, &f.csrf, ids[0])
            .await
            .map_err(LibraryError::Identity),
        Target::Library => return Err(LibraryError::Name.into()),
    };
    if let Err(error) = result {
        let q = MediaQuery {
            target: f.target,
            personal: f.personal,
            ..Default::default()
        };
        return page_response(&state, &session, &q, Some(&error), String::new()).await;
    }
    if let Target::Product(id) | Target::Gallery(id) = target {
        return Ok(
            Redirect::to(&format!("/admin/catalog/products/{id}?saved=1&tab=image"))
                .into_response(),
        );
    }
    let back = target.back();
    let separator = if back.contains('?') { '&' } else { '?' };
    Ok(Redirect::to(&format!("{back}{separator}saved=1")).into_response())
}
async fn purge(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<ActionForm>,
) -> Result<Response> {
    library(&state)
        .purge(&session, &f.csrf, f.personal, f.confirm)
        .await?;
    Ok(Redirect::to(&format!(
        "/admin/media?personal={}&trash=true&saved=true",
        f.personal
    ))
    .into_response())
}
async fn cache(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<ActionForm>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::MediaWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    if !f.confirm {
        return Err(LibraryError::Confirmation.into());
    }
    let action = match f.action.as_str() {
        "repair" => talocart_platform::media::CacheAction::Repair,
        "rebuild" => talocart_platform::media::CacheAction::Rebuild,
        "clear" => talocart_platform::media::CacheAction::Clear,
        _ => return Err(LibraryError::Name.into()),
    };
    state
        .media
        .queue_cache_action(&state.pool, &session, &f.csrf, action)
        .await?;
    Ok(Redirect::to("/admin/media?saved=true").into_response())
}

pub async fn public_alias(
    State(state): State<AppState>,
    Path((id, profile)): Path<(i64, String)>,
    headers: HeaderMap,
    method: axum::http::Method,
) -> Result<Response> {
    match state
        .media
        .rendition_file(&state.pool, id, &profile)
        .await?
    {
        Some(file) => Ok(images::public_file_response(&state, file, &headers, method).await?),
        None => Ok(images::fallback()),
    }
}

fn product_error(error: talocart_commerce::products::ProductError) -> LibraryError {
    match error {
        talocart_commerce::products::ProductError::Technical(error) => {
            LibraryError::Identity(error.into())
        }
        talocart_commerce::products::ProductError::Access(error) => {
            LibraryError::Identity(error.into())
        }
        talocart_commerce::products::ProductError::Identity(error) => LibraryError::Identity(error),
        talocart_commerce::products::ProductError::NotFound => LibraryError::NotFound,
        _ => LibraryError::Attachment(i18n::product_error(&error)),
    }
}
fn variant_error(error: talocart_commerce::variants::VariantError) -> LibraryError {
    match error {
        talocart_commerce::variants::VariantError::Technical(error) => {
            LibraryError::Identity(error.into())
        }
        talocart_commerce::variants::VariantError::Access(error) => {
            LibraryError::Identity(error.into())
        }
        talocart_commerce::variants::VariantError::Identity(error) => LibraryError::Identity(error),
        talocart_commerce::variants::VariantError::Product(error) => product_error(error),
        talocart_commerce::variants::VariantError::NotFound => LibraryError::NotFound,
        _ => LibraryError::Attachment(i18n::variant_error(&error)),
    }
}
fn catalog_error(error: talocart_commerce::CatalogError) -> LibraryError {
    match error {
        talocart_commerce::CatalogError::Technical(error) => LibraryError::Identity(error.into()),
        talocart_commerce::CatalogError::Access(error) => LibraryError::Identity(error.into()),
        talocart_commerce::CatalogError::Identity(error) => LibraryError::Identity(error),
        talocart_commerce::CatalogError::NotFound => LibraryError::NotFound,
        _ => LibraryError::Attachment(i18n::catalog_error(&error)),
    }
}

fn cache_text(report: talocart_platform::media::CacheReport) -> String {
    let mut args = fluent_bundle::FluentArgs::new();
    args.set("complete", report.complete.to_string());
    args.set("pending", report.pending.to_string());
    args.set("running", report.running.to_string());
    args.set("failed", report.failed.to_string());
    i18n::with_args("admin-cache-report", &args)
}

async fn cache_status(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::MediaRead)
        .await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        cache_text(state.media.cache_report_public(&state.pool).await?),
    )
        .into_response())
}
async fn download(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let asset = authorized_image_asset(&state, &session, id).await?;
    use tokio::io::AsyncReadExt;
    let file = state
        .media
        .open_registered(&state.pool, &asset.key)
        .await
        .map_err(|error| {
            if matches!(error, talocart_platform::media::MediaError::Missing) {
                LibraryError::NotFound
            } else {
                LibraryError::Media(error)
            }
        })?;
    let stream = futures_util::stream::try_unfold(file, |mut file| async move {
        let mut buffer = vec![0u8; 64 * 1024];
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            Ok::<_, std::io::Error>(None)
        } else {
            buffer.truncate(read);
            Ok(Some((axum::body::Bytes::from(buffer), file)))
        }
    });
    let mut response = axum::body::Body::from_stream(stream).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    if let Ok(value) = HeaderValue::from_str(&asset.content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    let encoded: String = asset.name.bytes().map(|b| format!("%{b:02X}")).collect();
    let disposition = HeaderValue::from_str(&format!(
        "attachment; filename=\"download\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment"));
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition);
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

/// Translate the fixed discriminators exposed by the immutable media_usage view.
pub(crate) fn usage_label(kind: &str) -> String {
    match kind {
        "Prodotto" => i18n::text("admin-resource-product"),
        "Variante" => i18n::text("admin-resource-variant"),
        "Galleria" => i18n::text("admin-resource-gallery"),
        "Marchio" => i18n::text("admin-brand-2"),
        "Logo negozio" => i18n::text("admin-store-logo"),
        "Favicon" => i18n::text("admin-favicon-title"),
        "Avatar" => i18n::text("admin-resource-avatar"),
        "Banner" => i18n::text("banners-singular"),
        _ => kind.to_owned(),
    }
}

#[cfg(test)]
mod localization_tests {
    use super::*;
    #[tokio::test]
    async fn attachment_errors_are_localized_from_variants_and_sql_kinds_are_explicit() {
        for (locale, confirmation, product, logo) in [
            (
                talocart_platform::locale::Locale::ENGLISH,
                "Confirm product deletion",
                "Product",
                "Store logo",
            ),
            (
                talocart_platform::locale::Locale::ITALIAN,
                "Conferma l’eliminazione del prodotto",
                "Prodotto",
                "Logo del negozio",
            ),
        ] {
            i18n::scope(locale, async {
                let error = product_error(talocart_commerce::products::ProductError::Confirmation);
                assert_eq!(i18n::library_error(&error), confirmation);
                assert_eq!(usage_label("Prodotto"), product);
                assert_eq!(usage_label("Logo negozio"), logo);
                assert_eq!(usage_label("Unknown <kind>"), "Unknown <kind>");
            })
            .await;
        }
    }
}
