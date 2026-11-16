use super::*;
use access::IdentityErrorResponse;
use axum::{
    extract::{Form, Multipart, Path, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_platform::identity::{BRANDING_UPLOAD_LIMIT, BrandingKind, IdentityError};

type Result<T> = std::result::Result<T, IdentityErrorResponse>;
fn kind(key: &str) -> std::result::Result<BrandingKind, IdentityError> {
    match key {
        "logo" => Ok(BrandingKind::Logo),
        "favicon" => Ok(BrandingKind::Favicon),
        _ => Err(IdentityError::NotFound),
    }
}
#[derive(Template)]
#[template(path = "branding.html")]
struct BrandingPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    owner: bool,
    images: Vec<ImageView>,
    saved: bool,
    error: String,
}
struct ImageView {
    key: &'static str,
    title: &'static str,
    description: &'static str,
    present: bool,
    version: i64,
    error: String,
}
async fn page_response(
    state: &AppState,
    session: &Session,
    saved: bool,
    failure: Option<(BrandingKind, &IdentityError)>,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let branding = state
        .identity
        .branding()
        .await?
        .ok_or(IdentityError::Unavailable)?;
    let images = [
        (
            BrandingKind::Logo,
            "admin-store-logo",
            "admin-appears-in-the-store-and-administration-header-proportions-and-transpare",
            branding.has_logo,
            branding.logo_version,
        ),
        (
            BrandingKind::Favicon,
            "admin-favicon-title",
            "admin-browser-tab-icon-the-image-is-fitted-into-a-square-without-cropping",
            branding.has_favicon,
            branding.favicon_version,
        ),
    ]
    .into_iter()
    .map(|(kind, title, description, present, version)| ImageView {
        key: kind.key(),
        title,
        description,
        present,
        version,
        error: failure
            .filter(|(failed, _)| failed.key() == kind.key())
            .map(|(_, source_error)| i18n::identity_error(source_error))
            .unwrap_or_default(),
    })
    .collect();
    let error = failure.map(|(_, source_error)| source_error);
    let response = render(BrandingPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "settings",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        owner: session.allows(Permission::SettingsWrite),
        images,
        saved,
        error: error.map(i18n::identity_error).unwrap_or_default(),
    });
    Ok(administration::form_response(response, error))
}
#[derive(Deserialize)]
pub struct PageQuery {
    saved: Option<String>,
}
pub async fn page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<PageQuery>,
) -> Result<Response> {
    unified_settings::show(
        &state,
        &session,
        "appearance",
        query.saved.as_deref() == Some("1"),
    )
    .await
}
pub async fn image(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(query): Query<images::ImageQuery>,
) -> Result<Response> {
    let kind = kind(&key)?;
    match state.identity.branding_metadata(kind).await? {
        Some(metadata) => Ok((
            [(header::CACHE_CONTROL, "no-store")],
            Redirect::temporary(&format!(
                "/media/public/{}/{}/{}",
                metadata.id,
                metadata.key,
                query.variant.key()
            )),
        )
            .into_response()),
        None => Ok(images::fallback()),
    }
}
pub async fn public_image(
    State(state): State<AppState>,
    Path((id, path)): Path<(i64, String)>,
    headers: HeaderMap,
    method: axum::http::Method,
) -> Result<Response> {
    let Some((key, profile)) = path.rsplit_once('/') else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let variant = match profile {
        "original" => talocart_platform::media::ImageVariant::Original,
        "thumbnail" => talocart_platform::media::ImageVariant::Thumbnail,
        "card" => talocart_platform::media::ImageVariant::Card,
        "detail" => talocart_platform::media::ImageVariant::Detail,
        _ => return Ok(StatusCode::NOT_FOUND.into_response()),
    };
    let file = state
        .media
        .public_file(&state.pool, id, key, variant)
        .await
        .map_err(talocart_platform::identity::IdentityError::from)?;
    let Some(file) = file else {
        if let Some(url) = state
            .media
            .cache_fallback_url(&state.pool, id, key, variant)
            .await
            .map_err(talocart_platform::identity::IdentityError::from)?
        {
            return Ok((
                [(header::CACHE_CONTROL, "no-store")],
                Redirect::temporary(&url),
            )
                .into_response());
        }
        return Ok(images::fallback());
    };
    if variant != talocart_platform::media::ImageVariant::Original && file.key == key {
        return Ok((
            [(header::CACHE_CONTROL, "no-store")],
            Redirect::temporary(&format!(
                "/media/public/{id}/{}/original",
                key.replace(' ', "%20")
            )),
        )
            .into_response());
    }
    images::public_file_response(&state, file, &headers, method)
        .await
        .map_err(|error| talocart_platform::identity::IdentityError::from(error).into())
}
pub(crate) async fn read_upload(
    session: &Session,
    mut multipart: Multipart,
) -> std::result::Result<(Vec<u8>, String), IdentityError> {
    let mut csrf = None;
    let mut image = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| IdentityError::InvalidBranding)?
    {
        match field.name() {
            Some("csrf") if csrf.is_none() => {
                let value = field
                    .text()
                    .await
                    .map_err(|_| IdentityError::InvalidBranding)?;
                session.check_csrf(&value)?;
                csrf = Some(value);
            }
            Some("image") if image.is_none() => {
                let filename = field.file_name().unwrap_or("image.png").to_owned();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| IdentityError::InvalidBranding)?;
                if bytes.is_empty() || bytes.len() > BRANDING_UPLOAD_LIMIT {
                    return Err(IdentityError::InvalidBranding);
                }
                image = Some((bytes.to_vec(), filename));
            }
            _ => return Err(IdentityError::InvalidBranding),
        }
    }
    csrf.ok_or(IdentityError::Forbidden)?;
    image.ok_or(IdentityError::InvalidBranding)
}
pub async fn upload(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(key): Path<String>,
    multipart: Multipart,
) -> Result<Response> {
    let kind = kind(&key)?;
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    let result = match read_upload(&session, multipart).await {
        Ok((raw, filename)) => {
            state
                .identity
                .update_branding_named(&session, &session.csrf, kind, Some(raw), &filename)
                .await
        }
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => Ok(Redirect::to("/admin/settings/appearance?saved=1").into_response()),
        Err(error) => page_response(&state, &session, false, Some((kind, &error))).await,
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveForm {
    csrf: String,
}
pub async fn remove(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(key): Path<String>,
    Form(form): Form<RemoveForm>,
) -> Result<Response> {
    let kind = kind(&key)?;
    state
        .identity
        .authorize(&session, Permission::SettingsWrite)
        .await?;
    match state
        .identity
        .update_branding(&session, &form.csrf, kind, None)
        .await
    {
        Ok(()) => Ok(Redirect::to("/admin/settings/appearance?saved=1").into_response()),
        Err(error) => page_response(&state, &session, false, Some((kind, &error))).await,
    }
}
