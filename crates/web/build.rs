#[path = "../../scripts/locale-catalogues.rs"]
mod catalogues;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let locales = manifest.join("locales");
    println!("cargo:rerun-if-changed={}", locales.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest
            .join("../../scripts/locale-catalogues.rs")
            .display()
    );
    let languages = catalogues::discover(&locales)?;
    let resources: Vec<_> = languages
        .iter()
        .map(|(code, _, source)| (code, source))
        .collect();
    std::fs::write(
        std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("catalogues.rs"),
        format!("const RESOURCES: &[(&str, &str)] = &{resources:?};"),
    )?;
    Ok(())
}
