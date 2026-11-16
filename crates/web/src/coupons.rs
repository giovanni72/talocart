use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_commerce::coupons::{Coupon, CouponError, Coupons, Input};
const PATH: &str = "/admin/settings/coupons";
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(PATH, get(index))
        .route(&format!("{PATH}/new"), get(new_page).post(create))
        .route(&format!("{PATH}/{{id}}"), get(edit).post(update))
}
struct Error(CouponError);
impl From<CouponError> for Error {
    fn from(error: CouponError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for Error {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self.0 {
            CouponError::Technical(error) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            CouponError::Access(error) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            CouponError::Identity(error) => {
                access::IdentityErrorResponse::from(error).into_response()
            }
            error => (status(&error), message(&error)).into_response(),
        }
    }
}
fn status(error: &CouponError) -> StatusCode {
    if matches!(error, CouponError::Stale) {
        StatusCode::CONFLICT
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    }
}
fn message(error: &CouponError) -> String {
    i18n::text(match error {
        CouponError::Stale => "coupon-stale",
        CouponError::Duplicate => "coupon-duplicate",
        _ => "coupon-invalid",
    })
}
fn service(state: &AppState) -> Coupons {
    Coupons::new(state.pool.clone(), state.identity.clone())
}
#[derive(Template)]
#[template(path = "coupons.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    can_write: bool,
    entries: Vec<Coupon>,
    pagination: super::pagination::Pagination,
    editing: bool,
    id: i64,
    revision: i64,
    input: Input,
    error: String,
    invalid: Vec<&'static str>,
    currency: String,
}
#[derive(Deserialize)]
struct List {
    #[serde(default = "first")]
    page: i64,
}
fn first() -> i64 {
    1
}
async fn render_page(
    state: &AppState,
    session: &Session,
    page: i64,
    editor: Option<(i64, i64, Input)>,
    error: Option<CouponError>,
) -> Result<Response, Error> {
    state
        .identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    let (entries, total, page) = if editor.is_none() {
        let result = service(state).list(session, page).await?;
        (result.entries, result.total, result.page)
    } else {
        (vec![], 0, 1)
    };
    let editing = editor.is_some();
    let (id, revision, input) = editor.unwrap_or_default();
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let currency = service(state).currency(session).await?;
    let code = error.as_ref().map(status).unwrap_or(StatusCode::OK);
    let mut r = render(Page {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "coupons",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        can_write: session.allows(Permission::SettingsWrite),
        entries,
        pagination: super::pagination::Pagination::new(
            total,
            page,
            20,
            &i18n::text("coupon-title"),
            &i18n::text("coupon-title"),
            |p| format!("{PATH}?page={p}"),
        ),
        editing,
        id,
        revision,
        input,
        invalid: match &error {
            Some(CouponError::Invalid(fields)) => fields.clone(),
            Some(CouponError::Duplicate) => vec!["code"],
            _ => vec![],
        },
        error: error.as_ref().map(message).unwrap_or_default(),
        currency,
    });
    *r.status_mut() = code;
    Ok(r)
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<List>,
) -> Result<Response, Error> {
    render_page(&state, &session, q.page, None, None).await
}
async fn new_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response, Error> {
    render_page(
        &state,
        &session,
        1,
        Some((
            0,
            0,
            Input {
                kind: "percent".into(),
                ..Input::default()
            },
        )),
        None,
    )
    .await
}
async fn edit(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    let c = service(&state).get(&session, id).await?;
    render_page(&state, &session, 1, Some((id, c.revision, c.input())), None).await
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Save {
    csrf: String,
    revision: String,
    code: String,
    kind: String,
    value: String,
    currency: String,
    expires_on: String,
    usage_limit: String,
    active: bool,
}
async fn save(
    state: AppState,
    session: Session,
    id: i64,
    submitted: Save,
) -> Result<Response, Error> {
    session.check_csrf(&submitted.csrf)?;
    let revision = submitted
        .revision
        .parse::<i64>()
        .map_err(|_| Error(CouponError::Invalid(vec!["revision"])))?;
    let input = Input {
        code: submitted.code,
        kind: submitted.kind,
        value: submitted.value,
        currency: submitted.currency,
        expires_on: submitted.expires_on,
        usage_limit: submitted.usage_limit,
        active: submitted.active,
    };
    match service(&state)
        .save(&session, &submitted.csrf, id, revision, &input)
        .await
    {
        Ok(id) => Ok(Redirect::to(&format!("{PATH}/{id}")).into_response()),
        Err(
            error @ (CouponError::Identity(_) | CouponError::Technical(_) | CouponError::Access(_)),
        ) => Err(Error(error)),
        Err(error) => {
            render_page(
                &state,
                &session,
                1,
                Some((id, revision, input)),
                Some(error),
            )
            .await
        }
    }
}
async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(submitted): Form<Save>,
) -> Result<Response, Error> {
    save(state, session, 0, submitted).await
}
async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(submitted): Form<Save>,
) -> Result<Response, Error> {
    save(state, session, id, submitted).await
}
