use super::*;
use talocart_commerce::orders::{FulfillmentEvent, FulfillmentInput};
#[derive(Default)]
pub(super) struct Context {
    pub input: Option<FulfillmentInput>,
    pub invalid: Vec<&'static str>,
    pub revision: Option<String>,
    pub request_key: Option<String>,
    pub stale: bool,
    pub saved: bool,
    pub page: i64,
}
pub(super) struct TrackingField {
    pub name: &'static str,
    pub value: String,
    pub label: String,
    pub max: usize,
    pub required: bool,
    pub invalid: bool,
    pub error: String,
}
pub(super) struct View {
    pub tracking_fields: Vec<TrackingField>,
    pub editing_tracking: bool,
    pub editing_note: bool,
    pub input: FulfillmentInput,
    pub invalid: Vec<&'static str>,
    pub revision: String,
    pub request_key: String,
    pub stale: bool,
    pub saved: bool,
    pub history: Vec<FulfillmentEvent>,
    pub pagination: crate::pagination::Pagination,
}
impl View {
    pub async fn load(
        state: &AppState,
        session: &Session,
        order: &Order,
        context: Option<Context>,
    ) -> Result<Self, OrderError> {
        let c = context.unwrap_or_default();
        let page = c.page.max(1);
        let h = service(state)
            .fulfillment_history(session, order.id, page)
            .await?;
        let input = c.input.unwrap_or_else(|| FulfillmentInput {
            carrier: order.fulfillment.carrier.clone(),
            tracking_number: order.fulfillment.tracking_number.clone(),
            tracking_url: order.fulfillment.tracking_url.clone(),
            ..Default::default()
        });
        let tracking_fields = [
            ("carrier", &input.carrier, 80),
            ("tracking_number", &input.tracking_number, 120),
            ("tracking_url", &input.tracking_url, 2048),
        ]
        .into_iter()
        .map(|(name, value, max)| TrackingField {
            name,
            value: value.clone(),
            max,
            required: name == "carrier",
            invalid: c.invalid.contains(&name),
            label: i18n::text(&format!("fulfillment-{name}")),
            error: i18n::text(&format!("fulfillment-error-{name}")),
        })
        .collect();
        Ok(Self {
            tracking_fields,
            editing_tracking: input.action == "tracking"
                || order.fulfillment.tracking_changed(&input),
            editing_note: !input.body.is_empty() || c.invalid.contains(&"body"),
            input,
            invalid: c.invalid,
            revision: c
                .revision
                .unwrap_or_else(|| order.fulfillment.revision.to_string()),
            request_key: c
                .request_key
                .map(Ok)
                .unwrap_or_else(talocart_platform::identity::token)?,
            stale: c.stale,
            saved: c.saved,
            history: h.entries,
            pagination: crate::pagination::Pagination::new(
                h.total,
                h.page,
                20,
                &i18n::text("fulfillment-history"),
                &i18n::text("fulfillment-history"),
                |p| {
                    format!(
                        "/admin/orders/{}?history_page={p}#fulfillment-history",
                        order.id
                    )
                },
            ),
        })
    }
}
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Input {
    csrf: String,
    revision: String,
    request_key: String,
    action: String,
    body: String,
    carrier: String,
    tracking_number: String,
    tracking_url: String,
}
pub(super) async fn update(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Path(id): Path<i64>,
    Form(f): Form<Input>,
) -> Response {
    let result = async {
        state
            .identity
            .authorize(&session, Permission::OrdersWrite)
            .await?;
        session.check_csrf(&f.csrf)?;
        Ok::<_, OrderError>(())
    }
    .await;
    if let Err(source_error) = result {
        return error(source_error);
    }
    let mut input = FulfillmentInput {
        action: f.action,
        body: f.body,
        carrier: f.carrier,
        tracking_number: f.tracking_number,
        tracking_url: f.tracking_url,
    };
    // A delivery action must not discard edits in another part of this native form.
    if input.action == "delivered" {
        let order = match service(&state).get(&session, id).await {
            Ok(order) => order,
            Err(source_error) => return error(source_error),
        };
        if f.revision.parse::<i64>().ok() == Some(order.fulfillment.revision)
            && (!input.body.is_empty() || order.fulfillment.tracking_changed(&input))
        {
            return show_with_fulfillment(
                &state,
                &session,
                id,
                &i18n::text("fulfillment-drafts"),
                StatusCode::CONFLICT,
                PaymentForm::default(),
                Some(Context {
                    input: Some(input),
                    revision: Some(f.revision),
                    request_key: Some(f.request_key),
                    ..Default::default()
                }),
            )
            .await
            .unwrap_or_else(error);
        }
    }
    let result = match f.revision.parse::<i64>() {
        Ok(rev) => {
            service(&state)
                .update_fulfillment(&session, &f.csrf, id, rev, &f.request_key, &input)
                .await
        }
        Err(_) => Err(OrderError::FulfillmentFields(vec!["revision"])),
    };
    let source_error = match result {
        Ok(order) => {
            if input.action == "note" {
                input.body.clear();
            }
            let pending_tracking =
                input.action == "note" && order.fulfillment.tracking_changed(&input);
            if !input.body.is_empty() || pending_tracking {
                return show_with_fulfillment(
                    &state,
                    &session,
                    id,
                    "",
                    StatusCode::OK,
                    PaymentForm::default(),
                    Some(Context {
                        input: Some(input),
                        saved: true,
                        ..Default::default()
                    }),
                )
                .await
                .unwrap_or_else(error);
            }
            return private(
                Redirect::to(&format!(
                    "/admin/orders/{id}?fulfillment=1#fulfillment-manage"
                ))
                .into_response(),
            );
        }
        Err(
            source_error @ (OrderError::Identity(_)
            | OrderError::Technical(_)
            | OrderError::Access(_)
            | OrderError::NotFound),
        ) => return error(source_error),
        Err(source_error) => source_error,
    };
    let (status, key, invalid) = match &source_error {
        OrderError::FulfillmentFields(fields) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "fulfillment-invalid",
            fields.clone(),
        ),
        OrderError::FulfillmentPayment => (StatusCode::CONFLICT, "fulfillment-payment", vec![]),
        OrderError::Cart(talocart_commerce::cart::CartError::Stale)
        | OrderError::FulfillmentConflict => (StatusCode::CONFLICT, "fulfillment-stale", vec![]),
        OrderError::FulfillmentTransition => {
            (StatusCode::CONFLICT, "fulfillment-transition", vec![])
        }
        _ => (StatusCode::SERVICE_UNAVAILABLE, "ord-error", vec![]),
    };
    let stale = matches!(
        source_error,
        OrderError::Cart(talocart_commerce::cart::CartError::Stale)
            | OrderError::FulfillmentConflict
    );
    show_with_fulfillment(
        &state,
        &session,
        id,
        &i18n::text(key),
        status,
        PaymentForm::default(),
        Some(Context {
            input: Some(input),
            invalid,
            revision: Some(f.revision),
            request_key: Some(f.request_key),
            stale,
            ..Default::default()
        }),
    )
    .await
    .unwrap_or_else(error)
}
