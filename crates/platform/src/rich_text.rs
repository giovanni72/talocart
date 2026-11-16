//! Versioned rich-text encoding in existing description fields. Unmarked values
//! are always literal text, including imported strings containing HTML-looking tags.
use std::collections::{HashMap, HashSet};

pub const PREFIX: &str = "<!--talocart-rich-text:v1-->";
pub fn is_html(value: &str) -> bool {
    value.starts_with(PREFIX)
}
pub fn source(value: &str) -> &str {
    value.strip_prefix(PREFIX).unwrap_or(value)
}
/// Encode only an explicit HTML submission. Sanitization belongs to validation;
/// keeping the submitted source here lets an invalid form retain the whole draft.
pub fn submitted(value: &str, format: &str) -> String {
    if format == "html" {
        format!("{PREFIX}{value}")
    } else {
        value.to_owned()
    }
}
pub fn sanitize(html: &str) -> String {
    ammonia::Builder::new()
        .tags(
            HashSet::from([
                "p",
                "br",
                "h2",
                "h3",
                "strong",
                "em",
                "u",
                "s",
                "ul",
                "ol",
                "li",
                "blockquote",
                "table",
                "caption",
                "thead",
                "tbody",
                "tfoot",
                "tr",
                "th",
                "td",
            ])
            .into_iter()
            .chain(["a"])
            .collect(),
        )
        .tag_attributes(HashMap::from([
            ("a", HashSet::from(["href", "title"])),
            ("th", HashSet::from(["colspan", "rowspan", "scope"])),
            ("td", HashSet::from(["colspan", "rowspan"])),
        ]))
        .generic_attributes(HashSet::new())
        .url_schemes(HashSet::from(["http", "https", "mailto", "tel"]))
        .link_rel(None)
        .clean(html)
        .to_string()
}
/// Called only after the owner's length/control-character validation.
pub fn normalize(value: &str) -> String {
    if is_html(value) {
        let html = sanitize(source(value));
        if plain_html(&html).trim().is_empty() {
            String::new()
        } else {
            format!("{PREFIX}{html}")
        }
    } else {
        value.to_owned()
    }
}
fn plain_html(html: &str) -> String {
    let spaced = html
        .replace("</p>", "</p> ")
        .replace("</h2>", "</h2> ")
        .replace("</h3>", "</h3> ")
        .replace("</li>", "</li> ")
        .replace("</caption>", "</caption> ")
        .replace("</th>", "</th> ")
        .replace("</td>", "</td> ")
        .replace("<br>", " ");
    let text = ammonia::Builder::new()
        .tags(HashSet::new())
        .tag_attributes(HashMap::new())
        .generic_attributes(HashSet::new())
        .link_rel(None)
        .clean(&spaced)
        .to_string();
    html_escape::decode_html_entities(&text).into_owned()
}
pub fn plain(value: &str) -> String {
    if is_html(value) {
        plain_html(&sanitize(source(value)))
    } else {
        value.to_owned()
    }
}
/// The only HTML accepted for unescaped rendering; also protects legacy/import paths.
pub fn render(value: &str) -> String {
    if is_html(value) {
        sanitize(source(value))
    } else {
        value
            .replace("\r\n", "\n")
            .split("\n\n")
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| {
                format!(
                    "<p>{}</p>",
                    html_escape::encode_text(p).replace('\n', "<br>")
                )
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn size_tables_keep_structure_and_plain_text_separators_without_unsafe_attributes() {
        let raw = submitted(
            "<table style='width:900px' onclick='bad()'><caption>Sizes</caption><thead><tr><th scope='col'>Size</th><th>Width</th></tr></thead><tbody><tr><th scope='row' rowspan='2'>S</th><td colspan='2' onmouseover='bad()'>50 &amp; 51</td></tr><tr><td>52</td></tr></tbody></table><script>bad()</script>",
            "html",
        );
        let normalized = normalize(&raw);
        let html = render(&normalized);
        assert!(
            html.contains("<table><caption>Sizes</caption><thead>"),
            "{html}"
        );
        assert!(html.contains("<th scope=\"row\" rowspan=\"2\">S</th>"));
        assert!(html.contains("<td colspan=\"2\">50 &amp; 51</td>"));
        for unsafe_content in ["style=", "onclick", "onmouseover", "<script", "bad()"] {
            assert!(!html.contains(unsafe_content), "{html}");
        }
        assert_eq!(normalize(&normalized), normalized);
        assert_eq!(plain(&normalized).trim(), "Sizes Size Width S 50 & 51 52");
    }
    #[test]
    fn legacy_is_literal_and_never_rewritten() {
        let old = "First <b>literal</b> & details.\nNext\n\n<script>alert(1)</script>";
        assert_eq!(normalize(old), old);
        assert_eq!(source(old), old);
        assert_eq!(plain(old), old);
        let html = render(old);
        assert!(html.contains("&lt;b&gt;literal&lt;/b&gt;"));
        assert!(html.contains("<br>Next</p><p>&lt;script&gt;"));
    }
    #[test]
    fn rich_text_is_sanitized_idempotent_and_seo_is_plain() {
        let raw = submitted(
            "<h2 onclick='x()'>Title</h2><p>A <strong>bold</strong> &amp; <u>underlined</u></p><script>alert(1)</script><iframe src='x'></iframe><a href='javascript:alert(1)' style='color:red'>link</a>",
            "html",
        );
        let normalized = normalize(&raw);
        assert!(!normalized.contains("onclick"));
        assert!(!normalized.contains("javascript:"));
        assert!(!normalized.contains("<script"));
        assert!(!normalized.contains("<iframe"));
        assert!(!normalized.contains("style="));
        assert!(normalized.contains("<strong>bold</strong>"));
        assert_eq!(normalize(&normalized), normalized);
        assert!(plain(&normalized).contains("Title A bold & underlined"));
        assert!(!plain(&normalized).contains('<'));
    }
    #[test]
    fn empty_html_and_unsafe_urls() {
        assert_eq!(
            normalize(&submitted("<p><br></p><script>x</script>", "html")),
            ""
        );
        for url in [
            "javascript:x",
            "data:text/html,x",
            "&#106;avascript:x",
            "vbscript:x",
        ] {
            let html = sanitize(&format!("<a href='{url}'>x</a>"));
            assert!(!html.contains("href="), "{html}");
        }
        assert!(sanitize("<a href='/contact'>Contact</a>").contains("href=\"/contact\""));
    }
}
