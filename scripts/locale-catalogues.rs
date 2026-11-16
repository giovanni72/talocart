//! Shared build-time discovery: only complete admin/store catalogue pairs are selectable.
use std::{collections::BTreeSet, error::Error, fs, path::Path};
pub type Catalogue = (String, String, String);
pub fn discover(root: &Path) -> Result<Vec<Catalogue>, Box<dyn Error>> {
    let mut languages = Vec::new();
    for entry in fs::read_dir(root)? {
        let directory = entry?.path();
        if !directory.is_dir() {
            continue;
        }
        let Some(code) = directory.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        let admin = directory.join("admin.ftl");
        let store = directory.join("store.ftl");
        if !admin.is_file() || !store.is_file() {
            continue;
        }
        if !code
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric()))
            || !code.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        {
            return Err(format!("Invalid catalogue directory: {code}").into());
        }
        let admin = fs::read_to_string(admin)?;
        let store = fs::read_to_string(store)?;
        let name = admin
            .lines()
            .find_map(|line| line.strip_prefix("# language-name: "))
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
            .ok_or_else(|| format!("Missing native language name: {code}"))?;
        languages.push((
            code.to_owned(),
            name.to_owned(),
            format!("{admin}\n{store}"),
        ));
    }
    languages.sort_by_key(|(code, _, _)| (code != "it", code != "en", code.clone()));
    let first = languages
        .first()
        .ok_or("No complete translation catalogue pair available")?;
    let keys = |source: &str| -> BTreeSet<String> {
        source
            .lines()
            .filter(|line| !line.starts_with(' ') && !line.starts_with('#'))
            .filter_map(|line| line.split_once(" =").map(|(key, _)| key.to_owned()))
            .collect()
    };
    let expected = keys(&first.2);
    for (code, _, source) in &languages {
        if keys(source) != expected {
            return Err(format!("Incomplete translation catalogue: {code}").into());
        }
    }
    Ok(languages)
}
