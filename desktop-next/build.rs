fn main() {
    validate_asset("assets/logo.ico", 6);
    validate_asset("assets/logo-32.rgba", 32 * 32 * 4);

    println!("cargo:rerun-if-changed=assets/logo.ico");
    println!("cargo:rerun-if-changed=assets/logo-32.rgba");
    println!("cargo:rerun-if-changed=desktop-next.rc");

    #[cfg(windows)]
    embed_resource::compile("desktop-next.rc", embed_resource::NONE);
}

fn validate_asset(path: &str, min_len: u64) {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let path = std::path::Path::new(&manifest_dir).join(path);
    let metadata = std::fs::metadata(&path)
        .unwrap_or_else(|error| panic!("required asset {} is missing: {error}", path.display()));
    assert!(
        metadata.len() >= min_len,
        "required asset {} is too small: {} bytes",
        path.display(),
        metadata.len()
    );
}
