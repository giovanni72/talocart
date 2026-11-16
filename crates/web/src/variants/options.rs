//! Option definitions and value editing, including search and pagination.
use super::*;
use talocart_commerce::variants::ValueSearch;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/catalog/options", get(options))
        .route(
            "/admin/catalog/options/new",
            get(new_option).post(create_option),
        )
        .route(
            "/admin/catalog/options/{id}",
            get(edit_option).post(update_option),
        )
        .route("/admin/catalog/options/{id}/delete", post(delete_option))
        .route("/admin/catalog/options/{id}/values", post(create_value))
        .route(
            "/admin/catalog/options/{id}/values/{value}",
            post(update_value),
        )
        .route(
            "/admin/catalog/options/{id}/values/{value}/delete",
            post(delete_value),
        )
}

#[derive(Template)]
#[template(path = "option-list.html")]
struct OptionList<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    rows: Vec<variants::OptionEntry>,
    can_write: bool,
    pagination: crate::pagination::Pagination,
    deleted: bool,
}
async fn options(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(q): Query<Page>,
) -> Result<Response> {
    let state = state.primary_content().await?;
    let result = state.variants.options_counted(&session, q.page).await?;
    let rows = result.entries;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    Ok(render(OptionList {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "options",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        rows,
        can_write: session.allows(Permission::ProductsWrite),
        pagination: crate::pagination::Pagination::new(
            result.total,
            result.page,
            20,
            &i18n::text("admin-options-2"),
            &i18n::text("admin-option-pages"),
            |page| format!("/admin/catalog/options?page={page}"),
        ),
        deleted: q.deleted == "1",
    }))
}
#[derive(Template)]
#[template(path = "option-form.html")]
struct OptionPage<'a> {
    content: content::Editor,
    new_content: content::Editor,
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    id: i64,
    name: String,
    new_value: String,
    sort_order: String,
    new_sort_order: String,
    order_error: bool,
    value_order_error: bool,
    rows: Vec<EditableValue>,
    can_write: bool,
    error: String,
    name_error: bool,
    value_error_id: i64,
    value_name_error: bool,
    saved: bool,
    value_deleted: bool,
    delete_error_id: i64,
    pagination: crate::pagination::Pagination,
    search: String,
    search_invalid: bool,
    search_error: String,
    search_results: String,
}
#[derive(Default)]
struct OptionFeedback<'a> {
    error: Option<&'a VariantError>,
    saved: bool,
    value_deleted: bool,
    deleting_value: Option<i64>,
    /// Raw search text from the URL or the submitted row form.
    search: &'a str,
}
/// Value search for the editor; invalid text is reported and not applied.
fn value_search(state: &AppState, text: &str) -> (ValueSearch, bool) {
    match ValueSearch::new(text, &state.content_active) {
        Ok(search) => (search, false),
        Err(_) => (ValueSearch::default(), true),
    }
}
/// Page of `value` within `search`. A value outside it is located in the full
/// list instead, and only then is the search cleared so the value stays visible;
/// a value missing everywhere leaves the search untouched for the caller.
async fn locate_value(
    state: &AppState,
    session: &Session,
    option: i64,
    value: i64,
    search: &mut ValueSearch,
) -> std::result::Result<i64, VariantError> {
    match state
        .variants
        .option_value_page(session, option, value, search)
        .await
    {
        Err(VariantError::NotFound) if !search.is_empty() => {
            let all = ValueSearch::default();
            let page = state
                .variants
                .option_value_page(session, option, value, &all)
                .await?;
            *search = all;
            Ok(page)
        }
        located => located,
    }
}
/// Query-string suffix that keeps the active value search across redirects and links.
fn search_suffix(search: &ValueSearch) -> String {
    if search.is_empty() {
        String::new()
    } else {
        format!("&q={}", crate::catalog::encoded(search.text()))
    }
}
struct EditableValue {
    id: i64,
    name: String,
    sort_order: String,
    content: content::Editor,
}
async fn option_page(
    state: &AppState,
    session: &Session,
    id: i64,
    pending: Option<(i64, &str, Option<&str>, &content::Submission)>,
    error: Option<&VariantError>,
    saved: bool,
) -> Result<Response> {
    option_page_at(
        state,
        session,
        id,
        pending,
        OptionFeedback {
            error,
            saved,
            ..OptionFeedback::default()
        },
        1,
    )
    .await
}
async fn option_page_at(
    state: &AppState,
    session: &Session,
    id: i64,
    pending: Option<(i64, &str, Option<&str>, &content::Submission)>,
    feedback: OptionFeedback<'_>,
    mut page: i64,
) -> Result<Response> {
    let error = feedback.error;
    let primary_state = state.primary_content().await?;
    let state = &primary_state;
    let (mut search, search_invalid) = value_search(state, feedback.search);
    let (mut name, mut sort_order) = if id > 0 {
        let option = state.variants.option(session, id).await?;
        (option.name, option.sort_order.to_string())
    } else {
        (String::new(), "0".into())
    };
    let mut new_sort_order = "0".to_string();
    let mut new_value = String::new();
    let mut value_error_id = 0;
    if let Some((field, value, order, _)) = pending {
        if field == 0 {
            name = value.into();
            if let Some(order) = order {
                sort_order = order.into();
            }
        } else if field == -1 {
            new_value = value.into();
            if let Some(order) = order {
                new_sort_order = order.into();
            }
            value_error_id = -1;
        } else {
            value_error_id = field;
            page = locate_value(state, session, id, field, &mut search).await?;
        }
    }
    let result = if id > 0 {
        if let Some(value) = feedback.deleting_value {
            state
                .variants
                .values_counted_at_value(session, id, value, page, &mut search)
                .await?
        } else {
            state
                .variants
                .values_counted(session, id, page, &search)
                .await?
        }
    } else {
        talocart_platform::pagination::Page {
            entries: vec![],
            total: 0,
            page: 1,
        }
    };
    let mut rows = result.entries;
    let delete_error_id = feedback
        .deleting_value
        .filter(|id| rows.iter().any(|row| row.id == *id))
        .unwrap_or(0);
    let error_message = if feedback.deleting_value.is_some() {
        match (error, rows.iter().find(|row| row.id == delete_error_id)) {
            (Some(VariantError::InUse | VariantError::Confirmation), Some(row)) => {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("name", row.name.as_str());
                i18n::with_args(
                    if matches!(error, Some(VariantError::InUse)) {
                        "options-value-delete-in-use"
                    } else {
                        "options-value-delete-confirm"
                    },
                    &args,
                )
            }
            (Some(VariantError::NotFound), _) => i18n::text("options-value-delete-not-found"),
            _ => error.map(i18n::variant_error).unwrap_or_default(),
        }
    } else {
        error.map(i18n::variant_error).unwrap_or_default()
    };
    if let Some((field, value, _, _)) = pending
        && let Some(row) = rows.iter_mut().find(|r| r.id == field)
    {
        row.name = value.into();
    }
    let primary = state
        .content_primary
        .ok_or(IdentityError::Unavailable)?
        .code();
    let content_error = error.and_then(|source_error| {
        if let VariantError::Content(source_error) = source_error {
            Some(source_error)
        } else {
            None
        }
    });
    let pending_for = |id| {
        pending
            .filter(|(field, _, _, _)| *field == id)
            .map(|(_, _, _, submitted)| submitted)
    };
    let content = content::Editor::load(
        state,
        session,
        talocart_commerce::content::Resource::Option,
        id,
        pending_for(0),
        if value_error_id == 0 {
            content_error
        } else {
            None
        },
        "option-content",
    )
    .await?;
    let ids = rows.iter().map(|r| r.id).collect::<Vec<_>>();
    let translations = state
        .content()
        .load_many(session, talocart_commerce::content::Resource::Value, &ids)
        .await
        .map_err(content::identity_error)?;
    let rows = rows
        .into_iter()
        .map(|row| {
            let localized = translations
                .iter()
                .filter(|stored| stored.entity_id == row.id)
                .map(|r| r.translation.clone())
                .collect();
            let editor = content::Editor::new(
                primary,
                &state.content_active,
                talocart_commerce::content::Resource::Value.definition(),
                localized,
                pending_for(row.id),
                if value_error_id == row.id {
                    content_error
                } else {
                    None
                },
                &format!("value-{}-content", row.id),
            );
            EditableValue {
                id: row.id,
                name: row.name,
                sort_order: pending
                    .filter(|(field, _, _, _)| *field == row.id)
                    .and_then(|(_, _, order, _)| order)
                    .map(str::to_owned)
                    .unwrap_or_else(|| row.sort_order.to_string()),
                content: editor,
            }
        })
        .collect();
    let new_content = content::Editor::new(
        primary,
        &state.content_active,
        talocart_commerce::content::Resource::Value.definition(),
        vec![],
        pending_for(-1),
        if value_error_id == -1 {
            content_error
        } else {
            None
        },
        "new-value-content",
    );
    let admin_shell = AdminShell::load(state, session).await?;
    let store = admin_shell.store_name;
    let suffix = search_suffix(&search);
    let search_results = if search.is_empty() {
        String::new()
    } else {
        let mut args = fluent_bundle::FluentArgs::new();
        args.set("count", result.total);
        args.set("query", search.text());
        i18n::with_args("options-values-search-results", &args)
    };
    let mut response = render(OptionPage {
        content,
        new_content,
        store_name: &store,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "options",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        id,
        name,
        new_value,
        sort_order,
        new_sort_order,
        order_error: value_error_id == 0 && matches!(error, Some(VariantError::SortOrder)),
        value_order_error: matches!(error, Some(VariantError::SortOrder)),
        rows,
        can_write: session.allows(Permission::ProductsWrite),
        error: error_message,
        name_error: value_error_id == 0
            && feedback.deleting_value.is_none()
            && matches!(
                error,
                Some(VariantError::Name | VariantError::DuplicateName)
            ),
        value_name_error: matches!(
            error,
            Some(VariantError::Name | VariantError::DuplicateName)
        ),
        value_error_id,
        delete_error_id,
        pagination: crate::pagination::Pagination::new(
            result.total,
            result.page,
            variants::VALUE_PAGE_SIZE,
            &i18n::text("admin-values"),
            &i18n::text("admin-value-pages"),
            |page| format!("/admin/catalog/options/{id}?page={page}{suffix}"),
        ),
        saved: feedback.saved,
        value_deleted: feedback.value_deleted,
        search: if search_invalid {
            feedback.search.to_owned()
        } else {
            search.text().to_owned()
        },
        search_invalid,
        search_error: if search_invalid {
            i18n::variant_error(&VariantError::Search)
        } else {
            String::new()
        },
        search_results,
    });
    if let Some(source_error) = error {
        *response.status_mut() = status(source_error);
    } else if search_invalid {
        *response.status_mut() = status(&VariantError::Search);
    }
    Ok(response)
}
async fn new_option(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    option_page(&state, &session, 0, None, None, false).await
}
async fn edit_option(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(q): Query<Page>,
) -> Result<Response> {
    let state = state.primary_content().await?;
    option_page_at(
        &state,
        &session,
        id,
        None,
        OptionFeedback {
            saved: q.saved == "1" && q.deleted != "1",
            value_deleted: q.deleted == "1",
            search: &q.q,
            ..OptionFeedback::default()
        },
        q.page,
    )
    .await
}
#[derive(Deserialize)]
struct NameForm {
    #[serde(default)]
    sort_order: Option<String>,
    /// Value search to restore after saving a value; ignored for options.
    #[serde(default)]
    q: String,
    #[serde(flatten)]
    content: content::Submission,
    csrf: String,
    name: String,
}
async fn save_option(
    state: AppState,
    session: Session,
    id: Option<i64>,
    f: NameForm,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    match state
        .variants
        .save_option_details(
            &session,
            &f.csrf,
            id,
            &variants::OptionInput {
                name: &f.name,
                sort_order: f.sort_order.as_deref(),
            },
            content::draft(&f.content).as_ref(),
        )
        .await
    {
        Ok(id) => Ok(Redirect::to(&format!("/admin/catalog/options/{id}?saved=1")).into_response()),
        Err(VariantError::Technical(error)) => Err(error.into()),
        Err(VariantError::Access(error)) => Err(error.into()),
        Err(VariantError::Identity(error)) => Err(error.into()),
        Err(VariantError::NotFound) => Err(VariantError::NotFound.into()),
        Err(error) => {
            option_page(
                &state,
                &session,
                id.unwrap_or(0),
                Some((0, &f.name, f.sort_order.as_deref(), &f.content)),
                Some(&error),
                false,
            )
            .await
        }
    }
}
async fn create_option(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(f): Form<NameForm>,
) -> Result<Response> {
    save_option(state, session, None, f).await
}
async fn update_option(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<NameForm>,
) -> Result<Response> {
    save_option(state, session, Some(id), f).await
}
async fn delete_option(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<Action>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    let result = if f.confirm {
        state.variants.delete_option(&session, &f.csrf, id).await
    } else {
        Err(VariantError::Confirmation)
    };
    match result {
        Ok(()) => Ok(Redirect::to("/admin/catalog/options?deleted=1").into_response()),
        Err(VariantError::Technical(error)) => Err(error.into()),
        Err(VariantError::Access(error)) => Err(error.into()),
        Err(VariantError::Identity(error)) => Err(error.into()),
        Err(error) => option_page(&state, &session, id, None, Some(&error), false).await,
    }
}
async fn save_value(
    state: AppState,
    session: Session,
    option: i64,
    id: Option<i64>,
    f: NameForm,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    match state
        .variants
        .save_value_details(
            &session,
            &f.csrf,
            option,
            id,
            &variants::OptionInput {
                name: &f.name,
                sort_order: f.sort_order.as_deref(),
            },
            content::draft(&f.content).as_ref(),
        )
        .await
    {
        Ok(value_id) => {
            let primary = state.primary_content().await?;
            let mut search = value_search(&primary, &f.q).0;
            let page = locate_value(&primary, &session, option, value_id, &mut search).await?;
            let page = if page > 1 {
                format!("&page={page}")
            } else {
                String::new()
            };
            let search = search_suffix(&search);
            Ok(Redirect::to(&format!(
                "/admin/catalog/options/{option}?saved=1{page}{search}"
            ))
            .into_response())
        }
        Err(VariantError::Technical(source_error)) => Err(source_error.into()),
        Err(VariantError::Access(source_error)) => Err(source_error.into()),
        Err(VariantError::Identity(source_error)) => Err(source_error.into()),
        Err(VariantError::NotFound) => Err(VariantError::NotFound.into()),
        Err(source_error) => {
            option_page_at(
                &state,
                &session,
                option,
                Some((
                    id.unwrap_or(-1),
                    &f.name,
                    f.sort_order.as_deref(),
                    &f.content,
                )),
                OptionFeedback {
                    error: Some(&source_error),
                    search: &f.q,
                    ..OptionFeedback::default()
                },
                1,
            )
            .await
        }
    }
}
async fn create_value(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<NameForm>,
) -> Result<Response> {
    save_value(state, session, id, None, f).await
}
async fn update_value(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((id, value)): Path<(i64, i64)>,
    Form(f): Form<NameForm>,
) -> Result<Response> {
    save_value(state, session, id, Some(value), f).await
}
async fn delete_value(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((id, value)): Path<(i64, i64)>,
    Form(f): Form<DeleteValueForm>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&f.csrf)?;
    let state = state.primary_content().await?;
    // Locate using the same content language and ordering as the editor. Old
    // forms without a page retain a fallback. Error rendering locates the value
    // again with its rows in one snapshot after any concurrent writer.
    let mut search = value_search(&state, &f.q).0;
    let page = match locate_value(&state, &session, id, value, &mut search).await {
        Ok(page) => page,
        Err(VariantError::NotFound) => f.page.max(1),
        Err(source_error) => return Err(source_error.into()),
    };
    let result = if f.confirm {
        state
            .variants
            .delete_value(&session, &f.csrf, id, value)
            .await
    } else {
        Err(VariantError::Confirmation)
    };
    match result {
        Ok(()) => {
            let page = state
                .variants
                .values_counted(&session, id, page, &search)
                .await?
                .page;
            let search = search_suffix(&search);
            Ok(Redirect::to(&format!(
                "/admin/catalog/options/{id}?deleted=1&page={page}{search}"
            ))
            .into_response())
        }
        Err(VariantError::Technical(source_error)) => Err(source_error.into()),
        Err(VariantError::Access(source_error)) => Err(source_error.into()),
        Err(VariantError::Identity(source_error)) => Err(source_error.into()),
        Err(source_error) => {
            option_page_at(
                &state,
                &session,
                id,
                None,
                OptionFeedback {
                    error: Some(&source_error),
                    deleting_value: Some(value),
                    search: search.text(),
                    ..OptionFeedback::default()
                },
                page,
            )
            .await
        }
    }
}
#[derive(Deserialize)]
struct DeleteValueForm {
    csrf: String,
    #[serde(default)]
    confirm: bool,
    #[serde(default = "first_page")]
    page: i64,
    #[serde(default)]
    q: String,
}
