use std::path::PathBuf;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR is set by cargo when running build scripts");
    let manifest_dir = manifest_dir
        .strip_prefix(r"\\?\")
        .unwrap_or(&manifest_dir)
        .to_string();
    // Built-in skills ship from the bundled skill kit under crates/skills.
    let bundled_dir = PathBuf::from(&manifest_dir).join("assets/bundled");
    assert!(
        bundled_dir.is_dir(),
        "bundled skill assets are missing: {}",
        bundled_dir.display()
    );

    println!("cargo:rerun-if-changed={}", bundled_dir.display());
    println!(
        "cargo:rustc-env=DEVO_SKILLS_BUNDLED_DIR={}",
        bundled_dir.display()
    );
}
