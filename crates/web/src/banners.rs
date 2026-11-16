use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_platform::{
    banners::{Banner, BannerError, Banners, CONTENT, Input},
    content::Language,
    identity::IdentityError,
    locale::Locale,
    media::library::MediaLibrary,
};
const PATH: &str = "/admin/banners";
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route(PATH, get(index))
        .route("/admin/banners/new", get(new).post(create))
        .route("/admin/banners/{id}", get(edit).post(update))
}
fn service(state: &AppState) -> Banners {
    Banners::new(state.pool.clone(), state.identity.clone())
}
fn library(state: &AppState) -> MediaLibrary {
    MediaLibrary::new(
        state.pool.clone(),
        state.identity.clone(),
        state.media.clone(),
    )
}
pub(super) struct Error(BannerError);
impl From<BannerError> for Error {
    fn from(error: BannerError) -> Self {
        Self(error)
    }
}
impl From<IdentityError> for Error {
    fn from(error: IdentityError) -> Self {
        Self(error.into())
    }
}
fn status(error: &BannerError) -> StatusCode {
    match error {
        BannerError::NotFound => StatusCode::NOT_FOUND,
        BannerError::Stale | BannerError::Limit => StatusCode::CONFLICT,
        BannerError::Content(error) => content::status(error),
        BannerError::Technical(error) => crate::access::technical_status(error),
        BannerError::Access(error) => crate::access::access_status(error),
        BannerError::Identity(error) => access::IdentityErrorResponse::status_for(error),
        BannerError::Media(_) => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
fn message(error: &BannerError) -> String {
    match error {
        BannerError::Content(error) => content::message(error),
        _ => i18n::text(match error {
            BannerError::NotFound => "banners-not-found",
            BannerError::Stale => "banners-stale",
            BannerError::Limit => "banners-limit",
            BannerError::Media(_) => "admin-media-storage-temporarily-unavailable",
            _ => "banners-invalid",
        }),
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self.0 {
            BannerError::Technical(error)
            | BannerError::Content(talocart_platform::content::ContentError::Technical(error)) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            BannerError::Access(error)
            | BannerError::Content(talocart_platform::content::ContentError::Access(error)) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            BannerError::Identity(error)
            | BannerError::Content(talocart_platform::content::ContentError::Identity(error)) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            error => (status(&error), message(&error)).into_response(),
        }
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
fn query() -> List {
    List {
        page: 1,
        saved: None,
    }
}
#[derive(Template)]
#[template(path = "banners.html")]
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
    entries: Vec<Banner>,
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
    error: Option<BannerError>,
) -> Result<Response, Error> {
    state
        .identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    let theme = state.storefront.settings().await?;
    let primary = Locale::parse(&theme.primary_locale).ok_or(IdentityError::Unavailable)?;
    let editing = editor.is_some();
    let (entries, total, page) = if editing {
        (vec![], 0, 1)
    } else {
        let r = service(state)
            .list(session, q.page, Language::new(primary, primary))
            .await?;
        (r.entries, r.total, r.page)
    };
    let (id, revision, input) = editor.unwrap_or_default();
    let rows = if id > 0 {
        match CONTENT
            .load(&state.pool, &state.identity, session, id)
            .await
        {
            Ok(rows) => rows,
            Err(talocart_platform::content::ContentError::NotFound)
                if submitted.is_some() && matches!(error, Some(BannerError::Stale)) =>
            {
                vec![]
            }
            Err(source_error) => return Err(BannerError::from(source_error).into()),
        }
    } else {
        vec![]
    };
    let content_error = if let Some(BannerError::Content(source_error)) = &error {
        Some(source_error)
    } else {
        None
    };
    let content = content::Editor::new(
        &theme.primary_locale,
        &theme.active_locales,
        CONTENT,
        rows,
        submitted,
        content_error,
        "banner-content",
    );
    let mut invalid = if let Some(BannerError::Invalid(fields)) = &error {
        fields.clone()
    } else {
        vec![]
    };
    if let Some(talocart_platform::content::ContentError::Invalid(fields)) = content_error {
        invalid.extend(
            fields
                .iter()
                .filter(|(locale, _)| locale == &content.primary)
                .map(|(_, f)| *f),
        );
    }
    let name = current_name(state).await?;
    let admin_shell = AdminShell::with_settings(state, session, name, theme).await?;
    let name = admin_shell.store_name;
    let mut r = render(AdminPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "banners",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        can_write: session.allows(Permission::SettingsWrite),
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
            &i18n::text("banners-title"),
            &i18n::text("banners-title"),
            |p| format!("{PATH}?page={p}"),
        ),
        error: error.as_ref().map(message).unwrap_or_default(),
        invalid,
    });
    if let Some(source_error) = error {
        *r.status_mut() = status(&source_error);
    }
    Ok(r)
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<List>,
) -> Result<Response, Error> {
    response(&state, &session, q, None, None, None).await
}
async fn new(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, Error> {
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
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
    let entry = service(&state)
        .get(&session, id, Language::new(primary, primary))
        .await?;
    response(
        &state,
        &session,
        q,
        Some((id, entry.revision, entry.input())),
        None,
        None,
    )
    .await
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Save,
    Delete,
    Picker(PickerAction),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerAction {
    Browse,
    Search,
    Next,
    Previous,
    Close,
    Choose(i64),
    Navigate,
}
impl Action {
    fn parse(value: &str) -> Result<Self, BannerError> {
        Ok(match value {
            "" | "save" => Self::Save,
            "delete" => Self::Delete,
            "browse" => Self::Picker(PickerAction::Browse),
            "media_search" => Self::Picker(PickerAction::Search),
            "media_next" => Self::Picker(PickerAction::Next),
            "media_previous" => Self::Picker(PickerAction::Previous),
            "media_close" => Self::Picker(PickerAction::Close),
            _ if value.starts_with("media_folder_") || value.starts_with("media_folders_") => {
                Self::Picker(PickerAction::Navigate)
            }
            _ => {
                let id = value
                    .strip_prefix("choose_")
                    .and_then(|v| v.parse::<i64>().ok())
                    .filter(|id| *id > 0)
                    .ok_or(BannerError::Invalid(vec!["action"]))?;
                Self::Picker(PickerAction::Choose(id))
            }
        })
    }
}
async fn apply_picker(
    state: &AppState,
    session: &Session,
    action: PickerAction,
    input: &mut Input,
    form: &mut content::Submission,
) -> Result<(), Error> {
    state
        .identity
        .authorize(session, Permission::MediaRead)
        .await?;
    if action == PickerAction::Navigate {
        let action = form.get("action").cloned().unwrap_or_default();
        super::media_picker::navigate(&action, |k, v| {
            form.insert(k.into(), v);
        })?;
    }
    let page = form
        .get("media_page")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(1)
        .clamp(1, 1_000_000);
    let page = match action {
        PickerAction::Next => (page + 1).min(1_000_000),
        PickerAction::Previous => (page - 1).max(1),
        PickerAction::Search | PickerAction::Browse => 1,
        _ => page,
    };
    form.insert("media_page".into(), page.to_string());
    form.insert(
        "picker".into(),
        (!matches!(action, PickerAction::Close | PickerAction::Choose(_))).to_string(),
    );
    if let PickerAction::Choose(selected) = action {
        let asset = library(state)
            .asset(session, selected, false)
            .await
            .map_err(|_| BannerError::Invalid(vec!["image_id"]))?;
        if asset.trashed || !asset.is_image() || asset.owner_id.is_some() {
            return Err(BannerError::Invalid(vec!["image_id"]).into());
        }
        input.image_id = selected.to_string();
        form.insert("image_id".into(), input.image_id.clone());
    }
    Ok(())
}

pub(super) async fn save(
    state: AppState,
    session: Session,
    id: i64,
    mut form: content::Submission,
) -> Result<Response, Error> {
    let field = |key: &str| form.get(key).cloned().unwrap_or_default();
    let csrf = field("csrf");
    session.check_csrf(&csrf)?;
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    let revision = field("revision")
        .parse::<i64>()
        .map_err(|_| BannerError::Invalid(vec!["revision"]))?;
    let action = Action::parse(&field("action"))?;
    let mut input = Input {
        label: field("label"),
        name: field("name"),
        description: field("description"),
        image_id: field("image_id"),
        active: field("active") == "true",
        sort_order: field("sort_order"),
    };
    if action == Action::Delete {
        return match service(&state).delete(&session, &csrf, id, revision).await {
            Ok(()) => Ok(Redirect::to("/admin/banners?saved=1").into_response()),
            Err(BannerError::Stale) => {
                response(
                    &state,
                    &session,
                    query(),
                    None,
                    None,
                    Some(BannerError::Stale),
                )
                .await
            }
            Err(error) => Err(error.into()),
        };
    }
    if let Action::Picker(action) = action {
        apply_picker(&state, &session, action, &mut input, &mut form).await?;
        if form.get("picker").is_some_and(|value| value == "true") {
            return super::media::draft_page(
                &state,
                &session,
                super::media_picker::Draft::banner(id, form),
            )
            .await
            .map_err(|_| IdentityError::Unavailable.into());
        }
        return response(
            &state,
            &session,
            query(),
            Some((id, revision, input)),
            Some(&form),
            None,
        )
        .await;
    }
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
        Err(
            error @ (BannerError::Identity(_)
            | BannerError::Technical(_)
            | BannerError::Access(_)
            | BannerError::Content(
                talocart_platform::content::ContentError::Identity(_)
                | talocart_platform::content::ContentError::Technical(_)
                | talocart_platform::content::ContentError::Access(_),
            )),
        ) => Err(error.into()),
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
    save(state, session, id, form).await
}

pub struct Slide {
    pub image: String,
    pub srcset: String,
    pub width: i32,
    pub height: i32,
    pub alt: String,
    pub href: String,
}
pub async fn slides(state: &AppState) -> Result<Vec<Slide>, IdentityError> {
    Ok(service(state)
        .public(store_language::content_language())
        .await
        .map_err(|_| IdentityError::Unavailable)?
        .into_iter()
        .map(|b| Slide {
            image: b.image(),
            srcset: b.srcset(),
            width: b.width,
            height: b.height,
            alt: b.name,
            href: store_language::banner_path(&b.description),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn nested_service_failures_preserve_status_retry_and_translation() {
        use talocart_platform::{
            content::ContentError,
            technical::{AccessError, TechnicalError},
        };
        for locale in [Locale::ENGLISH, Locale::ITALIAN] {
            i18n::scope(locale, async {
                for nested in [false, true] {
                    let causes = [
                        (
                            ContentError::Technical(TechnicalError::Busy),
                            StatusCode::SERVICE_UNAVAILABLE,
                            true,
                        ),
                        (
                            ContentError::Access(AccessError::Forbidden),
                            StatusCode::FORBIDDEN,
                            false,
                        ),
                        (
                            ContentError::Access(AccessError::Unauthorized),
                            StatusCode::UNAUTHORIZED,
                            false,
                        ),
                    ];
                    for (cause, status, busy) in causes {
                        let expected = content::message(&cause);
                        let error = if nested {
                            BannerError::Content(cause)
                        } else {
                            match cause {
                                ContentError::Technical(source_error) => {
                                    BannerError::Technical(source_error)
                                }
                                ContentError::Access(source_error) => {
                                    BannerError::Access(source_error)
                                }
                                _ => unreachable!("Only explicit service causes in this fixture"),
                            }
                        };
                        let response = Error(error).into_response();
                        assert_eq!(response.status(), status);
                        if busy {
                            assert_eq!(
                                response
                                    .headers()
                                    .get("retry-after")
                                    .and_then(|v| v.to_str().ok()),
                                Some("1")
                            );
                        }
                        let body = axum::body::to_bytes(response.into_body(), 4096)
                            .await
                            .unwrap();
                        assert_eq!(String::from_utf8(body.to_vec()).unwrap(), expected);
                    }
                }
            })
            .await;
        }
    }
    #[test]
    fn form_actions_are_explicit_and_image_ids_positive() {
        assert_eq!(Action::parse("save").unwrap(), Action::Save);
        assert_eq!(
            Action::parse("choose_42").unwrap(),
            Action::Picker(PickerAction::Choose(42))
        );
        for value in ["unknown", "choose_", "choose_0", "choose_-1"] {
            assert!(Action::parse(value).is_err());
        }
    }
}
