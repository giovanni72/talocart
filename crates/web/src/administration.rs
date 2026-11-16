use super::*;
use access::IdentityErrorResponse;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
};
use serde::Deserialize;
use talocart_platform::identity::{
    ADMINISTRATOR_PAGE_SIZE, Administrator, IdentityError, Permission, Role, RoleDefinition,
    SettingsField, SettingsInput,
};

type Result<T> = std::result::Result<T, IdentityErrorResponse>;
#[derive(Deserialize, Default)]
pub struct PageQuery {
    #[serde(default)]
    after: i64,
    #[serde(default = "first_page")]
    page: i64,
    saved: Option<String>,
}
fn first_page() -> i64 {
    1
}
#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage<'a> {
    currency: String,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    owner: bool,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    sections: Vec<SettingsSection<'a>>,
    regional: talocart_platform::regional::Settings,
    time_zones: Vec<String>,
    zone_known: bool,
    format_known: bool,
    date_format_label: String,
    regional_error: String,
    regional_invalid: bool,
    regional_saved: bool,
    regional_preview: String,
    error: String,
    saved: bool,
}
pub(super) struct SettingsSection<'a> {
    pub(super) title: &'static str,
    pub(super) description: &'static str,
    pub(super) fields: Vec<SettingsFieldView<'a>>,
}
pub(super) struct SettingsFieldView<'a> {
    pub(super) key: &'static str,
    pub(super) label: &'static str,
    pub(super) value: &'a str,
    pub(super) limit: usize,
    pub(super) required: bool,
    pub(super) input_type: &'static str,
    pub(super) autocomplete: &'static str,
    pub(super) error: String,
}
pub(super) fn settings_sections<'a>(
    values: &'a SettingsInput,
    error: Option<&IdentityError>,
) -> Vec<SettingsSection<'a>> {
    use SettingsField::*;
    let groups = [
        (
            "admin-store-identity",
            "admin-public-name-and-business-details",
            vec![Name, LegalName],
        ),
        (
            "admin-contacts",
            "admin-store-contact-details-separate-from-the-administrator-account",
            vec![ContactEmail, Phone],
        ),
        (
            "admin-address",
            "admin-store-address-you-can-complete-it-later",
            vec![
                AddressLine1,
                AddressLine2,
                PostalCode,
                City,
                Province,
                Country,
            ],
        ),
    ];
    groups.into_iter().map(|(title, description, fields)| SettingsSection {
        title, description,
        fields: fields.into_iter().map(|field| {
            let (label, input_type, autocomplete) = match field {
                Name => ("admin-store-name", "text", "organization"),
                LegalName => ("admin-legal-business-name", "text", "off"),
                ContactEmail => ("admin-contact-email", "email", "email"),
                Phone => ("admin-phone", "tel", "tel"),
                AddressLine1 => ("admin-address", "text", "address-line1"),
                AddressLine2 => ("admin-address-line-2", "text", "address-line2"),
                PostalCode => ("admin-postal-code", "text", "postal-code"),
                City => ("admin-city", "text", "address-level2"),
                Province => ("admin-province-region", "text", "address-level1"),
                Country => ("admin-country", "text", "country-name"),
            };
            let invalid = matches!(error, Some(IdentityError::InvalidSettings(fields)) if fields.contains(&field));
            let message = if !invalid { String::new() } else if field == ContactEmail {
                i18n::text("admin-enter-a-valid-email-of-up-to-254-characters-or-leave-the-field-blank")
            } else if field == Name {
                i18n::text("admin-the-name-must-contain-1-to-80-characters-without-control-characters")
            } else {
                i18n::count("admin-field-limit", field.limit() as i64)
            };
            SettingsFieldView {key: field.key(), label, value: values.value(field), limit: field.limit(),
                required: field == Name, input_type, autocomplete, error: message}
        }).collect(),
    }).collect()
}
async fn settings_response(
    state: &AppState,
    session: &Session,
    values: Option<&SettingsInput>,
    error: Option<&IdentityError>,
    saved: bool,
    regional_edit: Option<RegionalEdit>,
    regional_saved: bool,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::SettingsRead)
        .await?;
    let settings = state
        .identity
        .settings()
        .await?
        .ok_or(IdentityError::Unavailable)?;
    let values = values.unwrap_or(&settings.values);
    let time_zones =
        talocart_platform::regional::time_zones(&state.pool, &state.identity, session).await?;
    let (regional, regional_error, regional_status) = if let Some(edit) = regional_edit {
        (edit.values, edit.error, edit.status)
    } else {
        (
            talocart_platform::regional::settings(&state.pool, &state.identity, session).await?,
            String::new(),
            StatusCode::OK,
        )
    };
    let zone_known = time_zones.contains(&regional.time_zone);
    let format_known =
        talocart_platform::regional::DATE_FORMATS.contains(&regional.date_format.as_str());
    let date_format_label = i18n::text(match regional.date_format.as_str() {
        "dmy" => "regional-format-dmy",
        "mdy" => "regional-format-mdy",
        _ => "regional-format-ymd",
    });
    let admin_shell = AdminShell::with_settings(
        state,
        session,
        settings.values.name.clone(),
        state.storefront.settings().await?,
    )
    .await?;
    let response = render(SettingsPage {
        store_name: &settings.values.name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "settings",
        owner: session.allows(Permission::SettingsWrite),
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        sections: settings_sections(values, error),
        currency: settings.currency,
        regional,
        time_zones,
        zone_known,
        format_known,
        date_format_label,
        regional_error,
        regional_invalid: regional_status == StatusCode::UNPROCESSABLE_ENTITY,
        regional_saved,
        regional_preview: talocart_platform::regional::current_time(&state.pool).await?,
        error: error.map(i18n::identity_error).unwrap_or_default(),
        saved,
    });
    if regional_status != StatusCode::OK {
        return Ok((regional_status, response).into_response());
    }
    Ok(form_response(response, error))
}
struct RegionalEdit {
    values: talocart_platform::regional::Settings,
    error: String,
    status: StatusCode,
}
#[derive(Deserialize)]
pub struct RegionalForm {
    csrf: String,
    time_zone: String,
    date_format: String,
    revision: i64,
}
pub async fn regional_settings(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<RegionalForm>,
) -> Result<Response> {
    use talocart_platform::regional::{self, RegionalError};
    let error = match regional::save(
        &state.pool,
        &session,
        &form.csrf,
        &form.time_zone,
        &form.date_format,
        form.revision,
    )
    .await
    {
        Ok(()) => {
            return Ok(Redirect::to("/admin/settings?regional_saved=1#date-time").into_response());
        }
        Err(RegionalError::Technical(error)) => return Err(error.into()),
        Err(RegionalError::Access(error)) => return Err(error.into()),
        Err(RegionalError::Identity(error)) => return Err(error.into()),
        Err(RegionalError::Invalid) => (StatusCode::UNPROCESSABLE_ENTITY, "regional-invalid"),
        Err(RegionalError::Conflict) => (StatusCode::CONFLICT, "regional-conflict"),
    };
    settings_response(
        &state,
        &session,
        None,
        None,
        false,
        Some(RegionalEdit {
            values: regional::Settings {
                time_zone: form.time_zone,
                date_format: form.date_format,
                revision: form.revision,
            },
            status: error.0,
            error: i18n::text(error.1),
        }),
        false,
    )
    .await
}
#[derive(Deserialize)]
pub struct SettingsForm {
    csrf: String,
    #[serde(flatten)]
    values: SettingsInput,
}
pub async fn settings(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<SettingsForm>,
) -> Result<Response> {
    match state
        .identity
        .save_settings(&session, &form.csrf, &form.values)
        .await
    {
        Ok(()) => Ok(Redirect::to("/admin/settings?saved=1").into_response()),
        Err(error) => {
            settings_response(
                &state,
                &session,
                Some(&form.values),
                Some(&error),
                false,
                None,
                false,
            )
            .await
        }
    }
}
#[derive(Template)]
#[template(path = "users.html")]
struct UsersPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    owner: bool,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    users: &'a [Administrator],
    pagination: super::pagination::Pagination,
    saved: bool,
}
pub async fn users_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<PageQuery>,
) -> Result<Response> {
    if query.after != 0 {
        let page = state
            .identity
            .administrator_cursor_page(&session, query.after)
            .await?;
        return Ok(Redirect::to(&format!("/admin/users?page={page}")).into_response());
    }
    let result = state
        .identity
        .administrators_counted(&session, query.page)
        .await?;
    let users = result.entries;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    Ok(render(UsersPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "users",
        owner: session.allows(Permission::UsersWrite),
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        users: &users,
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            ADMINISTRATOR_PAGE_SIZE as u16,
            &i18n::text("admin-administrators"),
            &i18n::text("admin-administrator-pages"),
            |page| format!("/admin/users?page={page}"),
        ),
        saved: query.saved.as_deref() == Some("1"),
    }))
}
#[derive(Template)]
#[template(path = "user-form.html")]
struct UserPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    owner: bool,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    id: i64,
    email: &'a str,
    role: &'a str,
    role_name: &'a str,
    roles: &'a [RoleDefinition],
    active: bool,
    error: &'a str,
}
async fn user_response(
    state: &AppState,
    session: &Session,
    user: &Administrator,
    error: Option<&IdentityError>,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::UsersRead)
        .await?;
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let message = match error {
        Some(IdentityError::Invalid) => {
            i18n::text("admin-check-email-role-and-password-12-to-128-characters")
        }
        Some(source_error) => i18n::identity_error(source_error),
        None => String::new(),
    };
    let roles = state.identity.assignable_roles(session).await?;
    let can_edit = roles.iter().any(|r| r.key == user.role) || user.id == 0;
    let response = render(UserPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "users",
        owner: session.allows(Permission::UsersWrite) && can_edit,
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        id: user.id,
        role_name: &user.role_name,
        roles: &roles,
        email: &user.email,
        role: &user.role,
        active: user.active,
        error: &message,
    });
    Ok(form_response(response, error))
}
pub(crate) fn form_response(mut response: Response, error: Option<&IdentityError>) -> Response {
    if response.status() == StatusCode::OK
        && let Some(error) = error
    {
        *response.status_mut() = IdentityErrorResponse::status_for(error);
        IdentityErrorResponse::retry_for(error, &mut response);
    }
    response
}
pub async fn new_user_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::UsersWrite)
        .await?;
    user_response(
        &state,
        &session,
        &Administrator {
            id: 0,
            email: String::new(),
            role: "viewer".into(),
            role_name: "Lettore".into(),
            active: true,
        },
        None,
    )
    .await
}
pub async fn edit_user_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let user = state.identity.administrator(&session, id).await?;
    user_response(&state, &session, &user, None).await
}
#[derive(Deserialize)]
pub struct CreateUserForm {
    csrf: String,
    email: String,
    password: String,
    role: String,
}
pub async fn create_user(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(form): Form<CreateUserForm>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::UsersWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    let result = match Role::parse(&form.role) {
        Ok(role) => state
            .identity
            .create_administrator(&session, &form.csrf, &form.email, form.password, role)
            .await
            .map(|_| ()),
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => Ok(Redirect::to("/admin/users?saved=1").into_response()),
        Err(error) => {
            user_response(
                &state,
                &session,
                &Administrator {
                    id: 0,
                    email: form.email,
                    role: form.role,
                    role_name: String::new(),
                    active: true,
                },
                Some(&error),
            )
            .await
        }
    }
}
#[derive(Deserialize)]
pub struct UpdateUserForm {
    csrf: String,
    role: String,
    active: bool,
}
pub async fn update_user(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<UpdateUserForm>,
) -> Result<Response> {
    let mut user = state.identity.administrator(&session, id).await?;
    session.check_csrf(&form.csrf)?;
    let result = match Role::parse(&form.role) {
        Ok(role) => {
            state
                .identity
                .update_administrator(&session, &form.csrf, id, role, form.active)
                .await
        }
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => {
            let location = match state
                .identity
                .authorize(&session, Permission::UsersWrite)
                .await
            {
                Ok(_) => "/admin/users?saved=1",
                Err(IdentityError::Forbidden) => "/admin/login",
                Err(error) => return Err(error.into()),
            };
            Ok(Redirect::to(location).into_response())
        }
        Err(error) => {
            user.role = form.role;
            user.active = form.active;
            user_response(&state, &session, &user, Some(&error)).await
        }
    }
}
