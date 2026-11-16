use super::*;
use axum::extract::Query;
use serde::Deserialize;

/// The compiled decorative icon catalogue. Labels belong to the Fluent catalogues.
pub const ICONS: &[(&str, &str)] = &[
    ("shopping-cart", "admin-icons-cart"),
    ("search", "admin-icons-search"),
    ("x", "admin-icons-close"),
    ("pencil", "admin-icons-edit"),
    ("trash", "admin-icons-delete"),
    ("plus", "admin-icons-add"),
    ("check", "admin-icons-confirm"),
    ("triangle-alert", "admin-icons-warning"),
    ("layout-dashboard", "admin-icons-layout-dashboard"),
    ("package", "admin-icons-package"),
    ("layers", "admin-icons-layers"),
    ("folder-tree", "admin-icons-folder-tree"),
    ("folder", "admin-icons-folder"),
    ("folder-open", "admin-icons-folder-open"),
    ("tag", "admin-icons-tag"),
    ("download", "admin-icons-download"),
    ("image", "admin-icons-image"),
    ("settings", "admin-icons-settings"),
    ("percent", "admin-icons-percent"),
    ("map", "admin-icons-map"),
    ("users", "admin-icons-users"),
    ("shield-check", "admin-icons-shield-check"),
    ("user-round", "admin-icons-user-round"),
    ("log-out", "admin-icons-log-out"),
    ("save", "admin-icons-save"),
    ("eye", "admin-icons-eye"),
    ("list-filter", "admin-icons-list-filter"),
    ("rotate-ccw", "admin-icons-rotate-ccw"),
    ("refresh-cw", "admin-icons-refresh-cw"),
    ("upload", "admin-icons-upload"),
    ("folder-plus", "admin-icons-folder-plus"),
    ("move", "admin-icons-move"),
    ("copy", "admin-icons-copy"),
    ("arrow-up", "admin-icons-arrow-up"),
    ("arrow-down", "admin-icons-arrow-down"),
    ("external-link", "admin-icons-external-link"),
    ("book-open", "admin-icons-book-open"),
    ("contact-round", "admin-icons-contact-round"),
    ("key-round", "admin-icons-key-round"),
    ("user-cog", "admin-icons-user-cog"),
    ("file-text", "admin-icons-file-text"),
    ("panels-top-left", "admin-icons-panels-top-left"),
    ("clipboard-list", "admin-icons-clipboard-list"),
    ("sliders-horizontal", "admin-icons-sliders-horizontal"),
    ("ticket-percent", "admin-icons-ticket-percent"),
    ("list-checks", "admin-icons-list-checks"),
    ("truck", "admin-icons-truck"),
    ("credit-card", "admin-icons-credit-card"),
    ("mail", "admin-icons-mail"),
    ("tags", "admin-icons-tags"),
    ("blocks", "admin-icons-blocks"),
    ("bold", "rich-bold"),
    ("italic", "rich-italic"),
    ("underline", "rich-underline"),
    ("strikethrough", "rich-strike"),
    ("pilcrow", "rich-paragraph"),
    ("heading-2", "rich-heading2"),
    ("heading-3", "rich-heading3"),
    ("list", "rich-bulletList"),
    ("list-ordered", "rich-orderedList"),
    ("text-quote", "rich-blockquote"),
    ("link", "rich-link"),
    ("undo-2", "rich-undo"),
    ("redo-2", "rich-redo"),
    ("remove-formatting", "rich-clear"),
    ("code-xml", "rich-html"),
];

#[derive(Deserialize)]
pub struct ExampleQuery {
    #[serde(default = "example_label")]
    label: String,
    #[serde(default = "example_state")]
    status: String,
    preview: Option<String>,
}
fn example_label() -> String {
    i18n::text("admin-example-product")
}
fn example_state() -> String {
    "active".into()
}
impl ExampleQuery {
    fn label_error(&self) -> &'static str {
        if self.label.trim().is_empty()
            || self.label.chars().count() > 80
            || self.label.chars().any(char::is_control)
        {
            "admin-enter-a-name-of-1-to-80-characters-without-control-characters"
        } else {
            ""
        }
    }
    fn status_error(&self) -> &'static str {
        if matches!(self.status.as_str(), "active" | "inactive") {
            ""
        } else {
            "admin-select-one-of-the-available-statuses"
        }
    }
}

#[derive(Template)]
#[template(path = "components.html")]
struct ComponentsPage<'a> {
    store_name: &'a str,
    logo: Option<talocart_platform::media::ImageMetadata>,
    csrf: &'a str,
    section: &'static str,
    theme: talocart_platform::storefront::Settings,
    nav: AdminNavigation,
    account: AccountMenu,
    label: &'a str,
    status: &'a str,
    label_error: &'a str,
    status_error: &'a str,
    preview: bool,
    sort_name: crate::table_sort::SortState,
    sort_price: crate::table_sort::SortState,
}

pub async fn page(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Query(query): Query<ExampleQuery>,
) -> Result<Response, access::IdentityErrorResponse> {
    state
        .identity
        .authorize(&session, Permission::AdminRead)
        .await?;
    let admin_shell = AdminShell::load(&state, &session).await?;
    let name = admin_shell.store_name;
    let label_error = query.label_error();
    let status_error = query.status_error();
    let mut response = render(ComponentsPage {
        store_name: &name,
        logo: admin_shell.logo,
        csrf: admin_shell.csrf,
        section: "components",
        theme: admin_shell.theme,
        nav: admin_shell.nav,
        account: admin_shell.account,
        label: &query.label,
        status: &query.status,
        label_error,
        status_error,
        preview: query.preview.is_some(),
        sort_name: crate::table_sort::SortState::new(true, false, crate::i18n::text("admin-name")),
        sort_price: crate::table_sort::SortState::new(
            false,
            false,
            crate::i18n::text("admin-price"),
        ),
    });
    if (!label_error.is_empty() || !status_error.is_empty()) && response.status() == StatusCode::OK
    {
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Template)]
    #[template(
        source = "{% import \"ui.html\" as ui %}{% call ui::toasts() %}{{ ui::toast(\"toast\", tone, text, text) }}{% endcall %}",
        ext = "html"
    )]
    struct ToastExample<'a> {
        tone: &'a str,
        text: &'a str,
    }

    #[derive(Template)]
    #[template(
        source = "{% import \"ui.html\" as ui %}{{ ui::status_badge(label, color) }}",
        ext = "html"
    )]
    struct StatusBadgeExample<'a> {
        label: &'a str,
        color: &'a str,
    }

    #[test]
    fn status_badge_escapes_labels_and_colors_without_inline_styles() {
        let html = StatusBadgeExample {
            label: "<script>Paid</script>",
            color: "#123456",
        }
        .render()
        .expect("Badge renders");
        assert!(!html.contains("<script>"));
        assert!(html.contains("fill=\"#123456\""));
        assert!(html.contains("aria-hidden=\"true\""));
        assert!(html.contains("<rect width=\"4\""));
        assert!(!html.contains("<circle"));
        assert!(!html.contains("style="));
        let html = StatusBadgeExample {
            label: "Paid",
            color: "\" onload=\"alert(1)",
        }
        .render()
        .expect("Escaped color");
        assert!(!html.contains("onload=\""));
    }

    #[test]
    fn toast_roles_duration_and_escaping() {
        for (tone, role, duration) in [
            ("success", "status", "6000"),
            ("info", "status", "6000"),
            ("error", "alert", "0"),
            ("unknown", "status", "6000"),
        ] {
            let html = ToastExample {
                tone,
                text: "<script>bad</script>",
            }
            .render()
            .expect("Toast renders");
            assert!(!html.contains("<script>"));
            assert!(html.contains(&format!("role=\"{role}\"")));
            assert!(html.contains(&format!("data-duration=\"{duration}\"")));
            assert!(html.contains("hidden"));
            assert!(html.contains("data-ui-toasts"));
        }
    }

    #[tokio::test]
    async fn action_feedback_is_localized_toast_with_inline_fallback() {
        for (locale, close) in [
            (
                talocart_platform::locale::Locale::ITALIAN,
                "Chiudi messaggio",
            ),
            (
                talocart_platform::locale::Locale::ENGLISH,
                "Dismiss message",
            ),
        ] {
            let html = i18n::scope(locale, async {
                Example {
                    text: "<script>bad</script>",
                    tone: "error",
                }
                .render()
                .expect("Platform notification renders")
            })
            .await;
            assert!(html.contains("id=\"result\" class=\"ui-message ui-toast"));
            assert!(html.contains("data-ui-toasts"));
            assert!(html.contains("data-duration=\"0\""));
            assert!(html.contains(&format!("aria-label=\"{close}\" hidden")));
            assert!(!html.contains("<script>"));
            assert!(!html.contains("data-ui-toast-reusable"));
        }
    }

    #[derive(Template)]
    #[template(
        source = "{% import \"ui.html\" as ui %}{{ ui::message(\"product-variant-help\", \"info\", text) }}",
        ext = "html"
    )]
    struct ContextMessage<'a> {
        text: &'a str,
    }

    #[test]
    fn contextual_messages_remain_inline_without_timer_or_dismissal() {
        let html = ContextMessage {
            text: "Prices and stock belong to individual variants. <script>bad</script>",
        }
        .render()
        .expect("Context banner renders");
        assert!(html.contains("ui-message--info"));
        assert!(!html.contains("data-ui-toast"));
        assert!(!html.contains("data-duration"));
        assert!(!html.contains("<button"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn every_template_uses_shared_controls_and_feedback() {
        for (path, source) in crate::test_templates::sources() {
            if path.file_name().is_some_and(|name| name != "ui.html") {
                assert!(
                    !source.contains("data-ui-toast"),
                    "{}: toast markup must stay in the shared macros",
                    path.display()
                );
            }
            for arrow in ["\u{2197}", "&#8599;", "&#x2197;", "&#X2197;"] {
                assert!(
                    !source.contains(arrow),
                    "{}: diagonal navigation arrows are no longer used",
                    path.display()
                );
            }
            for element in ["button", "input", "select", "textarea"] {
                for tail in source.split(&format!("<{element}")).skip(1) {
                    let tag = tail.split('>').next().expect("Opening tag");
                    if element == "input" && tag.contains("type=\"hidden\"") {
                        continue;
                    }
                    let class = if element == "button" {
                        "ui-button"
                    } else if element == "input"
                        && (tag.contains("type=\"checkbox\"") || tag.contains("type=\"radio\""))
                    {
                        "ui-check"
                    } else {
                        "ui-control"
                    };
                    assert!(
                        tag.contains(class),
                        "{}: {element} missing {class}: {tag}",
                        path.display()
                    );
                }
            }
            for element in ["p", "div", "section", "small"] {
                for tail in source.split(&format!("<{element} ")).skip(1) {
                    let tag = tail.split('>').next().expect("Opening tag");
                    if tag.contains("form-error")
                        || tag.contains("form-success")
                        || tag.contains("class=\"notice")
                    {
                        assert!(
                            tag.contains("ui-message"),
                            "{}: unshared message {tag}",
                            path.display()
                        );
                    }
                    if tag.contains("catalog-empty")
                        || tag.contains("store-empty")
                        || tag.contains("media-empty")
                        || tag.contains("ui-table__empty")
                    {
                        assert!(
                            tag.contains("ui-empty"),
                            "{}: unshared empty state {tag}",
                            path.display()
                        );
                    }
                    if tag.contains("class=\"field-error") {
                        assert!(
                            tag.contains("ui-field-error"),
                            "{}: unshared field feedback {tag}",
                            path.display()
                        );
                    }
                }
            }
        }
    }

    #[derive(Template)]
    #[template(
        source = "{% import \"ui.html\" as ui %}{{ ui::feedback(\"result\", tone, text) }}{{ ui::field_feedback(\"help\", text, \"error\", text) }}{% call ui::empty(text) %}<p>{{ text }}</p>{% endcall %}",
        ext = "html"
    )]
    struct Example<'a> {
        text: &'a str,
        tone: &'a str,
    }

    #[test]
    fn components_escape_content_and_use_appropriate_roles() {
        for (tone, role) in [
            ("success", "status"),
            ("error", "alert"),
            ("info", "status"),
        ] {
            let html = Example {
                text: "<script>bad</script>",
                tone,
            }
            .render()
            .expect("UI components render");
            assert!(!html.contains("<script>"));
            assert_eq!(html.matches("&#60;script&#62;").count(), 5);
            assert!(html.contains(&format!("ui-message--{tone}")));
            if role.is_empty() {
                assert!(!html.contains("role="));
            } else {
                assert!(html.contains(&format!("role=\"{role}\"")));
            }
        }
    }

    #[derive(Template)]
    #[template(
        source = "{% import \"ui.html\" as ui %}{% call ui::confirm(label, open) %}<form method=\"post\"></form>{% endcall %}",
        ext = "html"
    )]
    struct ConfirmExample<'a> {
        label: &'a str,
        open: bool,
    }

    #[test]
    fn confirm_disclosure_is_native_named_and_escaped() {
        for open in [false, true] {
            let html = ConfirmExample {
                label: "Delete <b>",
                open,
            }
            .render()
            .expect("Confirm renders");
            assert_eq!(html.contains("<details class=\"ui-confirm\" open>"), open);
            assert!(html.contains("<summary class=\"ui-button ui-button--secondary ui-button--icon ui-button--compact\" title=\"Delete &#60;b&#62;\">"));
            assert!(html.contains("<span class=\"visually-hidden\">Delete &#60;b&#62;</span>"));
            assert!(html.contains("data-ui-icon=\"trash\""));
            assert!(html.contains(
                "<div class=\"ui-confirm__panel\"><form method=\"post\"></form></div></details>"
            ));
            assert!(!html.contains("<b>"));
        }
    }

    #[derive(Template)]
    #[template(
        source = "{% import \"ui.html\" as ui %}{{ ui::icon(name, size) }}",
        ext = "html"
    )]
    struct IconExample<'a> {
        name: &'a str,
        size: &'a str,
    }

    #[test]
    fn lucide_icons_are_decorative_local_and_bounded() {
        for &(name, _) in ICONS {
            for size in ["normal", "small", "large"] {
                let html = IconExample { name, size }.render().expect("Icon renders");
                assert_eq!(html.matches("<svg ").count(), 1);
                assert!(html.contains("aria-hidden=\"true\" focusable=\"false\""));
                assert!(html.contains("viewBox=\"0 0 24 24\""));
                assert!(html.contains("stroke=\"currentColor\""));
                assert!(html.contains(&format!("data-ui-icon=\"{name}\"")));
                assert!(
                    [
                        "<path ",
                        "<circle ",
                        "<rect ",
                        "<line ",
                        "<polyline ",
                        "<polygon "
                    ]
                    .iter()
                    .any(|element| html.contains(element))
                );
                assert!(!html.contains("<script"));
                assert!(!html.contains("tabindex"));
                assert!(!html.contains("<title"));
                assert!(!html.contains("href="));
                assert_eq!(html.contains("ui-icon--small"), size == "small");
                assert_eq!(html.contains("ui-icon--large"), size == "large");
                let class = match size {
                    "small" => "ui-icon ui-icon--small",
                    "large" => "ui-icon ui-icon--large",
                    _ => "ui-icon",
                };
                assert!(
                    html.contains(&format!("class=\"{class}\"")),
                    "Exact CSS classes for {name}/{size}"
                );
            }
        }
    }

    #[test]
    fn decorative_icons_do_not_add_text_nodes_to_control_labels() {
        for &(name, _) in ICONS {
            let html = IconExample {
                name,
                size: "small",
            }
            .render()
            .expect("Icon renders");
            assert_eq!(html, html.trim(), "No whitespace around the icon {name}");
            assert!(
                !html.contains('\n'),
                "No whitespace text nodes inside SVG {name}"
            );
        }
    }

    #[tokio::test]
    async fn icon_catalogue_has_unique_names_and_bilingual_labels() {
        let mut names = std::collections::HashSet::new();
        for &(name, label) in ICONS {
            assert!(names.insert(name), "Duplicate icon {name}");
            for locale in [
                talocart_platform::locale::Locale::ITALIAN,
                talocart_platform::locale::Locale::ENGLISH,
            ] {
                let text = i18n::scope(locale, async { i18n::text(label) }).await;
                assert!(
                    !text.is_empty() && text != label,
                    "Missing {label} in {}",
                    locale.code()
                );
            }
        }
    }

    #[test]
    fn unknown_icon_names_and_sizes_cannot_inject_markup() {
        for name in [
            "unknown",
            "",
            "<script>bad</script>",
            "search\" onload=\"bad",
        ] {
            assert!(
                IconExample {
                    name,
                    size: "normal"
                }
                .render()
                .expect("Unknown icon renders")
                .trim()
                .is_empty()
            );
        }
        let html = IconExample {
            name: "search",
            size: "\"><script>bad</script>",
        }
        .render()
        .expect("Size is allowlisted");
        assert!(html.contains("class=\"ui-icon\""));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn preview_validates_unicode_limits_and_allowed_states() {
        for (label, status, valid) in [
            ("Nome", "active", true),
            ("Caffè", "inactive", true),
            ("  ", "active", false),
            ("Nome\n", "active", false),
            ("Nome", "unknown", false),
        ] {
            let query = ExampleQuery {
                label: label.into(),
                status: status.into(),
                preview: None,
            };
            assert_eq!(
                query.label_error().is_empty() && query.status_error().is_empty(),
                valid
            );
        }
        for (length, valid) in [(80, true), (81, false)] {
            let query = ExampleQuery {
                label: "è".repeat(length),
                status: example_state(),
                preview: None,
            };
            assert_eq!(query.label_error().is_empty(), valid);
        }
    }
}
