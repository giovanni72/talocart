//! Test-only inventory of shared and compiled-theme templates.
use std::path::{Path, PathBuf};

pub(crate) fn sources() -> Vec<(PathBuf, String)> {
    fn visit(directory: &Path, result: &mut Vec<(PathBuf, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(directory)
            .expect("Template directory exists")
            .map(|entry| entry.expect("Template entry readable").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                visit(&path, result);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "html")
            {
                let source = std::fs::read_to_string(&path).expect("Template source readable");
                result.push((path, source));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut result = Vec::new();
    visit(&root.join("templates"), &mut result);
    for theme in talocart_platform::themes::COMPILED {
        visit(
            &root.join("../../themes").join(theme.id()).join("templates"),
            &mut result,
        );
    }
    result
}
