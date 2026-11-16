use super::*;

#[derive(Deserialize, Default)]
pub(super) struct Search {
    #[serde(default)]
    q: String,
}
#[derive(Deserialize)]
pub(super) struct Change {
    csrf: String,
    action: String,
    #[serde(default)]
    target: i64,
    #[serde(default)]
    fallback: String,
    #[serde(default)]
    related_q: String,
}
pub(super) async fn index(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Query(query): Query<Search>,
) -> Result<Response> {
    state.products.entry(&session, id).await?;
    if query.q.chars().count() > 120 {
        return Err(ProductError::Invalid(vec![]).into());
    }
    Ok(Redirect::to(&format!(
        "/admin/catalog/products/{id}?tab=related&related_q={}#panel-related",
        catalog::encoded(&query.q)
    ))
    .into_response())
}

pub(super) async fn change(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(input): Form<Change>,
) -> Result<Response> {
    if input.related_q.chars().count() > 120 {
        return Err(ProductError::Invalid(vec![]).into());
    }
    state
        .products
        .change_related(
            &session,
            &input.csrf,
            id,
            &input.action,
            input.target,
            &input.fallback,
        )
        .await?;
    Ok(Redirect::to(&format!(
        "/admin/catalog/products/{id}?tab=related&related_saved=1&related_q={}#panel-related",
        catalog::encoded(&input.related_q)
    ))
    .into_response())
}
