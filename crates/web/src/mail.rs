use super::*;
use axum::extract::{Form, Path, Query};
use axum::response::Redirect;
use serde::Deserialize;
use talocart_commerce::notifications::{self, Notice};
use talocart_platform::mail::{Mail, MailError, Settings, TransportSettings};
type Result<T> = std::result::Result<T, Error>;
struct Error(MailError);
impl From<MailError> for Error {
    fn from(error: MailError) -> Self {
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
        let status = match self.0 {
            MailError::Technical(error) => {
                return access::IdentityErrorResponse::from(error).into_response();
            }
            MailError::Identity(error) => {
                return access::IdentityErrorResponse::from(error).into_response();
            }
            MailError::Stale | MailError::Conflict => StatusCode::CONFLICT,
            MailError::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        (status, i18n::text("mail-error")).into_response()
    }
}
const PATH: &str = "/admin/settings/email";
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(PATH, get(index).post(save))
        .route(
            &format!("{PATH}/transport"),
            axum::routing::post(save_transport),
        )
        .route(&format!("{PATH}/check"), axum::routing::post(check))
        .route(&format!("{PATH}/test"), axum::routing::post(test))
        .route(&format!("{PATH}/{{id}}/retry"), axum::routing::post(retry))
        .route(
            &format!("{PATH}/preview/{{template}}/{{locale}}"),
            get(preview),
        )
}
fn service(state: &AppState) -> Mail {
    Mail::new(state.pool.clone(), state.identity.clone())
}
#[derive(Deserialize, Default)]
struct Params {
    #[serde(default)]
    page: i64,
    #[serde(default)]
    attempts: i64,
    #[serde(default)]
    notice: String,
}
#[derive(Deserialize)]
struct SettingsForm {
    csrf: String,
    revision: i64,
    #[serde(default)]
    enabled: bool,
    from_name: String,
    from_email: String,
    #[serde(default)]
    reply_to: String,
    admin_recipients: String,
}
#[derive(Deserialize)]
struct CsrfForm {
    csrf: String,
}
#[derive(Deserialize)]
struct TestForm {
    csrf: String,
    key: String,
    recipient: String,
    template: String,
    locale: String,
}
struct Choice {
    key: &'static str,
    label: String,
}
struct Row {
    id: i64,
    recipient: String,
    subject: String,
    locale: String,
    state: String,
    attempts: i32,
    created_at: String,
    accepted_at: String,
    outcome: String,
    retry: bool,
}
struct Attempt {
    number: i32,
    started: String,
    finished: String,
    outcome: String,
}
#[derive(Template)]
#[template(path = "email.html")]
struct Page<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    can_write: bool,
    settings: Settings,
    admins: String,
    transport: TransportSettings,
    error: String,
    notice: String,
    entries: Vec<Row>,
    choices: Vec<Choice>,
    key: String,
    pagination: pagination::Pagination,
    attempts: Vec<Attempt>,
    selected: i64,
}
async fn page(
    state: &AppState,
    session: &Session,
    params: Params,
    edit: Option<(Settings, MailError)>,
    transport_edit: Option<(TransportSettings, MailError)>,
) -> Result<Response> {
    let mail = service(state);
    let mut settings = mail.settings(session).await?;
    let mut transport = mail.transport_settings(session).await?;
    let mut error = String::new();
    let mut status = StatusCode::OK;
    if let Some((values, source_error)) = edit {
        settings = values;
        error = i18n::text(if matches!(source_error, MailError::Stale) {
            "mail-stale"
        } else {
            "mail-error"
        });
        status = if matches!(source_error, MailError::Stale) {
            StatusCode::CONFLICT
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        };
    }
    if let Some((values, source_error)) = transport_edit {
        transport = values;
        let stale = matches!(source_error, MailError::Stale);
        error = i18n::text(if stale {
            "mail-stale"
        } else {
            "mail-transport-error"
        });
        status = if stale {
            StatusCode::CONFLICT
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        };
    }
    let log = mail.log(session, params.page).await?;
    let pagination = pagination::Pagination::new(
        log.total,
        log.page,
        20,
        &i18n::text("mail-log"),
        &i18n::text("mail-pages"),
        |p| format!("{PATH}?page={p}"),
    );
    let entries = log
        .entries
        .into_iter()
        .map(|entry| Row {
            id: entry.id,
            recipient: entry.recipient,
            subject: entry.subject,
            locale: entry.locale,
            attempts: entry.attempts,
            created_at: entry.created_at,
            accepted_at: entry.accepted_at,
            outcome: outcome(&entry.last_outcome),
            retry: entry.state == "failed",
            state: outcome(&entry.state),
        })
        .collect();
    let attempts = if params.attempts > 0 {
        mail.attempts(session, params.attempts)
            .await?
            .into_iter()
            .map(|a| Attempt {
                number: a.attempt,
                started: a.started_at,
                finished: a.finished_at,
                outcome: outcome(&a.outcome),
            })
            .collect()
    } else {
        Vec::new()
    };
    let notice = match params.notice.as_str() {
        "saved" => i18n::text("mail-saved"),
        "connected" => i18n::text("mail-connected"),
        "queued" => i18n::text("mail-queued"),
        "retried" => i18n::text("mail-retried"),
        "connection-error" => i18n::text("mail-connection-error"),
        _ => String::new(),
    };
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    Ok((
        status,
        render(Page {
            store_name: &name,
            logo: admin_shell.logo,
            csrf: admin_shell.csrf,
            section: "email",
            theme: admin_shell.theme,
            nav: admin_shell.nav,
            account: admin_shell.account,
            can_write: session.require(Permission::SettingsWrite).is_ok(),
            admins: settings.admin_recipients.join("\n"),
            settings,
            transport,
            error,
            notice,
            entries,
            choices: Notice::ALL
                .into_iter()
                .map(|n| Choice {
                    key: n.key(),
                    label: i18n::text(&format!("mail-template-{}", n.key().replace('_', "-"))),
                })
                .collect(),
            key: talocart_platform::mail::token()?,
            pagination,
            attempts,
            selected: params.attempts,
        }),
    )
        .into_response())
}
fn outcome(value: &str) -> String {
    let key = match value {
        "pending" => "mail-state-pending",
        "sending" => "mail-state-sending",
        "accepted" => "mail-state-accepted",
        "failed" => "mail-state-failed",
        "superseded" => "mail-state-superseded",
        "transient" => "mail-outcome-transient",
        "permanent" => "mail-outcome-permanent",
        "timeout" => "mail-outcome-timeout",
        "interrupted" => "mail-outcome-interrupted",
        "invalid" => "mail-outcome-invalid",
        _ => return String::new(),
    };
    i18n::text(key)
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(params): Query<Params>,
) -> Result<Response> {
    page(&state, &session, params, None, None).await
}
async fn save(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<SettingsForm>,
) -> Result<Response> {
    let values = Settings {
        revision: f.revision,
        enabled: f.enabled,
        from_name: f.from_name,
        from_email: f.from_email,
        reply_to: f.reply_to,
        admin_recipients: f
            .admin_recipients
            .lines()
            .map(str::trim)
            .filter(|session| !session.is_empty())
            .map(str::to_owned)
            .collect(),
    };
    match service(&state).save(&session, &f.csrf, &values).await {
        Ok(()) => Ok(Redirect::to(&format!("{PATH}?notice=saved")).into_response()),
        Err(error @ (MailError::Identity(_) | MailError::Technical(_))) => Err(error.into()),
        Err(error) => {
            page(
                &state,
                &session,
                Params::default(),
                Some((values, error)),
                None,
            )
            .await
        }
    }
}
async fn check(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<CsrfForm>,
) -> Result<Response> {
    service(&state).authorize_probe(&session, &f.csrf).await?;
    let key = state.mail_key.as_deref().ok_or(MailError::Key)?;
    let config = service(&state).smtp_config(key).await?;
    let notice = if config.check_connection().await.is_ok() {
        "connected"
    } else {
        "connection-error"
    };
    Ok(Redirect::to(&format!("{PATH}?notice={notice}")).into_response())
}
async fn test(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<TestForm>,
) -> Result<Response> {
    let notice = Notice::parse(&f.template).ok_or(MailError::Invalid)?;
    let content =
        notifications::render(&notifications::sample_for(notice)?, notice, &f.locale, true)?;
    service(&state)
        .test_message(&session, &f.csrf, &f.key, &f.recipient, &content)
        .await?;
    Ok(Redirect::to(&format!("{PATH}?notice=queued")).into_response())
}
async fn retry(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> Result<Response> {
    service(&state).retry(&session, &f.csrf, id).await?;
    Ok(Redirect::to(&format!("{PATH}?notice=retried")).into_response())
}
#[derive(Deserialize, Default)]
struct Preview {
    #[serde(default)]
    plain: bool,
}
async fn preview(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((template, locale)): Path<(String, String)>,
    Query(p): Query<Preview>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::SettingsRead)
        .await?;
    let notice = Notice::parse(&template).ok_or(MailError::Invalid)?;
    let content =
        notifications::render(&notifications::sample_for(notice)?, notice, &locale, true)?;
    Ok(([(header::CONTENT_TYPE,if p.plain{"text/plain; charset=utf-8"}else{"text/html; charset=utf-8"}),(header::CONTENT_SECURITY_POLICY,"sandbox; default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"),(header::CACHE_CONTROL,"no-store")],if p.plain{content.text}else{content.html}).into_response())
}

#[derive(Deserialize)]
struct TransportForm {
    csrf: String,
    revision: i64,
    #[serde(default)]
    enabled: bool,
    host: String,
    port: i32,
    security: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    clear_password: bool,
    timeout_seconds: i32,
}
async fn save_transport(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<TransportForm>,
) -> Result<Response> {
    // Authorize before touching the private key; service rechecks under the write guard.
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    let key = state.mail_key.as_deref().ok_or(MailError::Key)?;
    let mail = service(&state);
    let current = mail.transport_settings(&session).await?;
    let values = TransportSettings {
        revision: f.revision,
        enabled: f.enabled,
        host: f.host,
        port: f.port,
        security: f.security,
        username: f.username,
        has_password: current.has_password,
        timeout_seconds: f.timeout_seconds,
    };
    match mail
        .save_transport(
            &session,
            &f.csrf,
            &values,
            &f.password,
            f.clear_password,
            key,
        )
        .await
    {
        Ok(()) => Ok(Redirect::to(&format!("{PATH}?notice=saved")).into_response()),
        Err(error @ (MailError::Identity(_) | MailError::Technical(_))) => Err(error.into()),
        Err(error) => {
            page(
                &state,
                &session,
                Params::default(),
                None,
                Some((values, error)),
            )
            .await
        }
    }
}
