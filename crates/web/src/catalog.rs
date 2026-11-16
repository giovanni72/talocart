use super::*;
use axum::{
    extract::{Form, Multipart, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use talocart_commerce::{CatalogError, Entry, Field, Kind, PAGE_SIZE, Values};

type Result<T> = std::result::Result<T, CatalogResponseError>;
pub struct CatalogResponseError(CatalogError);
impl From<CatalogError> for CatalogResponseError {
    fn from(error: CatalogError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for CatalogResponseError {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
fn status(error: &CatalogError) -> StatusCode {
    match error {
        CatalogError::Content(error) => crate::content::status(error),
        CatalogError::Invalid(_) | CatalogError::Hierarchy | CatalogError::Limit => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        CatalogError::DuplicateSlug | CatalogError::DuplicateName | CatalogError::InUse => {
            StatusCode::CONFLICT
        }
        CatalogError::NotFound => StatusCode::NOT_FOUND,
        CatalogError::Technical(source_error) => crate::access::technical_status(source_error),
        CatalogError::Access(source_error) => crate::access::access_status(source_error),
        CatalogError::Identity(source_error) => {
            access::IdentityErrorResponse::status_for(source_error)
        }
    }
}
impl IntoResponse for CatalogResponseError {
    fn into_response(self) -> Response {
        if let CatalogError::Technical(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let CatalogError::Access(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        let mut r = (status(&self.0), i18n::catalog_error(&self.0)).into_response();
        if let CatalogError::Identity(error) = &self.0 {
            access::IdentityErrorResponse::retry_for(error, &mut r);
        }
        r
    }
}
pub fn routes() -> Router<AppState> {
    let mut router = Router::new();
    for kind in [Kind::Category, Kind::Brand] {
        let path = kind.path();
        router = router.merge(
            Router::new()
                .route(path, get(index))
                .route(&format!("{path}/new"), get(new_page).post(create))
                .route(&format!("{path}/{{id}}"), get(edit_page).post(update))
                .route(&format!("{path}/{{id}}/delete"), post(delete))
                .layer(Extension(kind)),
        );
    }
    router
        .route(
            "/admin/catalog/brands/{id}/logo",
            get(logo).post(upload).layer(DefaultBodyLimit::max(
                talocart_platform::identity::BRANDING_UPLOAD_LIMIT + 16 * 1024,
            )),
        )
        .route("/admin/catalog/brands/{id}/logo/remove", post(remove_logo))
}
#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    status: String,
    #[serde(default = "first_page")]
    page: i64,
    saved: Option<String>,
    deleted: Option<String>,
}
fn first_page() -> i64 {
    1
}
pub(crate) fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
struct Row {
    entry: Entry,
    parent: String,
    initial: String,
    depth: u8,
    context: bool,
    opens_children: bool,
    closes_children: std::ops::Range<u8>,
}
#[derive(Template)]
#[template(path = "catalog-list.html")]
struct ListPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    category: bool,
    path: &'static str,
    title: &'static str,
    rows: Vec<Row>,
    has_context: bool,
    can_write: bool,
    q: &'a str,
    filter: &'a str,
    pagination: super::pagination::Pagination,
    saved: bool,
    deleted: bool,
}
fn labels(kind: Kind) -> (&'static str, &'static str) {
    match kind {
        Kind::Category => ("categories", "admin-categories-2"),
        Kind::Brand => ("brands", "admin-brands-2"),
    }
}
pub async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let state = state.primary_content().await?;
    let active = match query.status.as_str() {
        "" => None,
        "active" => Some(true),
        "inactive" => Some(false),
        _ => return Err(CatalogError::Invalid(vec![]).into()),
    };
    let (mut rows, total, result_page) = if kind == Kind::Category {
        let result = state
            .catalog
            .category_tree_page(&session, &query.q, active, query.page)
            .await?;
        let rows = result
            .entries
            .into_iter()
            .map(|row| Row {
                entry: row.entry,
                parent: row.parent_path,
                initial: String::new(),
                depth: row.depth,
                context: row.context,
                opens_children: false,
                closes_children: 0..0,
            })
            .collect::<Vec<_>>();
        (rows, result.total, result.page)
    } else {
        let result = state
            .catalog
            .list_counted(&session, kind, &query.q, active, query.page)
            .await?;
        let rows = result
            .entries
            .into_iter()
            .map(|entry| {
                let initial = entry
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().collect())
                    .unwrap_or_default();
                Row {
                    entry,
                    parent: String::new(),
                    initial,
                    depth: 0,
                    context: false,
                    opens_children: false,
                    closes_children: 0..0,
                }
            })
            .collect::<Vec<_>>();
        (rows, result.total, result.page)
    };
    let depths = rows.iter().map(|row| row.depth).collect::<Vec<_>>();
    for (index, row) in rows.iter_mut().enumerate() {
        let next = depths.get(index + 1).copied().unwrap_or_default();
        row.opens_children = next > row.depth;
        row.closes_children = 0..row.depth.saturating_sub(next);
    }
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    let (section, title) = labels(kind);
    let page_link = |page| {
        format!(
            "{}?q={}&status={}&page={page}",
            kind.path(),
            encoded(&query.q),
            query.status
        )
    };
    Ok(render(ListPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section,
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        category: kind == Kind::Category,
        path: kind.path(),
        title,
        has_context: rows.iter().any(|row| row.context),
        rows,
        can_write: session.allows(kind.write()),
        q: &query.q,
        filter: &query.status,
        pagination: super::pagination::Pagination::new(
            total,
            result_page,
            PAGE_SIZE as u16,
            &i18n::text(title),
            &i18n::text(match kind {
                Kind::Category => "admin-category-pages",
                Kind::Brand => "admin-brand-pages-2",
            }),
            page_link,
        ),
        saved: query.saved.as_deref() == Some("1"),
        deleted: query.deleted.as_deref() == Some("1"),
    }))
}
struct ParentChoice {
    id: i64,
    name: String,
    active: bool,
    selected: bool,
}
#[derive(Template)]
#[template(path = "catalog-form.html")]
struct FormPage<'a> {
    content: content::Editor,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    category: bool,
    path: &'static str,
    title: &'static str,
    id: i64,
    values: &'a Values,
    parents: Vec<ParentChoice>,
    can_write: bool,
    error: String,
    name_error: bool,
    slug_error: bool,
    description_error: bool,
    parent_error: bool,
    order_error: bool,
    brand_logo: Option<talocart_platform::media::ImageMetadata>,
    saved: bool,
}
#[derive(Default)]
struct FormFeedback<'a> {
    error: Option<&'a CatalogError>,
    saved: bool,
    submitted: Option<&'a content::Submission>,
}
async fn form_response(
    state: &AppState,
    session: &Session,
    kind: Kind,
    id: i64,
    values: &Values,
    feedback: FormFeedback<'_>,
) -> Result<Response> {
    let FormFeedback {
        error,
        saved,
        submitted,
    } = feedback;
    let primary_state = state.primary_content().await?;
    let state = &primary_state;
    state.identity.authorize(session, kind.read()).await?;
    if let Some(source_error) = error
        && (matches!(
            source_error,
            CatalogError::NotFound | CatalogError::Technical(_) | CatalogError::Access(_)
        ) || matches!(source_error, CatalogError::Identity(_))
            && !matches!(
                source_error,
                CatalogError::Identity(talocart_platform::identity::IdentityError::InvalidBranding)
            ))
    {
        return Ok((status(source_error), i18n::catalog_error(source_error)).into_response());
    }
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let (section, title) = labels(kind);
    let field = |f| match error {
        Some(CatalogError::Invalid(fields)) => fields.contains(&f),
        Some(CatalogError::DuplicateSlug) => f == Field::Slug,
        Some(CatalogError::DuplicateName) => f == Field::Name,
        Some(CatalogError::Hierarchy) => f == Field::Parent,
        _ => false,
    };
    let brand_logo = if kind == Kind::Brand && id > 0 {
        let entry = state.catalog.entry(session, kind, id).await?;
        match entry.logo_key {
            Some(key) => state.catalog.logo_metadata(&key).await?,
            None => None,
        }
    } else {
        None
    };
    let resource = if kind == Kind::Category {
        talocart_commerce::content::Resource::Category
    } else {
        talocart_commerce::content::Resource::Brand
    };
    let content = content::Editor::load(
        state,
        session,
        resource,
        id,
        submitted,
        error.and_then(|source_error| {
            if let CatalogError::Content(source_error) = source_error {
                Some(source_error)
            } else {
                None
            }
        }),
        "catalog-content",
    )
    .await?;
    let mut response = render(FormPage {
        content,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section,
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        category: kind == Kind::Category,
        path: kind.path(),
        title,
        id,
        values,
        parents: if kind == Kind::Category {
            state
                .catalog
                .parents(session, id)
                .await?
                .into_iter()
                .map(|p| ParentChoice {
                    id: p.id,
                    selected: values.parent_id == Some(p.id),
                    name: p.name,
                    active: p.active,
                })
                .collect()
        } else {
            vec![]
        },
        can_write: session.allows(kind.write()),
        error: error.map(i18n::catalog_error).unwrap_or_default(),
        name_error: field(Field::Name),
        slug_error: field(Field::Slug),
        description_error: field(Field::Description),
        parent_error: field(Field::Parent),
        order_error: field(Field::Order),
        brand_logo,
        saved,
    });
    if let Some(source_error) = error {
        *response.status_mut() = status(source_error);
    }
    Ok(response)
}
#[derive(Deserialize)]
pub struct EditQuery {
    saved: Option<String>,
}
pub async fn new_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
) -> Result<Response> {
    state.identity.authorize(&session, kind.write()).await?;
    form_response(
        &state,
        &session,
        kind,
        0,
        &Values {
            active: true,
            sort_order: "0".into(),
            ..Values::default()
        },
        FormFeedback {
            error: None,
            saved: false,
            submitted: None,
        },
    )
    .await
}
pub async fn edit_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
    Path(id): Path<i64>,
    Query(query): Query<EditQuery>,
) -> Result<Response> {
    let state = state.primary_content().await?;
    let entry = state.catalog.entry(&session, kind, id).await?;
    form_response(
        &state,
        &session,
        kind,
        id,
        &entry.values(),
        FormFeedback {
            error: None,
            saved: query.saved.as_deref() == Some("1"),
            submitted: None,
        },
    )
    .await
}
#[derive(Deserialize)]
pub struct EntryForm {
    #[serde(flatten)]
    content: content::Submission,
    csrf: String,
    name: String,
    slug: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parent_id: String,
    sort_order: String,
    #[serde(default)]
    active: bool,
}
impl EntryForm {
    fn values(&self) -> Values {
        Values {
            name: self.name.clone(),
            slug: self.slug.clone(),
            description: self.description.clone(),
            parent_id: if self.parent_id.is_empty() {
                None
            } else {
                Some(self.parent_id.parse().unwrap_or(-1))
            },
            sort_order: self.sort_order.clone(),
            active: self.active,
        }
    }
}
async fn save_response(
    state: AppState,
    session: Session,
    kind: Kind,
    id: Option<i64>,
    form: EntryForm,
) -> Result<Response> {
    state.identity.authorize(&session, kind.write()).await?;
    session.check_csrf(&form.csrf)?;
    let values = form.values();
    match state
        .catalog
        .save_with_content(
            &session,
            &form.csrf,
            kind,
            id,
            &values,
            content::draft(&form.content).as_ref(),
        )
        .await
    {
        Ok(id) => Ok(Redirect::to(&format!("{}/{id}?saved=1", kind.path())).into_response()),
        Err(CatalogError::Technical(source_error)) => Err(source_error.into()),
        Err(CatalogError::Access(source_error)) => Err(source_error.into()),
        Err(CatalogError::Identity(source_error)) => Err(source_error.into()),
        Err(source_error) => {
            form_response(
                &state,
                &session,
                kind,
                id.unwrap_or(0),
                &values,
                FormFeedback {
                    error: Some(&source_error),
                    saved: false,
                    submitted: Some(&form.content),
                },
            )
            .await
        }
    }
}
pub async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
    Form(form): Form<EntryForm>,
) -> Result<Response> {
    save_response(state, session, kind, None, form).await
}
pub async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
    Path(id): Path<i64>,
    Form(form): Form<EntryForm>,
) -> Result<Response> {
    save_response(state, session, kind, Some(id), form).await
}
#[derive(Deserialize)]
pub struct CsrfForm {
    csrf: String,
    #[serde(default)]
    confirm: bool,
}
pub async fn delete(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(kind): Extension<Kind>,
    Path(id): Path<i64>,
    Form(form): Form<CsrfForm>,
) -> Result<Response> {
    state.identity.authorize(&session, kind.write()).await?;
    session.check_csrf(&form.csrf)?;
    let result = if form.confirm {
        state.catalog.delete(&session, &form.csrf, kind, id).await
    } else {
        Err(CatalogError::Invalid(vec![]))
    };
    match result {
        Ok(()) => Ok(Redirect::to(&format!("{}?deleted=1", kind.path())).into_response()),
        Err(CatalogError::Technical(source_error)) => Err(source_error.into()),
        Err(CatalogError::Access(source_error)) => Err(source_error.into()),
        Err(CatalogError::Identity(source_error)) => Err(source_error.into()),
        Err(source_error) => {
            let entry = state.catalog.entry(&session, kind, id).await?;
            form_response(
                &state,
                &session,
                kind,
                id,
                &entry.values(),
                FormFeedback {
                    error: Some(&source_error),
                    saved: false,
                    submitted: None,
                },
            )
            .await
        }
    }
}
pub async fn logo(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let entry = state.catalog.entry(&session, Kind::Brand, id).await?;
    if let Some(key) = entry.logo_key
        && let Some(image) = state.catalog.logo_metadata(&key).await?
    {
        return Ok(
            Redirect::temporary(&format!("/media/public/{}/{}/card", image.id, image.key))
                .into_response(),
        );
    }
    Ok(images::fallback())
}
async fn logo_result(
    state: &AppState,
    session: &Session,
    id: i64,
    result: talocart_commerce::Result<()>,
) -> Result<Response> {
    match result {
        Ok(()) => Ok(Redirect::to(&format!("/admin/catalog/brands/{id}?saved=1")).into_response()),
        Err(source_error) => {
            let entry = state.catalog.entry(session, Kind::Brand, id).await?;
            form_response(
                state,
                session,
                Kind::Brand,
                id,
                &entry.values(),
                FormFeedback {
                    error: Some(&source_error),
                    saved: false,
                    submitted: None,
                },
            )
            .await
        }
    }
}
pub async fn upload(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::BrandsWrite)
        .await?;
    let result = match branding::read_upload(&session, multipart).await {
        Ok((raw, name)) => {
            state
                .catalog
                .brand_logo(&session, &session.csrf, id, Some(raw), &name)
                .await
        }
        Err(error) => Err(error.into()),
    };
    logo_result(&state, &session, id, result).await
}
pub async fn remove_logo(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<CsrfForm>,
) -> Result<Response> {
    let result = state
        .catalog
        .brand_logo(&session, &form.csrf, id, None, "")
        .await;
    logo_result(&state, &session, id, result).await
}

impl From<talocart_platform::technical::TechnicalError> for CatalogResponseError {
    fn from(error: talocart_platform::technical::TechnicalError) -> Self {
        Self(CatalogError::Technical(error))
    }
}

impl From<talocart_platform::technical::AccessError> for CatalogResponseError {
    fn from(error: talocart_platform::technical::AccessError) -> Self {
        Self(CatalogError::Access(error))
    }
}
