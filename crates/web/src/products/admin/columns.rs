use super::*;
use talocart_platform::identity::ProductColumns;

pub(super) async fn save(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response> {
    let mut csrf = None;
    let mut return_to = None;
    let mut visible = Vec::new();
    for (key, value) in pairs {
        match key.as_str() {
            "columns" if visible.len() < talocart_platform::identity::PRODUCT_COLUMNS.len() => {
                visible.push(value)
            }
            "csrf" if csrf.is_none() => csrf = Some(value),
            "return_to" if return_to.is_none() => return_to = Some(value),
            _ => return Err(ProductError::Invalid(vec![]).into()),
        }
    }
    session.check_csrf(csrf.as_deref().unwrap_or_default())?;
    let columns =
        ProductColumns::from_visible(&visible).map_err(|_| ProductError::Invalid(vec![]))?;
    state
        .identity
        .save_product_columns(&session, csrf.as_deref().unwrap_or_default(), &columns)
        .await?;
    if headers
        .get("accept")
        .is_some_and(|value| value == "application/json")
    {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let target = return_to
        .filter(|value| {
            (value == "/admin/catalog/products" || value.starts_with("/admin/catalog/products?"))
                && !value.chars().any(char::is_control)
                && !value.contains('#')
        })
        .unwrap_or_else(|| "/admin/catalog/products".into());
    Ok(Redirect::to(&target).into_response())
}
