use std::path::PathBuf;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR is set by cargo when running build scripts");
    let manifest_dir = manifest_dir
        .strip_prefix(r"\\?\")
        .unwrap_or(&manifest_dir)
        .to_string();
    // Built-in skills ship from the bundled skill kit under crates/skills.
    let samples_dir = PathBuf::from(&manifest_dir).join("assets/bundled");
    let samples_dir = if samples_dir.is_dir() {
        samples_dir
    } else {
        PathBuf::from(&manifest_dir)
            .join("src")
            .join("assets")
            .join("samples")
    };

    println!("cargo:rerun-if-changed={}", samples_dir.display());
    println!(
        "cargo:rustc-env=DEVO_SKILLS_SAMPLES_DIR={}",
        samples_dir.display()
    );
}
