use super::*;
use axum::{
    extract::{Form, Path, Query},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;
use talocart_platform::identity::customer_groups::{
    CustomerGroup, CustomerGroups, GroupError, GroupInput, MAX_QUANTITY_TIERS, Member,
    MemberFilters, MemberOrder, MemberSort, QuantityTierInput,
};
type Result<T> = std::result::Result<T, GroupResponseError>;
struct GroupResponseError(GroupError);
impl From<GroupError> for GroupResponseError {
    fn from(error: GroupError) -> Self {
        Self(error)
    }
}
impl From<talocart_platform::identity::IdentityError> for GroupResponseError {
    fn from(error: talocart_platform::identity::IdentityError) -> Self {
        Self(error.into())
    }
}
impl IntoResponse for GroupResponseError {
    fn into_response(self) -> Response {
        if let GroupError::Technical(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let GroupError::Access(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        if let GroupError::Identity(error) = self.0 {
            return access::IdentityErrorResponse::from(error).into_response();
        }
        (status(&self.0), message(&self.0)).into_response()
    }
}
fn status(error: &GroupError) -> StatusCode {
    match error {
        GroupError::NotFound => StatusCode::NOT_FOUND,
        GroupError::Stale | GroupError::InUse => StatusCode::CONFLICT,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
fn message(error: &GroupError) -> String {
    i18n::text(match error {
        GroupError::Stale => "cg-stale",
        GroupError::NotFound => "cg-not-found",
        GroupError::InUse => "cg-in-use",
        GroupError::Duplicate => "cg-duplicate",
        GroupError::Capacity => "cg-capacity",
        _ => "cg-invalid",
    })
}
const PATH: &str = "/admin/customer-groups";
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/customers", get(members))
        .route("/admin/customers/{id}", get(member_edit).post(assign))
        .route(PATH, get(index))
        .route(&format!("{PATH}/new"), get(new_page).post(create))
        .route(&format!("{PATH}/{{id}}"), get(edit).post(update))
        .route(&format!("{PATH}/{{id}}/delete"), post(delete))
}
fn service(state: &AppState) -> CustomerGroups {
    CustomerGroups::new(state.pool.clone(), state.identity.clone())
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
#[template(path = "cg-list.html")]
struct ListPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    entries: Vec<CustomerGroup>,
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
        section: "customer_groups",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        can_write: session.allows(Permission::CustomersWrite),
        entries: result.entries,
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            20,
            &i18n::text("cg-title"),
            &i18n::text("cg-pages"),
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
#[template(path = "cg-form.html")]
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
    is_default: bool,
    persisted_default: bool,
    quantity_discounts_enabled: bool,
    quantity_discounts_for_guests: bool,
    quantity_tiers: Vec<QuantityTierInput>,
    tiers_invalid: bool,
    can_add_tier: bool,
}
async fn form(
    state: &AppState,
    session: &Session,
    id: i64,
    revision: i64,
    input: GroupInput,
    error: Option<GroupError>,
    persisted_default: bool,
) -> Result<Response> {
    let invalid = |key| {
        matches!(&error,Some(GroupError::Invalid(fields)) if fields.contains(&key))
            || (matches!(&error, Some(GroupError::Duplicate))
                && matches!(key, "name_it" | "name_en"))
    };
    let fields = [
        ("name_it", &input.name_it, 120),
        ("name_en", &input.name_en, 120),
    ]
    .into_iter()
    .map(|(key, value, max)| Field {
        name: key,
        value: value.clone(),
        label: i18n::text(&format!("cg-{key}")),
        invalid: invalid(key),
        error: i18n::text("cg-name-error"),
        max,
    })
    .collect();
    let admin_shell = AdminShell::load(state, session).await?;
    let name = admin_shell.store_name;
    let code = error.as_ref().map(status).unwrap_or(StatusCode::OK);
    let mut r = render(FormPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "customer_groups",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        can_write: session.allows(Permission::CustomersWrite),
        id,
        revision,
        fields,
        error: error.as_ref().map(message).unwrap_or_default(),
        action: if id == 0 {
            format!("{PATH}/new")
        } else {
            format!("{PATH}/{id}")
        },
        quantity_discounts_enabled: input.quantity_discounts_enabled,
        quantity_discounts_for_guests: input.quantity_discounts_for_guests,
        tiers_invalid: invalid("quantity_tiers"),
        can_add_tier: input.quantity_tiers.len() < MAX_QUANTITY_TIERS,
        quantity_tiers: input.quantity_tiers,
        is_default: input.is_default,
        persisted_default,
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
        .authorize(&session, Permission::CustomersWrite)
        .await?;
    let (revision, _) = service(&state).editor(&session, None).await?;
    form(
        &state,
        &session,
        0,
        revision,
        GroupInput::default(),
        None,
        false,
    )
    .await
}
async fn edit(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let (revision, entry) = service(&state).editor(&session, Some(id)).await?;
    let input = entry.ok_or(GroupError::NotFound)?.input();
    let persisted_default = input.is_default;
    form(
        &state,
        &session,
        id,
        revision,
        input,
        None,
        persisted_default,
    )
    .await
}
struct Submitted {
    csrf: String,
    revision: i64,
    input: GroupInput,
    confirm: bool,
    malformed: bool,
    action: String,
}
fn parse(fields: Vec<(String, String)>) -> Submitted {
    let mut values = std::collections::BTreeMap::new();
    let mut malformed = false;
    for (k, v) in fields {
        if ![
            "csrf",
            "revision",
            "name_it",
            "name_en",
            "is_default",
            "confirm",
            "action",
            "quantity_discounts_enabled",
            "quantity_discounts_for_guests",
        ]
        .contains(&k.as_str())
            && !["tier_quantity_", "tier_percent_"].iter().any(|prefix| {
                k.strip_prefix(prefix).is_some_and(|i| {
                    i.parse::<usize>()
                        .is_ok_and(|index| index < MAX_QUANTITY_TIERS && index.to_string() == i)
                })
            })
            || values.insert(k, v).is_some()
        {
            malformed = true;
        }
    }
    let is_default = values.remove("is_default");
    let confirm = values.remove("confirm");
    let enabled = values.remove("quantity_discounts_enabled");
    let guests = values.remove("quantity_discounts_for_guests");
    malformed |= guests.as_deref().is_some_and(|v| v != "true");
    let mut quantity_tiers = Vec::new();
    for i in 0..MAX_QUANTITY_TIERS {
        let min_quantity = values.remove(&format!("tier_quantity_{i}"));
        let percent = values.remove(&format!("tier_percent_{i}"));
        if min_quantity.is_some() || percent.is_some() {
            quantity_tiers.push(QuantityTierInput {
                min_quantity: min_quantity.unwrap_or_default(),
                percent: percent.unwrap_or_default(),
            });
        }
    }
    malformed |= enabled.as_deref().is_some_and(|v| v != "true");
    malformed |= is_default.as_deref().is_some_and(|v| v != "true")
        || confirm.as_deref().is_some_and(|v| v != "true");
    Submitted {
        csrf: values.remove("csrf").unwrap_or_default(),
        revision: values
            .remove("revision")
            .and_then(|v| v.parse().ok())
            .filter(|v| *v >= 0)
            .unwrap_or(-1),
        input: GroupInput {
            name_it: values.remove("name_it").unwrap_or_default(),
            name_en: values.remove("name_en").unwrap_or_default(),
            is_default: is_default.as_deref() == Some("true"),
            quantity_discounts_enabled: enabled.as_deref() == Some("true"),
            quantity_discounts_for_guests: guests.as_deref() == Some("true"),
            quantity_tiers,
        },
        confirm: confirm.as_deref() == Some("true"),
        malformed,
        action: values.remove("action").unwrap_or_else(|| "save".into()),
    }
}
async fn save(
    state: AppState,
    session: Session,
    id: Option<i64>,
    fields: Vec<(String, String)>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::CustomersWrite)
        .await?;
    let mut f = parse(fields);
    session.check_csrf(&f.csrf)?;
    if !f.malformed && f.revision >= 0 && f.action != "save" {
        let valid = if f.action == "add_tier" && f.input.quantity_tiers.len() < MAX_QUANTITY_TIERS {
            f.input.quantity_tiers.push(QuantityTierInput::default());
            true
        } else if let Some(index) = f
            .action
            .strip_prefix("remove_tier_")
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|i| *i < f.input.quantity_tiers.len())
        {
            f.input.quantity_tiers.remove(index);
            true
        } else {
            false
        };
        let (_, entry) = service(&state).editor(&session, id).await?;
        return form(
            &state,
            &session,
            id.unwrap_or(0),
            f.revision,
            f.input,
            (!valid).then_some(GroupError::Invalid(vec!["quantity_tiers"])),
            entry.is_some_and(|g| g.is_default),
        )
        .await;
    }
    let result = if f.revision < 0 {
        Err(GroupError::Stale)
    } else if f.malformed {
        Err(GroupError::Invalid(vec![]))
    } else {
        service(&state)
            .save(&session, &f.csrf, id, f.revision, &f.input)
            .await
    };
    match result {
        Ok(_) => Ok(Redirect::to(PATH).into_response()),
        Err(
            error @ (GroupError::Identity(_) | GroupError::Technical(_) | GroupError::Access(_)),
        )
        | Err(error @ GroupError::NotFound) => Err(error.into()),
        Err(error) => {
            let (_, entry) = service(&state).editor(&session, id).await?;
            form(
                &state,
                &session,
                id.unwrap_or(0),
                f.revision,
                f.input,
                Some(error),
                entry.is_some_and(|g| g.is_default),
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
        .authorize(&session, Permission::CustomersWrite)
        .await?;
    let f = parse(fields);
    session.check_csrf(&f.csrf)?;
    let result = if f.revision < 0 {
        Err(GroupError::Stale)
    } else if f.malformed || !f.confirm {
        Err(GroupError::Invalid(vec![]))
    } else {
        service(&state)
            .delete(&session, &f.csrf, id, f.revision)
            .await
    };
    match result {
        Ok(()) => Ok(Redirect::to(PATH).into_response()),
        Err(
            error @ (GroupError::Identity(_) | GroupError::Technical(_) | GroupError::Access(_)),
        )
        | Err(error @ GroupError::NotFound) => Err(error.into()),
        Err(error) => {
            let (_, entry) = service(&state).editor(&session, Some(id)).await?;
            let entry = entry.ok_or(GroupError::NotFound)?;
            form(
                &state,
                &session,
                id,
                f.revision,
                entry.input(),
                Some(error),
                entry.is_default,
            )
            .await
        }
    }
}

#[derive(Deserialize)]
struct MemberQuery {
    #[serde(default = "first_page")]
    page: i64,
    #[serde(flatten)]
    filters: MemberFilters,
}
struct MemberHeader {
    label: &'static str,
    href: String,
    sort: super::table_sort::SortState,
}
fn member_link(filters: &MemberFilters, order: MemberOrder) -> String {
    format!(
        "/admin/customers?name={}&email={}&group={}&kind={}&state={}&sort={}&dir={}&per_page={}",
        catalog::encoded(&filters.name),
        catalog::encoded(&filters.email),
        catalog::encoded(&filters.group),
        catalog::encoded(&filters.kind),
        catalog::encoded(&filters.state),
        order.column.key(),
        order.direction(),
        catalog::encoded(&filters.per_page)
    )
}
#[derive(Template)]
#[template(path = "customer-admin-list.html")]
struct MemberList<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    entries: Vec<Member>,
    can_write: bool,
    groups: Vec<CustomerGroup>,
    filters: MemberFilters,
    headers: Vec<MemberHeader>,
    sort: &'static str,
    direction: &'static str,
    page_size: u16,
    page_sizes: [u16; 4],
    filter_error: bool,
    filtered: bool,
    pagination: super::pagination::Pagination,
}
async fn members(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(mut q): Query<MemberQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let explicit_size = q.filters.per_page.clone();
    let preference = super::page_preference::AdminList::Customers;
    let size = preference.resolve(&explicit_size, &headers).ok();
    if let Some(size) = size {
        q.filters.per_page = size.value().to_string();
    }
    let (result, filter_error) = match service(&state)
        .filtered_members(&session, &q.filters, q.page, i18n::locale())
        .await
    {
        Ok(result) => (result, false),
        Err(GroupError::Invalid(_)) => (
            talocart_platform::pagination::Page {
                entries: vec![],
                total: 0,
                page: 1,
            },
            true,
        ),
        Err(error) => return Err(error.into()),
    };
    let order = MemberOrder::parse(&q.filters.sort, &q.filters.dir).unwrap_or_default();
    let page_size = q.filters.page_size().unwrap_or_default().value();
    let link = member_link(&q.filters, order);
    let headers = [
        (MemberSort::Name, "cl-name"),
        (MemberSort::Email, "cl-email"),
        (MemberSort::Group, "cg-group"),
        (MemberSort::Kind, "cl-kind"),
        (MemberSort::State, "cl-state"),
    ]
    .into_iter()
    .map(|(column, label)| {
        let sort = super::table_sort::SortState::new(
            column == order.column,
            order.descending,
            i18n::text(label),
        );
        MemberHeader {
            label,
            href: member_link(
                &q.filters,
                MemberOrder {
                    column,
                    descending: sort.next_descending,
                },
            ),
            sort,
        }
    })
    .collect();
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    let mut response = render(MemberList {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "customers",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        entries: result.entries,
        can_write: session.allows(Permission::CustomersWrite),
        groups: service(&state).choices(&session).await?,
        headers,
        sort: order.column.key(),
        direction: order.direction(),
        page_size,
        page_sizes: talocart_platform::pagination::ADMIN_PAGE_SIZES,
        filtered: q.filters.active(),
        filters: q.filters,
        filter_error,
        pagination: super::pagination::Pagination::new(
            result.total,
            result.page,
            page_size,
            &i18n::text("cg-customers"),
            &i18n::text("cg-customer-pages"),
            |page| format!("{link}&page={page}"),
        ),
    });
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if filter_error {
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    } else if let Some(size) = size {
        response = preference.remember(response, &explicit_size, size, state.secure_cookies)?;
    }
    Ok(response)
}
async fn member_form(
    state: &AppState,
    session: &Session,
    id: i64,
    submitted: Option<(i64, i64, GroupError)>,
) -> Result<Response> {
    Ok(
        match super::admin_customers::group_form(state, session, id, submitted).await {
            Ok(response) => response,
            Err(error) => error.into_response(),
        },
    )
}
async fn member_edit(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(q): Query<super::admin_customers::CardQuery>,
) -> Response {
    match super::admin_customers::card(&state, &session, id, q).await {
        Ok(r) => r,
        Err(error) => error.into_response(),
    }
}
async fn assign(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::CustomersWrite)
        .await?;
    let mut values = std::collections::BTreeMap::new();
    let mut malformed = false;
    for (key, value) in fields {
        if !["csrf", "revision", "group_id"].contains(&key.as_str())
            || values.insert(key, value).is_some()
        {
            malformed = true;
        }
    }
    let csrf = values.remove("csrf").unwrap_or_default();
    session.check_csrf(&csrf)?;
    let revision = values
        .remove("revision")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(-1);
    let group_id = values
        .remove("group_id")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    let result = if revision < 0 {
        Err(GroupError::Stale)
    } else if malformed {
        Err(GroupError::Invalid(vec![]))
    } else {
        service(&state)
            .assign(&session, &csrf, id, revision, group_id)
            .await
    };
    match result {
        Ok(()) => Ok(Redirect::to(&format!("/admin/customers/{id}")).into_response()),
        Err(
            error @ (GroupError::Identity(_) | GroupError::Technical(_) | GroupError::Access(_)),
        )
        | Err(error @ GroupError::NotFound) => Err(error.into()),
        Err(error) => member_form(&state, &session, id, Some((group_id, revision, error))).await,
    }
}

#[cfg(test)]
mod quantity_tests {
    use super::*;
    #[test]
    fn tier_form_rejects_duplicates_unknown_fields_and_invalid_switches() {
        let fields = || {
            vec![
                ("revision".into(), "0".into()),
                ("tier_quantity_0".into(), "10".into()),
                ("tier_percent_0".into(), "2,5".into()),
                ("quantity_discounts_enabled".into(), "true".into()),
            ]
        };
        let parsed = parse(fields());
        assert!(!parsed.malformed);
        assert!(parsed.input.quantity_discounts_enabled);
        assert_eq!(
            parsed.input.validated_tiers().unwrap()[0].discount_basis_points,
            250
        );
        for (key, value) in [
            ("tier_percent_0", "20"),
            ("tier_quantity_20", "1"),
            ("tier_quantity_01", "1"),
            ("quantity_discounts_enabled", "false"),
            ("quantity_discounts_for_guests", "false"),
            ("unknown", "1"),
        ] {
            let mut f = fields();
            f.push((key.into(), value.into()));
            assert!(parse(f).malformed, "{key}");
        }
    }
}
