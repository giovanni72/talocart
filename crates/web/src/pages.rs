use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_commerce::pages::{CONTENT, Input, PageError, Pages};
use talocart_platform::{content::Language, identity::IdentityError, locale::Locale};
const PATH: &str = "/admin/pages";
// Two 20,000-character descriptions plus metadata, including UTF-8 form encoding.
const FORM_BODY_LIMIT: usize = 512 * 1024;
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route(PATH, get(index))
        .route(
            "/admin/pages/new",
            get(new_page)
                .post(create)
                .layer(DefaultBodyLimit::max(FORM_BODY_LIMIT)),
        )
        .route(
            "/admin/pages/{id}",
            get(edit)
                .post(update)
                .layer(DefaultBodyLimit::max(FORM_BODY_LIMIT)),
        )
}
pub(crate) fn service(state: &AppState) -> Pages {
    Pages::new(state.pool.clone(), state.identity.clone())
}
pub(crate) fn identity_error(error: PageError) -> IdentityError {
    match error {
        PageError::Technical(error) => error.into(),
        PageError::Access(error) => error.into(),
        PageError::Identity(error) => error,
        _ => IdentityError::Unavailable,
    }
}
pub(super) struct Error(PageError);
impl From<PageError> for Error {
    fn from(error: PageError) -> Self {
        Self(error)
    }
}
impl From<IdentityError> for Error {
    fn from(error: IdentityError) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self.0 {
            PageError::Technical(error) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            PageError::Access(error) => access::IdentityErrorResponse::from(error).into_response(),
            PageError::Identity(error) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            PageError::Content(talocart_platform::content::ContentError::Identity(error)) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            PageError::NotFound => {
                (StatusCode::NOT_FOUND, i18n::text("pages-not-found")).into_response()
            }
            error => (status(&error), message(&error)).into_response(),
        }
    }
}
fn status(error: &PageError) -> StatusCode {
    match error {
        PageError::NotFound => StatusCode::NOT_FOUND,
        PageError::Duplicate | PageError::Stale => StatusCode::CONFLICT,
        PageError::Content(error) => content::status(error),
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
fn message(error: &PageError) -> String {
    match error {
        PageError::Content(error) => content::message(error),
        _ => i18n::text(match error {
            PageError::Duplicate => "pages-duplicate",
            PageError::Stale => "pages-stale",
            PageError::Limit => "pages-limit",
            PageError::NotFound => "pages-not-found",
            _ => "pages-invalid",
        }),
    }
}
#[derive(Deserialize)]
struct List {
    #[serde(default = "first")]
    page: i64,
    saved: Option<String>,
}
fn first() -> i64 {
    1
}
#[derive(Template)]
#[template(path = "pages.html")]
struct AdminPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    can_write: bool,
    saved: bool,
    editing: bool,
    id: i64,
    revision: i64,
    input: Input,
    content: content::Editor,
    entries: Vec<talocart_commerce::pages::PageSummary>,
    pagination: pagination::Pagination,
    error: String,
    invalid: Vec<&'static str>,
}
async fn response(
    state: &AppState,
    session: &Session,
    q: List,
    editor: Option<(i64, i64, Input)>,
    submitted: Option<&content::Submission>,
    error: Option<PageError>,
) -> Result<Response, Error> {
    state
        .identity
        .authorize(session, Permission::PagesRead)
        .await?;
    let theme = state.storefront.settings().await?;
    let primary = Locale::parse(&theme.primary_locale).ok_or(IdentityError::Unavailable)?;
    let language = Language::new(primary, primary);
    let editing = editor.is_some();
    let (entries, total, page) = if editing {
        (vec![], 0, 1)
    } else {
        let r = service(state).list(session, q.page, language).await?;
        (r.entries, r.total, r.page)
    };
    let (id, revision, input) = editor.unwrap_or_default();
    let rows = if id > 0 {
        match CONTENT
            .load(&state.pool, &state.identity, session, id)
            .await
        {
            Ok(rows) => rows,
            // A concurrent deletion must not replace the submitted draft with a 404.
            Err(talocart_platform::content::ContentError::NotFound)
                if matches!(&error, Some(PageError::Stale)) && submitted.is_some() =>
            {
                vec![]
            }
            Err(source_error) => return Err(PageError::from(source_error).into()),
        }
    } else {
        vec![]
    };
    let content = content::Editor::new(
        &theme.primary_locale,
        &theme.active_locales,
        CONTENT,
        rows,
        submitted,
        error.as_ref().and_then(|source_error| {
            if let PageError::Content(source_error) = source_error {
                Some(source_error)
            } else {
                None
            }
        }),
        "page-content",
    );
    let mut invalid = match &error {
        Some(PageError::Invalid(f)) => f.clone(),
        Some(PageError::Duplicate) => vec!["slug"],
        _ => vec![],
    };
    if let Some(PageError::Content(talocart_platform::content::ContentError::Invalid(fields))) =
        &error
    {
        invalid.extend(
            fields
                .iter()
                .filter(|(locale, _)| locale == &content.primary)
                .map(|(_, field)| *field),
        );
    }
    let name = current_name(state).await?;
    let admin_shell = AdminShell::with_settings(state, session, name, theme).await?;
    let name = admin_shell.store_name;
    let mut r = render(AdminPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "pages",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        can_write: session.allows(Permission::PagesWrite),
        saved: q.saved.as_deref() == Some("1"),
        editing,
        id,
        revision,
        input,
        content,
        entries,
        pagination: pagination::Pagination::new(
            total,
            page,
            20,
            &i18n::text("pages-title"),
            &i18n::text("pages-title"),
            |p| format!("{PATH}?page={p}"),
        ),
        error: error
            .as_ref()
            .map(|entry| {
                if !editing && matches!(entry, PageError::Stale) {
                    i18n::text("pages-delete-conflict")
                } else {
                    message(entry)
                }
            })
            .unwrap_or_default(),
        invalid,
    });
    if let Some(source_error) = error {
        *r.status_mut() = status(&source_error);
    }
    Ok(r)
}
fn query() -> List {
    List {
        page: 1,
        saved: None,
    }
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<List>,
) -> Result<Response, Error> {
    response(&state, &session, q, None, None, None).await
}
async fn new_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, Error> {
    state
        .identity
        .authorize(&session, Permission::PagesWrite)
        .await?;
    response(
        &state,
        &session,
        query(),
        Some((
            0,
            0,
            Input {
                sort_order: "0".into(),
                ..Input::default()
            },
        )),
        None,
        None,
    )
    .await
}
async fn edit(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(q): Query<List>,
) -> Result<Response, Error> {
    let theme = state.storefront.settings().await?;
    let primary = Locale::parse(&theme.primary_locale).ok_or(IdentityError::Unavailable)?;
    let p = service(&state)
        .get(&session, id, Language::new(primary, primary))
        .await?;
    response(
        &state,
        &session,
        q,
        Some((id, p.revision, p.input())),
        None,
        None,
    )
    .await
}
async fn save(
    state: AppState,
    session: Session,
    id: i64,
    form: content::Submission,
) -> Result<Response, Error> {
    let field = |key: &str| form.get(key).cloned().unwrap_or_default();
    let csrf = field("csrf");
    session.check_csrf(&csrf)?;
    let revision = field("revision")
        .parse::<i64>()
        .map_err(|_| PageError::Invalid(vec!["revision"]))?;
    let input = Input {
        slug: field("slug"),
        name: field("name"),
        description: talocart_platform::rich_text::submitted(
            &field("description"),
            &field("description_format"),
        ),
        seo_title: field("seo_title"),
        seo_description: field("seo_description"),
        sort_order: field("sort_order"),
        published: field("published") == "true",
        header_menu: field("header_menu") == "true",
        footer_menu: field("footer_menu") == "true",
    };
    let result = service(&state)
        .save(
            &session,
            &csrf,
            id,
            revision,
            &input,
            content::draft(&form).as_ref(),
        )
        .await;
    match result {
        Ok(id) => Ok(Redirect::to(&format!("{PATH}/{id}?saved=1")).into_response()),
        Err(error @ (PageError::Identity(_) | PageError::Technical(_) | PageError::Access(_))) => {
            Err(Error(error))
        }
        Err(error @ PageError::Content(talocart_platform::content::ContentError::Identity(_))) => {
            Err(Error(error))
        }
        Err(error) => {
            response(
                &state,
                &session,
                query(),
                Some((id, revision, input)),
                Some(&form),
                Some(error),
            )
            .await
        }
    }
}
async fn delete(
    state: AppState,
    session: Session,
    id: i64,
    form: content::Submission,
) -> Result<Response, Error> {
    let csrf = form.get("csrf").map(String::as_str).unwrap_or_default();
    session.check_csrf(csrf)?;
    let revision = form
        .get("revision")
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or(PageError::Invalid(vec!["revision"]))?;
    match service(&state).delete(&session, csrf, id, revision).await {
        Ok(()) => Ok(Redirect::to("/admin/pages?saved=1").into_response()),
        Err(PageError::Stale) => {
            // A delete form contains no content draft: render the current list.
            response(
                &state,
                &session,
                query(),
                None,
                None,
                Some(PageError::Stale),
            )
            .await
        }
        Err(error) => Err(error.into()),
    }
}
async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<content::Submission>,
) -> Result<Response, Error> {
    save(state, session, 0, form).await
}
async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<content::Submission>,
) -> Result<Response, Error> {
    if form.get("action").is_some_and(|action| action == "delete") {
        delete(state, session, id, form).await
    } else {
        save(state, session, id, form).await
    }
}
#[derive(Template)]
#[template(path = "default/templates/information-page.html")]
struct PublicPage {
    store_name: String,
    logo: Option<talocart_platform::media::ImageMetadata>,
    theme: talocart_platform::storefront::Settings,
    search_query: String,
    search_action: &'static str,
    search_filters: navigation::SearchFilters,
    category_menu: navigation::CategoryMenu,
    name: String,
    body_html: String,
    meta_title: String,
    meta_description: String,
    canonical: String,
}
pub async fn public(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Response, Error> {
    let p = service(&state)
        .public(&slug, store_language::content_language())
        .await?;
    if p.slug != slug {
        let mut r = Redirect::permanent(&store_language::path(format!("/pages/{}", p.slug)))
            .into_response();
        *r.status_mut() = StatusCode::MOVED_PERMANENTLY;
        return Ok(r);
    }
    let theme = store_language::settings(&state).await?;
    let canonical = theme.canonical(
        &state.public_origin,
        &store_language::path(format!("/pages/{}", p.slug)),
    );
    let meta_title = if p.seo_title.is_empty() {
        p.name.clone()
    } else {
        p.seo_title
    };
    let body_html = talocart_platform::rich_text::render(&p.description);
    Ok(render(PublicPage {
        store_name: current_name(&state).await?,
        logo: current_logo(&state).await?,
        theme,
        search_query: String::new(),
        search_action: "/search",
        search_filters: navigation::SearchFilters::default(),
        category_menu: navigation::CategoryMenu::load(&state, "", false, "", false).await?,
        name: p.name,
        body_html,
        meta_title,
        meta_description: p.seo_description,
        canonical,
    }))
}
