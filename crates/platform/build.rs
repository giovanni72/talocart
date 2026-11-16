#[path = "../../scripts/locale-catalogues.rs"]
mod catalogues;
#[path = "../../scripts/build-version.rs"]
mod version;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    version::emit(
        &manifest.join("../.."),
        &std::env::var("CARGO_PKG_VERSION")?,
    );
    let locales = manifest.join("../web/locales");
    println!("cargo:rerun-if-changed={}", locales.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest
            .join("../../scripts/locale-catalogues.rs")
            .display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("../../migrations").display()
    );
    let languages = catalogues::discover(&locales)?;
    let supported: Vec<_> = languages
        .iter()
        .map(|(code, name, _)| (code, name))
        .collect();
    let default = languages.first().ok_or("No available language")?;
    let generated = format!(
        "pub const SUPPORTED: &[(&str, &str)] = &{supported:?};\npub const DEFAULT_CODE: &str = {:?};",
        default.0
    );
    std::fs::write(
        std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("locales.rs"),
        generated,
    )?;
    Ok(())
}
