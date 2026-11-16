use super::*;
async fn image_result(
    state: &AppState,
    session: &Session,
    id: i64,
    result: talocart_commerce::products::Result<()>,
) -> Result<Response> {
    match result {
        Ok(()) => Ok(
            Redirect::to(&format!("/admin/catalog/products/{id}?saved=1&tab=image"))
                .into_response(),
        ),
        Err(ProductError::Technical(source_error)) => Err(source_error.into()),
        Err(ProductError::Access(source_error)) => Err(source_error.into()),
        Err(ProductError::Identity(source_error)) => Err(source_error.into()),
        Err(ProductError::NotFound) => Err(ProductError::NotFound.into()),
        Err(source_error) => {
            let entry = state.products.entry(session, id).await?;
            form_response(
                state,
                session,
                id,
                &entry.values(),
                None,
                FormFeedback {
                    error: Some(&source_error),
                    tab: ProductTab::Image,
                    ..Default::default()
                },
            )
            .await
        }
    }
}
pub(super) async fn upload(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    let result = match branding::read_upload(&session, multipart).await {
        Ok((raw, name)) => {
            state
                .products
                .image(&session, &session.csrf, id, Some(raw), &name)
                .await
        }
        Err(talocart_platform::identity::IdentityError::InvalidBranding) => {
            Err(ProductError::Image)
        }
        Err(error) => Err(error.into()),
    };
    image_result(&state, &session, id, result).await
}
pub(super) async fn remove_image(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(form): Form<ActionForm>,
) -> Result<Response> {
    let result = state
        .products
        .image(&session, &form.csrf, id, None, "")
        .await;
    image_result(&state, &session, id, result).await
}
pub(super) async fn upload_gallery(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    let result = match branding::read_upload(&session, multipart).await {
        Ok((raw, name)) => {
            state
                .products
                .add_gallery_image(&session, &session.csrf, id, raw, &name)
                .await
        }
        Err(talocart_platform::identity::IdentityError::InvalidBranding) => {
            Err(ProductError::Image)
        }
        Err(error) => Err(error.into()),
    };
    image_result(&state, &session, id, result).await
}
#[derive(Deserialize)]
pub(super) struct GalleryForm {
    csrf: String,
    action: String,
}
pub(super) async fn change_gallery(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path((id, image)): Path<(i64, i64)>,
    Form(form): Form<GalleryForm>,
) -> Result<Response> {
    state
        .identity
        .authorize(&session, Permission::ProductsWrite)
        .await?;
    session.check_csrf(&form.csrf)?;
    let result = match GalleryAction::parse(&form.action) {
        Ok(action) => {
            state
                .products
                .change_gallery_image(&session, &form.csrf, id, image, action)
                .await
        }
        Err(error) => Err(error),
    };
    image_result(&state, &session, id, result).await
}
