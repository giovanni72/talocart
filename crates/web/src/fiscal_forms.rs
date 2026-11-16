use super::*;
use axum::http::Request;

/// Captured before any form data is read. A concurrent change yields a stale form,
/// never old values paired with a newer revision. POST errors retain the submitted revision.
#[derive(Clone, Copy)]
pub(super) struct FiscalRevision(pub i64);
pub(super) async fn snapshot(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    match state.taxes.revision().await {
        Ok(revision) => {
            request.extensions_mut().insert(FiscalRevision(revision));
            next.run(request).await
        }
        Err(e) => taxes::TaxResponseError::from(e).into_response(),
    }
}
