use super::*;
use access::IdentityErrorResponse;
use axum::{
    extract::{Form, Multipart, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_platform::identity::{AVATAR_UPLOAD_LIMIT, Account, IdentityError};

type Result<T> = std::result::Result<T, IdentityErrorResponse>;
#[derive(Deserialize, Default)]
pub struct AccountQuery {
    saved: Option<String>,
}
#[derive(Template)]
#[template(path = "account.html")]
struct AccountPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    profile: &'a Account,
    name: &'a str,
    email: &'a str,
    error: &'a str,
    saved: bool,
}
async fn page_response(
    state: &AppState,
    session: &Session,
    values: Option<(&str, &str)>,
    error: Option<&IdentityError>,
    saved: bool,
) -> Result<Response> {
    let profile = state.identity.account(session).await?;
    let admin_shell = AdminShell::load(state, session).await?;
    let store_name = admin_shell.store_name;
    let (name, email) = values.unwrap_or((&profile.display_name, &profile.email));
    let message = error.map(i18n::identity_error).unwrap_or_default();
    let response = render(AccountPage {
        store_name: &store_name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "account",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        name,
        email,
        error: &message,
        saved,
        profile: &profile,
    });
    Ok(administration::form_response(response, error))
}
pub async fn page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<AccountQuery>,
) -> Result<Response> {
    page_response(
        &state,
        &session,
        None,
        None,
        query.saved.as_deref() == Some("1"),
    )
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileForm {
    csrf: String,
    display_name: String,
    email: String,
    #[serde(default)]
    current_password: String,
}
fn login_again(state: &AppState) -> Result<Response> {
    let mut response = Redirect::to("/admin/login").into_response();
    access::set_cookie(&mut response, "", state.secure_cookies, 0)?;
    Ok(response)
}
pub async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<ProfileForm>,
) -> Result<Response> {
    match state
        .identity
        .update_account(
            &session,
            &form.csrf,
            &form.display_name,
            &form.email,
            form.current_password,
        )
        .await
    {
        Ok(true) => login_again(&state),
        Ok(false) => Ok(Redirect::to("/admin/account?saved=1").into_response()),
        Err(error) => {
            page_response(
                &state,
                &session,
                Some((&form.display_name, &form.email)),
                Some(&error),
                false,
            )
            .await
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordForm {
    csrf: String,
    current_password: String,
    new_password: String,
    confirmation: String,
}
pub async fn password(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<PasswordForm>,
) -> Result<Response> {
    match state
        .identity
        .change_password(
            &session,
            &form.csrf,
            form.current_password,
            form.new_password,
            &form.confirmation,
        )
        .await
    {
        Ok(()) => login_again(&state),
        Err(error) => page_response(&state, &session, None, Some(&error), false).await,
    }
}
pub async fn avatar(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<images::ImageQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    Ok(
        match state.identity.avatar_image(&session, query.variant).await? {
            Some(image) => images::response(image, &headers, false),
            None => images::fallback(),
        },
    )
}
async fn read_upload(
    session: &Session,
    mut multipart: Multipart,
) -> std::result::Result<(Vec<u8>, String), IdentityError> {
    let mut csrf = None;
    let mut avatar = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| IdentityError::InvalidAvatar)?
    {
        match field.name() {
            Some("csrf") if csrf.is_none() => {
                let value = field
                    .text()
                    .await
                    .map_err(|_| IdentityError::InvalidAvatar)?;
                session.check_csrf(&value)?;
                csrf = Some(value);
            }
            Some("avatar") if avatar.is_none() => {
                let filename = field.file_name().unwrap_or("avatar.png").to_owned();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| IdentityError::InvalidAvatar)?;
                if bytes.is_empty() || bytes.len() > AVATAR_UPLOAD_LIMIT {
                    return Err(IdentityError::InvalidAvatar);
                }
                avatar = Some((bytes.to_vec(), filename));
            }
            _ => return Err(IdentityError::InvalidAvatar),
        }
    }
    csrf.ok_or(IdentityError::Forbidden)?;
    avatar.ok_or(IdentityError::InvalidAvatar)
}
pub async fn upload(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    multipart: Multipart,
) -> Result<Response> {
    let result = match read_upload(&session, multipart).await {
        Ok((raw, filename)) => {
            state
                .identity
                .update_avatar_named(&session, &session.csrf, Some(raw), &filename)
                .await
        }
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => Ok(Redirect::to("/admin/account?saved=1").into_response()),
        Err(error) => page_response(&state, &session, None, Some(&error), false).await,
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveAvatarForm {
    csrf: String,
}
pub async fn remove_avatar(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<RemoveAvatarForm>,
) -> Result<Response> {
    match state
        .identity
        .update_avatar(&session, &form.csrf, None)
        .await
    {
        Ok(()) => Ok(Redirect::to("/admin/account?saved=1").into_response()),
        Err(error) => page_response(&state, &session, None, Some(&error), false).await,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageForm {
    csrf: String,
    locale: String,
}
pub async fn language(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<LanguageForm>,
) -> Result<Response> {
    match state
        .identity
        .update_locale(&session, &form.csrf, &form.locale)
        .await
    {
        Ok(()) => {
            let mut response = Redirect::to("/admin/account?saved=1").into_response();
            i18n::remember(&mut response, &form.locale, state.secure_cookies)?;
            Ok(response)
        }
        Err(error) => page_response(&state, &session, None, Some(&error), false).await,
    }
}
