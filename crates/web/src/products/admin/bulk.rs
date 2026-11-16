use super::*;
use talocart_commerce::products::{BulkAction, BulkActions, ProductSelection};

struct CategoryRow {
    entry: talocart_commerce::ProductCategory,
    selected: bool,
    children: bool,
    closes: Vec<()>,
}
// The catalog supplies bounded preorder rows. Encode nesting as typed transitions;
// templates emit fixed markup and escape every category name/path.
fn category_rows(
    entries: Vec<talocart_commerce::ProductCategory>,
    selected: &str,
) -> Vec<CategoryRow> {
    let mut entries = entries.into_iter().peekable();
    let mut rows = Vec::new();
    while let Some(entry) = entries.next() {
        let next = entries.peek().map(|entry| entry.depth).unwrap_or(0);
        let children = next > entry.depth;
        let closes = (0..entry.depth.saturating_sub(next).max(0))
            .map(|_| ())
            .collect();
        rows.push(CategoryRow {
            selected: selected == entry.id.to_string(),
            entry,
            children,
            closes,
        });
    }
    rows
}

struct BulkForm {
    csrf: String,
    return_to: String,
    ids: Vec<i64>,
    actions: Vec<String>,
    confirm: bool,
    category_id: String,
}
impl BulkForm {
    // serde_urlencoded supports repeated keys as pairs; native checkboxes need no JS serializer.
    fn parse(pairs: Vec<(String, String)>) -> Result<Self> {
        let mut fields = std::collections::HashMap::new();
        let mut ids = Vec::new();
        let mut actions = Vec::new();
        for (key, value) in pairs {
            if key == "ids" {
                if ids.len() == ProductSelection::MAX_LEN {
                    return Err(ProductError::Selection.into());
                }
                ids.push(value.parse::<i64>().map_err(|_| ProductError::Selection)?);
            } else if key == "action" {
                if actions.len() == 4 {
                    return Err(ProductError::BulkAction.into());
                }
                actions.push(value);
            } else if !matches!(
                key.as_str(),
                "csrf" | "return_to" | "confirm" | "category_id"
            ) || fields.insert(key, value).is_some()
            {
                return Err(ProductError::BulkAction.into());
            }
        }
        let return_to = fields.remove("return_to").unwrap_or_default();
        // Redirect only to this exact local listing, including its GET context.
        let return_to = if (return_to == "/admin/catalog/products"
            || return_to.starts_with("/admin/catalog/products?"))
            && !return_to.chars().any(char::is_control)
            && !return_to.contains('#')
        {
            return_to
        } else {
            "/admin/catalog/products".into()
        };
        Ok(Self {
            csrf: fields.remove("csrf").unwrap_or_default(),
            return_to,
            ids,
            actions,
            confirm: fields.remove("confirm").as_deref() == Some("true"),
            category_id: fields.remove("category_id").unwrap_or_default(),
        })
    }
    fn selection(&self) -> Result<ProductSelection> {
        Ok(ProductSelection::new(self.ids.clone())?)
    }
}

#[derive(Template)]
#[template(path = "product-bulk.html")]
struct BulkPage<'a> {
    store_name: String,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    ids: Vec<i64>,
    delete: bool,
    publish: bool,
    draft: bool,
    assign_category: bool,
    return_to: &'a str,
    error: String,
    categories: Vec<CategoryRow>,
    category_error: bool,
}

async fn page(
    state: &AppState,
    session: &Session,
    form: &BulkForm,
    error: Option<ProductError>,
) -> Result<Response> {
    let result = match form.selection() {
        Ok(selection) => state.products.selected(session, &selection).await,
        Err(error) => Err(error.0),
    };
    let (rows, error) = match result {
        Ok(rows) => (rows, error),
        Err(ProductError::Technical(error)) => return Err(error.into()),
        Err(ProductError::Access(error)) => return Err(error.into()),
        Err(ProductError::Identity(error)) => return Err(error.into()),
        Err(reason) => (Vec::new(), Some(reason)),
    };
    let code = error.as_ref().map(status).unwrap_or(StatusCode::OK);
    let admin_shell = AdminShell::load(state, session).await?;
    let mut response = render(BulkPage {
        store_name: admin_shell.store_name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "products",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        ids: rows,
        delete: form.actions.iter().any(|action| action == "delete"),
        publish: form.actions.iter().any(|action| action == "publish"),
        draft: form.actions.iter().any(|action| action == "draft"),
        assign_category: form.actions.iter().any(|action| action == "category"),
        return_to: &form.return_to,
        categories: category_rows(
            state.catalog.product_category_tree(session).await?,
            &form.category_id,
        ),
        category_error: matches!(&error,Some(ProductError::Invalid(fields)) if fields.contains(&Field::Category)),
        error: error.as_ref().map(i18n::product_error).unwrap_or_default(),
    });
    *response.status_mut() = code;
    Ok(response)
}

pub(super) async fn preview(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    state
        .identity
        .authorize(&session, Permission::ProductsRead)
        .await?;
    let form = BulkForm::parse(pairs)?;
    session.check_csrf(&form.csrf)?;
    page(&state, &session, &form, None).await
}

pub(super) async fn apply(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    state
        .identity
        .authorize(&session, Permission::ProductsRead)
        .await?;
    let form = BulkForm::parse(pairs)?;
    session.check_csrf(&form.csrf)?;
    let selection = form.selection()?;
    let kinds = form
        .actions
        .iter()
        .map(|action| BulkAction::parse(action))
        .collect::<std::result::Result<Vec<_>, _>>();
    let actions = match kinds
        .and_then(|kinds| BulkActions::new(&kinds, form.category_id.parse::<i64>().ok()))
    {
        Ok(actions) => actions,
        Err(error) => return page(&state, &session, &form, Some(error)).await,
    };
    let result = if !form.confirm {
        Err(ProductError::BulkConfirmation)
    } else {
        state
            .products
            .apply_actions_selected(&session, &form.csrf, &selection, &actions)
            .await
    };
    match result {
        Ok(()) => {
            let separator = if form.return_to.contains('?') {
                '&'
            } else {
                '?'
            };
            let notice = if actions.deletes() {
                "bulk_deleted=1"
            } else if actions.combined() {
                "bulk_applied=1"
            } else if actions.category().is_some() {
                "bulk_category=1"
            } else if actions.status() == Some("published") {
                "bulk_status=published"
            } else {
                "bulk_status=draft"
            };
            Ok(Redirect::to(&format!("{}{separator}{notice}", form.return_to)).into_response())
        }
        Err(ProductError::Technical(error)) => Err(error.into()),
        Err(ProductError::Access(error)) => Err(error.into()),
        Err(ProductError::Identity(error)) => Err(error.into()),
        Err(error) => page(&state, &session, &form, Some(error)).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn category_tree_transitions_keep_children_siblings_and_root_closures() {
        let entries = [(1, 0), (2, 1), (3, 2), (4, 1), (5, 0)]
            .into_iter()
            .map(|(id, depth)| talocart_commerce::ProductCategory {
                id,
                depth,
                name: format!("Category {id}"),
                path: format!("Path {id}"),
                active: true,
            })
            .collect();
        let rows = category_rows(entries, "3");
        assert_eq!(
            rows.iter().map(|r| r.children).collect::<Vec<_>>(),
            [true, true, false, false, false]
        );
        assert_eq!(
            rows.iter().map(|r| r.closes.len()).collect::<Vec<_>>(),
            [0, 0, 1, 1, 0]
        );
        assert!(rows[2].selected);
        assert!(category_rows(vec![], "").is_empty());
        let deep = (0..16)
            .map(|depth| talocart_commerce::ProductCategory {
                id: i64::from(depth) + 1,
                depth,
                name: "Deep".into(),
                path: "Deep".into(),
                active: false,
            })
            .collect();
        assert_eq!(category_rows(deep, "").last().unwrap().closes.len(), 15);
    }
    #[test]
    fn native_pairs_reject_ambiguous_fields_and_external_return_urls() {
        let form = BulkForm::parse(vec![
            ("ids".into(), "2".into()),
            ("ids".into(), "1".into()),
            ("return_to".into(), "//evil.test".into()),
        ])
        .ok()
        .expect("valid native form");
        assert_eq!(form.ids, [2, 1]);
        assert_eq!(form.return_to, "/admin/catalog/products");
        for pairs in [
            vec![("ids".into(), "invalid".into())],
            vec![("csrf".into(), "a".into()), ("csrf".into(), "b".into())],
            vec![
                ("category_id".into(), "1".into()),
                ("category_id".into(), "2".into()),
            ],
        ] {
            assert!(BulkForm::parse(pairs).is_err());
        }
    }
}
