use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use talocart_commerce::attributes::{AttributeError, Entry, Kind, Values};
use talocart_commerce::content::Resource;
use talocart_platform::identity::IdentityError;

type Result<T> = std::result::Result<T, ErrorResponse>;
pub struct ErrorResponse(AttributeError);
impl From<AttributeError> for ErrorResponse {
    fn from(error: AttributeError) -> Self {
        Self(error)
    }
}
impl From<IdentityError> for ErrorResponse {
    fn from(error: IdentityError) -> Self {
        Self(error.into())
    }
}
fn status(error: &AttributeError) -> StatusCode {
    match error {
        AttributeError::Technical(error) => crate::access::technical_status(error),
        AttributeError::Access(error) => crate::access::access_status(error),
        AttributeError::Identity(error) => access::IdentityErrorResponse::status_for(error),
        AttributeError::Content(error) => content::status(error),
        AttributeError::NotFound => StatusCode::NOT_FOUND,
        AttributeError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::CONFLICT,
    }
}
fn message(error: &AttributeError) -> String {
    match error {
        AttributeError::Technical(error) => crate::i18n::technical_error(error),
        AttributeError::Access(error) => crate::i18n::access_error(error),
        AttributeError::Identity(error) => i18n::identity_error(error),
        AttributeError::Content(error) => content::message(error),
        _ => i18n::text(match error {
            AttributeError::NotFound => "attributes-not-found",
            AttributeError::Invalid => "attributes-invalid",
            AttributeError::InUse => "attributes-in-use",
            AttributeError::Limit => "attributes-limit",
            _ => "attributes-conflict",
        }),
    }
}
impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        if let AttributeError::Technical(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let AttributeError::Access(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        let mut r = (status(&self.0), message(&self.0)).into_response();
        if let AttributeError::Identity(error) = &self.0 {
            access::IdentityErrorResponse::retry_for(error, &mut r);
        }
        r
    }
}
#[derive(Clone, Copy)]
enum Target {
    Group,
    Attribute,
    Value,
}
impl Target {
    fn kind(self) -> Kind {
        if matches!(self, Self::Group) {
            Kind::Group
        } else {
            Kind::Attribute
        }
    }
    fn resource(self) -> Resource {
        if matches!(self, Self::Value) {
            Resource::ProductAttribute
        } else {
            self.kind().resource()
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Group => "attributes-groups",
            Self::Attribute => "attributes-title",
            Self::Value => "attributes-product-value",
        }
    }
    fn path(self, product: i64) -> String {
        if matches!(self, Self::Value) {
            format!("/admin/catalog/products/{product}/attributes")
        } else {
            match self {
                Self::Group => "/admin/catalog/attribute-groups",
                _ => "/admin/catalog/attributes",
            }
            .into()
        }
    }
}
pub fn routes() -> Router<AppState> {
    let mut router = Router::new();
    for target in [Target::Group, Target::Attribute, Target::Value] {
        let path = if matches!(target, Target::Value) {
            "/admin/catalog/products/{product}/attributes".to_owned()
        } else {
            target.path(0)
        };
        let mut r = Router::new()
            .route(&format!("{path}/new"), get(new_page).post(create))
            .route(&format!("{path}/{{id}}"), get(edit_page).post(update))
            .route(&format!("{path}/{{id}}/delete"), post(delete));
        if !matches!(target, Target::Value) {
            r = r.route(&path, get(index));
        }
        router = router.merge(r.layer(Extension(target)));
    }
    router
}
#[derive(Deserialize, Default)]
struct ListQuery {
    #[serde(default = "first")]
    page: i64,
    #[serde(default)]
    saved: String,
}
fn first() -> i64 {
    1
}
#[derive(Template)]
#[template(path = "attribute-list.html")]
struct ListPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    rows: Vec<Entry>,
    groups: bool,
    path: String,
    title: &'static str,
    can_write: bool,
    pagination: pagination::Pagination,
}
async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(target): Extension<Target>,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    if q.page < 1 {
        return Err(AttributeError::Invalid.into());
    }
    let state = state.primary_content().await?;
    let mut rows = state.attributes().entries(&session, target.kind()).await?;
    let total = rows.len() as i64;
    let page = q.page.min(((total + 19) / 20).max(1));
    rows = rows
        .into_iter()
        .skip(((page - 1) * 20) as usize)
        .take(20)
        .collect();
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    let path = target.path(0);
    Ok(render(ListPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "attributes",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        rows,
        groups: matches!(target, Target::Group),
        path: path.clone(),
        title: target.title(),
        can_write: session.allows(Permission::ProductsWrite),
        pagination: pagination::Pagination::new(
            total,
            page,
            20,
            &i18n::text(target.title()),
            &i18n::text("attributes-pages"),
            |p| format!("{path}?page={p}"),
        ),
    }))
}
type Params = BTreeMap<String, String>;
fn parameter(params: &Params, key: &str) -> Result<i64> {
    params.get(key).map_or(Ok(0), |s| {
        s.parse()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| AttributeError::Invalid.into())
    })
}
struct Choice {
    id: i64,
    name: String,
}
impl From<Entry> for Choice {
    fn from(entry: Entry) -> Self {
        Self {
            id: entry.id,
            name: entry.name,
        }
    }
}
#[derive(Template)]
#[template(path = "attribute-form.html")]
struct FormPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    id: i64,
    path: String,
    back: String,
    title: &'static str,
    group: bool,
    product_value: bool,
    values: Values,
    content: content::Editor,
    choices: Vec<Choice>,
    can_write: bool,
    error: String,
    saved: bool,
    choice_error: bool,
    order_error: bool,
    name_error: bool,
}
struct Feedback<'a> {
    pending: Option<&'a content::Submission>,
    error: Option<&'a AttributeError>,
    saved: bool,
}
async fn form_page(
    state: &AppState,
    session: &Session,
    target: Target,
    product: i64,
    id: i64,
    feedback: Feedback<'_>,
) -> Result<Response> {
    state
        .identity
        .authorize(session, Permission::ProductsRead)
        .await?;
    let state = state.primary_content().await?;
    let service = state.attributes();
    let assigned = if matches!(target, Target::Value) && id > 0 {
        Some(service.product_value(session, product, id).await?)
    } else {
        None
    };
    let mut values = if matches!(target, Target::Value) {
        if let Some(row) = &assigned {
            Values {
                name: row.name.clone(),
                sort_order: row.sort_order.to_string(),
                revision: row.revision.to_string(),
                attribute_id: row.attribute_id.to_string(),
                ..Values::default()
            }
        } else {
            Values {
                sort_order: "0".into(),
                ..Values::default()
            }
        }
    } else if id > 0 {
        let row = service.entry(session, target.kind(), id).await?;
        Values {
            name: row.name,
            sort_order: row.sort_order.to_string(),
            revision: row.revision.to_string(),
            group_id: row.group_id.map(|n| n.to_string()).unwrap_or_default(),
            ..Values::default()
        }
    } else {
        Values {
            sort_order: "0".into(),
            ..Values::default()
        }
    };
    if let Some(pending) = feedback.pending {
        values = submitted_values(pending);
        // Disabled controls are absent from native submissions. Identity is immutable
        // and must come from the owned database row, even if the client forges a value.
        if let Some(row) = &assigned {
            values.attribute_id = row.attribute_id.to_string();
        }
    }
    let choices = if matches!(target, Target::Value) {
        if let Some(row) = assigned {
            vec![Choice {
                id: row.attribute_id,
                name: row.attribute_name,
            }]
        } else {
            service
                .available_entries(session, product)
                .await?
                .into_iter()
                .map(Choice::from)
                .collect()
        }
    } else if matches!(target, Target::Attribute) {
        service
            .entries(session, Kind::Group)
            .await?
            .into_iter()
            .map(Choice::from)
            .collect()
    } else {
        vec![]
    };
    let content_error = if let Some(AttributeError::Content(source_error)) = feedback.error {
        Some(source_error)
    } else {
        None
    };
    let content = content::Editor::load(
        &state,
        session,
        target.resource(),
        id,
        feedback.pending,
        content_error,
        "attribute-content",
    )
    .await?;
    let admin_shell = AdminShell::load(&state, session).await?;
    let name = admin_shell.store_name;
    let invalid = matches!(feedback.error, Some(AttributeError::Invalid));
    let order_error = invalid && talocart_commerce::attributes::order(&values.sort_order).is_err();
    let limit = target.resource().definition().name_limit;
    let name_error = feedback.error.is_some()
        && (values.name.trim().is_empty()
            || values.name.trim().chars().count() > limit
            || values.name.chars().any(char::is_control));
    let mut response = render(FormPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: if matches!(target, Target::Value) {
            "products"
        } else {
            "attributes"
        },
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        id,
        path: target.path(product),
        back: if matches!(target, Target::Value) {
            format!("/admin/catalog/products/{product}?tab=attributes#panel-attributes")
        } else {
            target.path(0)
        },
        title: target.title(),
        group: matches!(target, Target::Group),
        product_value: matches!(target, Target::Value),
        choice_error: invalid && !order_error && !name_error,
        order_error,
        name_error,
        values,
        content,
        choices,
        can_write: session.allows(Permission::ProductsWrite),
        error: feedback.error.map(message).unwrap_or_default(),
        saved: feedback.saved,
    });
    if let Some(error) = feedback.error {
        *response.status_mut() = status(error);
    }
    Ok(response)
}
async fn new_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(target): Extension<Target>,
    params: Option<Path<Params>>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    let params = params.map(|p| p.0).unwrap_or_default();
    form_page(
        &state,
        &session,
        target,
        parameter(&params, "product")?,
        0,
        Feedback {
            pending: None,
            error: None,
            saved: false,
        },
    )
    .await
}
async fn edit_page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(target): Extension<Target>,
    Path(params): Path<Params>,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    form_page(
        &state,
        &session,
        target,
        parameter(&params, "product")?,
        parameter(&params, "id")?,
        Feedback {
            pending: None,
            error: None,
            saved: q.saved == "1",
        },
    )
    .await
}
fn submitted_values(form: &content::Submission) -> Values {
    let value = |key: &str| form.get(key).cloned().unwrap_or_default();
    Values {
        name: value("name"),
        sort_order: value("sort_order"),
        revision: value("revision"),
        group_id: value("group_id"),
        attribute_id: value("attribute_id"),
    }
}
async fn save(
    state: AppState,
    session: Session,
    target: Target,
    params: Params,
    form: content::Submission,
    id: i64,
) -> Result<Response> {
    let values = submitted_values(&form);
    let csrf = form.get("csrf").map_or("", String::as_str);
    let draft = content::draft(&form);
    let product = parameter(&params, "product")?;
    let service = state.attributes();
    let result = if matches!(target, Target::Value) {
        service
            .save_product(
                &session,
                csrf,
                product,
                (id > 0).then_some(id),
                &values,
                draft.as_ref(),
            )
            .await
    } else {
        service
            .save(
                &session,
                csrf,
                target.kind(),
                (id > 0).then_some(id),
                &values,
                draft.as_ref(),
            )
            .await
    };
    match result {
        Ok(id) => {
            Ok(Redirect::to(&format!("{}/{id}?saved=1", target.path(product))).into_response())
        }
        Err(
            source_error @ (AttributeError::Identity(_)
            | AttributeError::Technical(_)
            | AttributeError::Access(_)),
        ) => Err(source_error.into()),
        Err(source_error) => {
            form_page(
                &state,
                &session,
                target,
                product,
                id,
                Feedback {
                    pending: Some(&form),
                    error: Some(&source_error),
                    saved: false,
                },
            )
            .await
        }
    }
}
async fn create(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(target): Extension<Target>,
    params: Option<Path<Params>>,
    Form(form): Form<content::Submission>,
) -> Result<Response> {
    save(
        state,
        session,
        target,
        params.map(|p| p.0).unwrap_or_default(),
        form,
        0,
    )
    .await
}
async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(target): Extension<Target>,
    Path(params): Path<Params>,
    Form(form): Form<content::Submission>,
) -> Result<Response> {
    let id = parameter(&params, "id")?;
    save(state, session, target, params, form, id).await
}
async fn delete(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(target): Extension<Target>,
    Path(params): Path<Params>,
    Form(form): Form<content::Submission>,
) -> Result<Response> {
    let id = parameter(&params, "id")?;
    let product = parameter(&params, "product")?;
    let csrf = form.get("csrf").map_or("", String::as_str);
    // Authentication/CSRF precede field validation, including deletion confirmation.
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(csrf)?;
    let expected = form
        .get("revision")
        .and_then(|session| session.parse::<i64>().ok())
        .filter(|n| *n >= 0);
    let result = if form.get("confirm").is_none_or(|v| v != "true") || expected.is_none() {
        Err(AttributeError::Invalid)
    } else if matches!(target, Target::Value) {
        state
            .attributes()
            .remove_product(&session, csrf, product, id, expected.unwrap_or(0))
            .await
    } else {
        state
            .attributes()
            .delete(&session, csrf, target.kind(), id, expected.unwrap_or(0))
            .await
    };
    match result {
        Ok(()) => Ok(Redirect::to(&if matches!(target, Target::Value) {
            format!("/admin/catalog/products/{product}?tab=attributes#panel-attributes")
        } else {
            target.path(0)
        })
        .into_response()),
        Err(
            source_error @ (AttributeError::Identity(_)
            | AttributeError::Technical(_)
            | AttributeError::Access(_)),
        ) => Err(source_error.into()),
        Err(source_error) => {
            form_page(
                &state,
                &session,
                target,
                product,
                id,
                Feedback {
                    pending: None,
                    error: Some(&source_error),
                    saved: false,
                },
            )
            .await
        }
    }
}
