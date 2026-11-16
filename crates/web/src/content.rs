//! Shared inline language groups for native resource forms.
use super::*;
use std::collections::BTreeMap;
use talocart_commerce::content::Resource;
use talocart_platform::{
    content::{ContentError, Definition, Draft, Translation},
    identity::IdentityError,
    locale::Locale,
};
pub type Submission = BTreeMap<String, String>;
pub fn draft(values: &Submission) -> Option<Draft> {
    let primary = values.get("content_primary_locale")?.clone();
    let rows = Locale::SUPPORTED
        .iter()
        .filter(|(code, _)| {
            *code == primary
                || ["name", "description", "seo_title", "seo_description"]
                    .iter()
                    .any(|field| values.contains_key(&format!("content_{code}_{field}")))
        })
        .map(|(code, _)| {
            let value = |field| {
                values
                    .get(&format!("content_{code}_{field}"))
                    .cloned()
                    .unwrap_or_default()
            };
            Translation {
                locale: (*code).into(),
                name: value("name"),
                description: talocart_platform::rich_text::submitted(
                    &value("description"),
                    &value("description_format"),
                ),
                seo_title: value("seo_title"),
                seo_description: value("seo_description"),
            }
        })
        .collect();
    Some(Draft {
        primary,
        rows,
        active: values
            .get("content_active_locales")
            .map(|value| value.split(',').map(str::to_owned).collect()),
        submitted: Some(
            Locale::SUPPORTED
                .iter()
                .flat_map(|(code, _)| {
                    ["name", "description", "seo_title", "seo_description"]
                        .into_iter()
                        .filter(move |field| {
                            values.contains_key(&format!("content_{code}_{field}"))
                        })
                        .map(move |field| ((*code).to_owned(), field))
                })
                .collect(),
        ),
    })
}
pub fn message(error: &ContentError) -> String {
    match error {
        ContentError::Technical(source_error) => crate::i18n::technical_error(source_error),
        ContentError::Access(source_error) => crate::i18n::access_error(source_error),
        ContentError::Identity(source_error) => i18n::identity_error(source_error),
        _ => i18n::text(match error {
            ContentError::Invalid(_) => "admin-content-invalid",
            ContentError::Conflict => "admin-content-conflict",
            ContentError::LanguageChanged => "admin-content-language-changed",
            _ => "admin-content-not-found",
        }),
    }
}
pub fn status(error: &ContentError) -> StatusCode {
    match error {
        ContentError::Technical(source_error) => crate::access::technical_status(source_error),
        ContentError::Access(source_error) => crate::access::access_status(source_error),
        ContentError::Identity(source_error) => {
            access::IdentityErrorResponse::status_for(source_error)
        }
        ContentError::NotFound => StatusCode::NOT_FOUND,
        ContentError::Conflict | ContentError::LanguageChanged => StatusCode::CONFLICT,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
pub fn identity_error(error: ContentError) -> IdentityError {
    match error {
        ContentError::Technical(source_error) => source_error.into(),
        ContentError::Access(source_error) => source_error.into(),
        ContentError::Identity(source_error) => source_error,
        _ => IdentityError::Unavailable,
    }
}
pub struct Editor {
    pub primary: String,
    pub active: String,
    pub primary_name: String,
    pub secondary: Vec<LanguageFields>,
}
pub struct LanguageFields {
    pub code: String,
    pub name: String,
    pub fields: Vec<Field>,
}
pub struct Field {
    pub label: &'static str,
    pub id: String,
    pub name: String,
    pub value: String,
    pub maximum: usize,
    pub multiline: bool,
    pub rich: bool,
    pub invalid: bool,
}
impl Editor {
    pub fn new(
        primary: &str,
        active: &[String],
        definition: Definition,
        mut rows: Vec<Translation>,
        submitted: Option<&Submission>,
        error: Option<&ContentError>,
        prefix: &str,
    ) -> Self {
        let pending = submitted.and_then(draft);
        let primary = pending.as_ref().map_or(primary, |p| p.primary.as_str());
        let active = pending
            .as_ref()
            .and_then(|p| p.active.as_deref())
            .unwrap_or(active);
        if let Some(pending) = &pending {
            pending.apply(&mut rows);
        }
        let primary_name = Locale::SUPPORTED
            .iter()
            .find(|(code, _)| *code == primary)
            .map_or(primary, |(_, name)| *name)
            .to_owned();
        let mut secondary = Vec::new();
        for (code, native) in Locale::SUPPORTED {
            if *code == primary || !active.iter().any(|selected| selected == code) {
                continue;
            }
            let row = rows
                .iter()
                .find(|r| r.locale == *code)
                .cloned()
                .unwrap_or_default();
            let mut fields = Vec::new();
            for (key, label, value, maximum, multiline) in [
                (
                    "name",
                    if definition.table == "home_banners" {
                        "banners-alt"
                    } else if definition.table == "catalog_product_attributes" {
                        "attributes-value"
                    } else if definition.table == "information_pages" {
                        "pages-name"
                    } else if definition.name_column == "heading" {
                        "admin-content-heading"
                    } else {
                        "admin-content-name"
                    },
                    row.name,
                    definition.name_limit,
                    false,
                ),
                (
                    "description",
                    if definition.table == "home_banners" {
                        "banners-link"
                    } else if definition.table == "information_pages" {
                        "pages-body"
                    } else if definition.name_column == "heading" {
                        "admin-content-introduction"
                    } else {
                        "admin-content-description"
                    },
                    row.description,
                    definition.description_limit,
                    definition.description_limit > 500 && definition.table != "home_banners",
                ),
                (
                    "seo_title",
                    "admin-seo-title",
                    row.seo_title,
                    if definition.seo { 120 } else { 0 },
                    false,
                ),
                (
                    "seo_description",
                    "admin-seo-description",
                    row.seo_description,
                    if definition.seo { 320 } else { 0 },
                    false,
                ),
            ] {
                if maximum == 0 {
                    continue;
                }
                let invalid = matches!(error,Some(ContentError::Invalid(errors)) if errors.iter().any(|(locale,field)|locale==code && *field==key));
                fields.push(Field {
                    label,
                    id: format!("{prefix}-{code}-{key}"),
                    name: format!("content_{code}_{key}"),
                    value,
                    maximum,
                    multiline,
                    rich: key == "description"
                        && matches!(definition.table, "catalog_products" | "information_pages"),
                    invalid,
                });
            }
            secondary.push(LanguageFields {
                code: (*code).into(),
                name: (*native).into(),
                fields,
            });
        }
        Self {
            primary: primary.into(),
            active: active.join(","),
            primary_name,
            secondary,
        }
    }
    pub async fn load(
        state: &AppState,
        session: &Session,
        resource: Resource,
        id: i64,
        submitted: Option<&Submission>,
        error: Option<&ContentError>,
        prefix: &str,
    ) -> Result<Self, IdentityError> {
        let rows = if id > 0 {
            state
                .content()
                .load(session, resource, id)
                .await
                .map_err(identity_error)?
        } else {
            vec![]
        };
        let primary = match state.content_primary {
            Some(primary) => primary.code().to_owned(),
            None => state.storefront.settings().await?.primary_locale,
        };
        Ok(Self::new(
            &primary,
            &state.content_active,
            resource.definition(),
            rows,
            submitted,
            error,
            prefix,
        ))
    }
}
