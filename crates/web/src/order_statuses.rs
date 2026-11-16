use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use talocart_commerce::order_statuses::{OrderStatus, OrderStatuses, StatusError, StatusInput};
type Result<T> = std::result::Result<T, StatusResponseError>;
struct StatusResponseError(StatusError);
impl From<StatusError> for StatusResponseError {
    fn from(error: StatusError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for StatusResponseError {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for StatusResponseError {
    fn into_response(self) -> Response {
        if let StatusError::Technical(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let StatusError::Access(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let StatusError::Identity(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        (status(&self.0), message(&self.0)).into_response()
    }
}
fn status(error: &StatusError) -> StatusCode {
    match error {
        StatusError::NotFound => StatusCode::NOT_FOUND,
        StatusError::Stale | StatusError::InUse => StatusCode::CONFLICT,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
fn message(error: &StatusError) -> String {
    i18n::text(match error {
        StatusError::Stale => "order-status-stale",
        StatusError::NotFound => "order-status-not-found",
        StatusError::InUse => "order-status-in-use",
        StatusError::Duplicate => "order-status-duplicate",
        StatusError::Capacity => "order-status-capacity",
        _ => "order-status-invalid",
    })
}
const PATH: &str = "/admin/settings/order-statuses";
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(PATH, get(index))
        .route(&format!("{PATH}/new"), get(new_page).post(create))
        .route(&format!("{PATH}/{{id}}"), get(edit).post(update))
        .route(&format!("{PATH}/{{id}}/delete"), post(delete))
}
fn service(state: &AppState) -> OrderStatuses {
    OrderStatuses::new(state.pool.clone(), state.identity.clone())
}
#[derive(Deserialize)]
struct ListQuery {
    #[serde(default = "first_page")]
    page: i64,
}
fn first_page() -> i64 {
    1
}
#[derive(Template)]
#[template(path = "order-status-list.html")]
struct ListPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    entries: Vec<OrderStatus>,
    can_write: bool,
    pagination: super::pagination::Pagination,
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let result = service(&state).list(&session, q.page).await?;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    Ok(render(ListPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "order_statuses",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        can_write: session.allows(Permission::SettingsWrite),
        entries: result.entries,
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            20,
            &i18n::text("order-status-title"),
            &i18n::text("order-status-pages"),
            |page| format!("{PATH}?page={page}"),
        ),
    }))
}
struct Field {
    name: &'static str,
    value: String,
    label: String,
    invalid: bool,
    max: usize,
    error: String,
}
#[derive(Template)]
#[template(path = "order-status-form.html")]
struct FormPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    can_write: bool,
    id: i64,
    revision: i64,
    fields: Vec<Field>,
    error: String,
    action: String,
    preview_name: String,
    preview_color: String,
    role_key: String,
}
async fn form(
    state: &AppState,
    session: &Session,
    id: i64,
    revision: i64,
    input: StatusInput,
    error: Option<StatusError>,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    let invalid = |key| {
        matches!(&error,Some(StatusError::Invalid(fields)) if fields.contains(&key))
            || (matches!(&error, Some(StatusError::Duplicate))
                && matches!(key, "name_it" | "name_en"))
    };
    let fields = [
        ("name_it", &input.name_it, 120),
        ("name_en", &input.name_en, 120),
        ("color", &input.color, 7),
        ("sort_order", &input.sort_order, 4),
    ]
    .into_iter()
    .map(|(key, value, max)| Field {
        name: key,
        value: value.clone(),
        label: i18n::text(&format!("order-status-{key}")),
        invalid: invalid(key),
        error: i18n::text(match key {
            "color" => "order-status-color-error",
            "sort_order" => "order-status-sort-error",
            _ => "order-status-name-error",
        }),
        max,
    })
    .collect();
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let code = error.as_ref().map(status).unwrap_or(StatusCode::OK);
    let role_key = if id > 0 {
        service(state)
            .editor(session, Some(id))
            .await?
            .1
            .map(|v| v.role_key().to_string())
            .unwrap_or_else(|| "status-role-custom".into())
    } else {
        "status-role-custom".into()
    };
    let mut r = render(FormPage {
        role_key,
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "order_statuses",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        can_write: session.allows(Permission::SettingsWrite),
        id,
        revision,
        fields,
        error: error.as_ref().map(message).unwrap_or_default(),
        action: if id == 0 {
            format!("{PATH}/new")
        } else {
            format!("{PATH}/{id}")
        },
        preview_name: if i18n::locale() == "it" {
            input.name_it
        } else {
            input.name_en
        },
        preview_color: if talocart_commerce::order_statuses::valid_color(&input.color) {
            input.color
        } else {
            "#64748b".into()
        },
    });
    *r.status_mut() = code;
    Ok(r)
}
async fn new_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    let (revision, _) = service(&state).editor(&session, None).await?;
    form(
        &state,
        &session,
        0,
        revision,
        StatusInput {
            color: "#64748b".into(),
            sort_order: "0".into(),
            ..Default::default()
        },
        None,
    )
    .await
}
async fn edit(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let (revision, entry) = service(&state).editor(&session, Some(id)).await?;
    let input = entry.ok_or(StatusError::NotFound)?.input();
    form(&state, &session, id, revision, input, None).await
}
struct Submitted {
    csrf: String,
    revision: i64,
    input: StatusInput,
    confirm: bool,
    malformed: bool,
}
fn parse(fields: Vec<(String, String)>) -> std::result::Result<Submitted, StatusError> {
    let mut values = std::collections::BTreeMap::new();
    let mut malformed = false;
    for (k, v) in fields {
        if ![
            "csrf",
            "revision",
            "name_it",
            "name_en",
            "color",
            "sort_order",
            "confirm",
        ]
        .contains(&k.as_str())
            || values.insert(k, v).is_some()
        {
            malformed = true;
        }
    }
    Ok(Submitted {
        csrf: values.remove("csrf").unwrap_or_default(),
        revision: values
            .remove("revision")
            .and_then(|v| v.parse().ok())
            .ok_or(StatusError::Stale)?,
        input: StatusInput {
            name_it: values.remove("name_it").unwrap_or_default(),
            name_en: values.remove("name_en").unwrap_or_default(),
            color: values.remove("color").unwrap_or_default(),
            sort_order: values.remove("sort_order").unwrap_or_default(),
        },
        confirm: values.remove("confirm").as_deref() == Some("true"),
        malformed,
    })
}
async fn save(
    state: AppState,
    session: Session,
    id: Option<i64>,
    fields: Vec<(String, String)>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    let f = parse(fields)?;
    session.check_csrf(&f.csrf)?;
    let result = if f.malformed {
        Err(StatusError::Invalid(vec![]))
    } else {
        service(&state)
            .save(&session, &f.csrf, id, f.revision, &f.input)
            .await
    };
    match result {
        Ok(_) => Ok(Redirect::to(PATH).into_response()),
        Err(
            error @ (StatusError::Identity(_) | StatusError::Technical(_) | StatusError::Access(_)),
        )
        | Err(error @ StatusError::NotFound) => Err(error.into()),
        Err(error) => {
            form(
                &state,
                &session,
                id.unwrap_or(0),
                f.revision,
                f.input,
                Some(error),
            )
            .await
        }
    }
}
async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    save(state, session, None, fields).await
}
async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    save(state, session, Some(id), fields).await
}
async fn delete(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    let f = parse(fields)?;
    session.check_csrf(&f.csrf)?;
    let result = if f.malformed || !f.confirm {
        Err(StatusError::Invalid(vec![]))
    } else {
        service(&state)
            .delete(&session, &f.csrf, id, f.revision)
            .await
    };
    match result {
        Ok(()) => Ok(Redirect::to(PATH).into_response()),
        Err(
            error @ (StatusError::Identity(_) | StatusError::Technical(_) | StatusError::Access(_)),
        )
        | Err(error @ StatusError::NotFound) => Err(error.into()),
        Err(error) => {
            let (_, entry) = service(&state).editor(&session, Some(id)).await?;
            form(
                &state,
                &session,
                id,
                f.revision,
                entry.ok_or(StatusError::NotFound)?.input(),
                Some(error),
            )
            .await
        }
    }
}
