//! Native editor drafts use the canonical media page without persisting the draft.
use super::*;
use axum::extract::{Form, Multipart};
use talocart_platform::{
    identity::IdentityError,
    media::library::{LibraryError, MediaLibrary},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Editor {
    Settings,
    Banner(i64),
}
impl Editor {
    fn parse(fields: &[(String, String)]) -> Result<Self, IdentityError> {
        let mut editors = fields.iter().filter(|(key, _)| key == "media_editor");
        let value = editors.next().ok_or(IdentityError::Invalid)?.1.as_str();
        if editors.next().is_some() {
            return Err(IdentityError::Invalid);
        }
        match value {
            "settings" => Ok(Self::Settings),
            value => value
                .strip_prefix("banner:")
                .and_then(|id| id.parse::<i64>().ok())
                .filter(|id| *id >= 0)
                .map(Self::Banner)
                .ok_or(IdentityError::Invalid),
        }
    }
    async fn dispatch(
        self,
        state: AppState,
        session: Session,
        mut pairs: Vec<(String, String)>,
    ) -> Response {
        pairs.retain(|(key, _)| {
            !["media_editor", "media_uses_page", "media_folder_name"].contains(&key.as_str())
        });
        match self {
            Self::Settings => {
                match super::unified_settings::save(State(state), Extension(session), Form(pairs))
                    .await
                {
                    Ok(r) => r,
                    Err(error) => error.into_response(),
                }
            }
            Self::Banner(id) => {
                match super::banners::save(state, session, id, pairs.into_iter().collect()).await {
                    Ok(r) => r,
                    Err(error) => error.into_response(),
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Draft {
    pub fields: Vec<(String, String)>,
    pub editor: String,
    pub prefix: &'static str,
    pub upload_failed: bool,
    pub folder_name: String,
}
impl Draft {
    pub fn banner(id: i64, fields: content::Submission) -> Self {
        Self {
            fields: fields.into_iter().collect(),
            editor: format!("banner:{id}"),
            prefix: "banner",
            ..Default::default()
        }
    }
    pub fn settings(mut fields: Vec<(String, String)>) -> Self {
        fields.retain(|(key, _)| key != "module_install_password");
        Self {
            fields,
            editor: "settings".into(),
            prefix: "settings",
            ..Default::default()
        }
    }
    pub fn parse(fields: Vec<(String, String)>) -> Result<Self, IdentityError> {
        let editor = Editor::parse(&fields)?;
        if editor == Editor::Settings {
            let target = fields
                .iter()
                .find(|(key, _)| key == "media_target")
                .map(|(_, value)| value.as_str())
                .unwrap_or_default();
            if !["logo", "favicon"].contains(&target) {
                return Err(IdentityError::InvalidBranding);
            }
        }
        Ok(match editor {
            Editor::Settings => Self::settings(fields),
            Editor::Banner(id) => Self {
                fields,
                editor: format!("banner:{id}"),
                prefix: "banner",
                ..Default::default()
            },
        })
    }
    pub fn active(&self) -> bool {
        !self.editor.is_empty()
    }
    fn get(&self, key: &str) -> String {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
    pub fn query(&self) -> Result<super::media::MediaQuery, LibraryError> {
        let page = |key| {
            self.get(key)
                .parse::<i64>()
                .unwrap_or(1)
                .clamp(1, 1_000_000)
        };
        Ok(super::media::MediaQuery {
            target: "logo".into(),
            folder: folder(&self.get("media_folder"))?,
            q: self.get("media_query"),
            format: self.get("media_format"),
            usage: self.get("media_usage"),
            sort: self.get("media_sort"),
            page: page("media_page"),
            folders_page: page("media_folders_page"),
            selected: self.get("media_selected").parse().ok(),
            saved: self.get("media_saved") == "true",
            uses_page: page("media_uses_page"),
            ..Default::default()
        })
    }
    pub fn prepare(&mut self) {
        self.upload_failed = self.get("media_upload_failed") == "true";
        self.folder_name = self.get("media_folder_name");
        self.fields.retain(|(key, _)| {
            ![
                "media_editor",
                "action",
                "picker_image",
                "media_query",
                "media_format",
                "media_usage",
                "media_sort",
                "media_page",
                "media_folder",
                "media_folders_page",
                "media_upload_failed",
                "media_selected",
                "media_saved",
                "media_uses_page",
                "media_folder_name",
            ]
            .contains(&key.as_str())
        });
    }
    pub async fn authorize(
        &self,
        state: &AppState,
        session: &Session,
    ) -> Result<(), IdentityError> {
        session.check_csrf(&self.get("csrf"))?;
        if self.prefix == "settings" {
            state
                .identity
                .authorize(session, Permission::SettingsRead)
                .await?;
        }
        for permission in [Permission::SettingsWrite, Permission::MediaRead] {
            state.identity.authorize(session, permission).await?;
        }
        Ok(())
    }
}
pub(super) fn library(state: &AppState) -> MediaLibrary {
    MediaLibrary::new(
        state.pool.clone(),
        state.identity.clone(),
        state.media.clone(),
    )
}
fn folder(value: &str) -> Result<Option<i64>, LibraryError> {
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .map(Some)
            .ok_or(LibraryError::NotFound)
    }
}

pub(super) async fn choose(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    if pairs
        .iter()
        .any(|(key, value)| key == "action" && value == "media_return")
    {
        return match return_to_selection(&state, &session, pairs).await {
            Ok(r) => r,
            Err(error) => error.into_response(),
        };
    }
    match Editor::parse(&pairs) {
        Ok(editor) => editor.dispatch(state, session, pairs).await,
        Err(error) => access::IdentityErrorResponse::from(error).into_response(),
    }
}

async fn return_to_selection(
    state: &AppState,
    session: &Session,
    pairs: Vec<(String, String)>,
) -> Result<Response, super::media::MediaResponseError> {
    let draft = Draft::parse(pairs)?;
    draft.authorize(state, session).await?;
    super::media::draft_page(state, session, draft).await
}

pub(super) async fn detail(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, super::media::MediaResponseError> {
    let mut draft = Draft::parse(pairs)?;
    draft.authorize(&state, &session).await?;
    let mut query = draft.query()?;
    query.uses_page = match draft.get("action").as_str() {
        "media_uses_next" => query.uses_page.saturating_add(1).min(1_000_000),
        "media_uses_previous" => (query.uses_page - 1).max(1),
        _ => query.uses_page,
    };
    draft.prepare();
    super::media::draft_detail(&state, &session, id, &query, draft).await
}

/// Navigation is a POST of the entire editor draft; no draft is stored or saved.
pub(super) fn navigate(
    action: &str,
    mut set: impl FnMut(&str, String),
) -> Result<bool, IdentityError> {
    if let Some(value) = action.strip_prefix("media_folder_") {
        let id = value.parse::<i64>().map_err(|_| IdentityError::Invalid)?;
        if id < 0 {
            return Err(IdentityError::Invalid);
        }
        set(
            "media_folder",
            if id == 0 {
                String::new()
            } else {
                id.to_string()
            },
        );
        set("media_folders_page", "1".into());
    } else if let Some(value) = action.strip_prefix("media_folders_") {
        let (parent, page) = value.split_once('_').ok_or(IdentityError::Invalid)?;
        let parent = parent.parse::<i64>().map_err(|_| IdentityError::Invalid)?;
        let page = page.parse::<i64>().map_err(|_| IdentityError::Invalid)?;
        if parent < 0 || !(1..=1_000_000).contains(&page) {
            return Err(IdentityError::Invalid);
        }
        set(
            "media_folder",
            if parent == 0 {
                String::new()
            } else {
                parent.to_string()
            },
        );
        set("media_folders_page", page.to_string());
    } else {
        return Ok(false);
    }
    set("media_page", "1".into());
    set("media_query", String::new());
    Ok(true)
}

pub(super) async fn create_folder(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<Response, super::media::MediaResponseError> {
    let mut draft = Draft::parse(pairs)?;
    draft.authorize(&state, &session).await?;
    // Authorize before validation/rendering, including crafted POSTs from readers.
    state
        .identity
        .authorize(&session, Permission::MediaWrite)
        .await?;
    let result = library(&state)
        .create_folder(
            &session,
            &draft.get("csrf"),
            draft.query()?.folder,
            &draft.get("media_folder_name"),
            false,
        )
        .await;
    match result {
        Ok(id) => {
            draft.fields.retain(|(key, _)| {
                ![
                    "media_folder",
                    "media_folder_name",
                    "media_page",
                    "media_folders_page",
                    "media_selected",
                    "media_saved",
                ]
                .contains(&key.as_str())
            });
            draft.fields.push(("media_folder".into(), id.to_string()));
            super::media::draft_page(&state, &session, draft).await
        }
        Err(error) => {
            super::media::draft_page_with_failure(&state, &session, draft, Some(&error)).await
        }
    }
}

pub(super) async fn upload(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    mut multipart: Multipart,
) -> Response {
    async fn run(
        state: AppState,
        session: Session,
        multipart: &mut Multipart,
    ) -> Result<Response, Response> {
        state
            .identity
            .authorize(&session, Permission::MediaWrite)
            .await
            .map_err(|error| access::IdentityErrorResponse::from(error).into_response())?;
        let invalid = || {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                i18n::text("media-picker-upload-invalid"),
            )
                .into_response()
        };
        let mut pairs = Vec::new();
        let mut files = Vec::new();
        let mut too_many_files = false;
        let mut size = 0;
        while let Some(field) = multipart.next_field().await.map_err(|_| invalid())? {
            let name = field.name().ok_or_else(invalid)?.to_owned();
            if name == "picker_image" {
                if files.len() >= 10 {
                    too_many_files = true;
                    let _ = field.bytes().await.map_err(|_| invalid())?;
                    continue;
                }
                let filename = field.file_name().unwrap_or("").to_owned();
                let bytes = field.bytes().await.map_err(|_| invalid())?;
                files.push((filename, bytes.to_vec()));
            } else {
                if pairs.len() >= 256 {
                    return Err(invalid());
                }
                let value = field.text().await.map_err(|_| invalid())?;
                size += name.len() + value.len();
                if size > 512 * 1024 {
                    return Err(invalid());
                }
                pairs.push((name, value));
            }
        }
        let editor = Editor::parse(&pairs).map_err(|_| invalid())?;
        let draft = Draft::parse(pairs).map_err(|_| invalid())?;
        draft
            .authorize(&state, &session)
            .await
            .map_err(|error| access::IdentityErrorResponse::from(error).into_response())?;
        let csrf = draft.get("csrf");
        let selected = folder(&draft.get("media_folder")).map_err(|_| invalid())?;
        let mut pairs = draft.fields;
        let lib = library(&state);
        if too_many_files {
            files.clear();
        }
        let mut selected_asset = None;
        let mut failed = too_many_files || files.is_empty();
        for (name, raw) in files {
            // Classification and decoding remain in the media service; documents cannot
            // enter an image-only selection through an extension accepted for documents.
            if name.is_empty()
                || raw.is_empty()
                || !["png", "jpg", "jpeg", "webp"].contains(
                    &name
                        .rsplit('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                )
            {
                failed = true;
                continue;
            }
            match lib
                .upload(&session, &csrf, selected, raw, &name, false)
                .await
            {
                Ok(id) => selected_asset = Some(id),
                Err(_) => failed = true,
            }
        }
        pairs.retain(|(key, _)| {
            ![
                "media_editor",
                "action",
                "media_upload_failed",
                "media_page",
                "media_selected",
                "media_saved",
            ]
            .contains(&key.as_str())
        });
        pairs.push(("media_page".into(), "1".into()));
        pairs.push(("media_upload_failed".into(), failed.to_string()));
        if let Some(id) = selected_asset {
            pairs.push(("media_selected".into(), id.to_string()));
            pairs.push(("media_saved".into(), "true".into()));
        }
        // Upload and attachment are deliberately separate operations.
        pairs.push(("action".into(), "media_search".into()));
        Ok(editor.dispatch(state, session, pairs).await)
    }
    match run(state, session, &mut multipart).await {
        Ok(r) | Err(r) => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_dispatch_accepts_initial_browse_but_rejects_ambiguous_destinations() {
        let fields = vec![
            ("media_editor".into(), "settings".into()),
            ("action".into(), "browse_logo".into()),
        ];
        assert!(matches!(Editor::parse(&fields), Ok(Editor::Settings)));
        assert!(
            Draft::parse(fields).is_err(),
            "Detail and upload still require an active target"
        );
        assert!(Editor::parse(&[]).is_err());
        for value in ["settings", "banner:0", "banner:42"] {
            let fields = vec![("media_editor".into(), value.into())];
            assert!(Editor::parse(&fields).is_ok());
            let mut duplicated = fields;
            duplicated.push(("media_editor".into(), value.into()));
            assert!(Editor::parse(&duplicated).is_err());
        }
    }

    #[test]
    fn draft_context_is_allowlisted_and_repeated_values_survive_navigation() {
        for editor in ["https://example.test", "banner:-1", "banner:x", "product:1"] {
            assert!(Draft::parse(vec![("media_editor".into(), editor.into())]).is_err());
        }
        assert!(Draft::parse(vec![("media_editor".into(), "settings".into())]).is_err());
        let mut draft = Draft::parse(vec![
            ("media_editor".into(), "settings".into()),
            ("media_target".into(), "logo".into()),
            ("csrf".into(), "synthetic-csrf".into()),
            ("name".into(), "Draft <&> name".into()),
            ("purchase_country".into(), "IT".into()),
            ("purchase_country".into(), "FR".into()),
            ("locale".into(), "it".into()),
            ("locale".into(), "en".into()),
            ("media_folder".into(), "42".into()),
            ("media_format".into(), "png".into()),
        ])
        .expect("Valid settings draft");
        let query = draft.query().expect("Valid navigation");
        assert_eq!(query.folder, Some(42));
        assert_eq!(query.format, "png");
        assert!(!query.personal);
        let url = query.url(1, query.folder);
        assert!(!url.contains("synthetic-csrf"));
        assert!(!url.contains("Draft"));
        draft.prepare();
        for key in ["purchase_country", "locale"] {
            assert_eq!(
                draft.fields.iter().filter(|(name, _)| name == key).count(),
                2
            );
        }
        assert!(
            draft
                .fields
                .contains(&("name".into(), "Draft <&> name".into()))
        );
        assert!(
            draft
                .fields
                .contains(&("csrf".into(), "synthetic-csrf".into()))
        );
        assert!(!draft.fields.iter().any(|(key, _)| key == "media_folder"));
    }
}
