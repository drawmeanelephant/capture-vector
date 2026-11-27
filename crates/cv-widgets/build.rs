use std::path::Path;

#[path = "src/font_asset.rs"]
mod font_asset;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = manifest.join("assets/fonts/DroidSansFallback.ttf.gz");
    println!("cargo:rerun-if-changed={}", source.display());
    let compressed = std::fs::read(&source).expect("read compressed Droid Sans Fallback");
    let raw = font_asset::decode_bounded(&compressed)
        .unwrap_or_else(|e| panic!("Droid Sans Fallback failed verification: {e}"));
    let out =
        Path::new(&std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("DroidSansFallback.ttf");
    std::fs::write(out, raw).expect("write decompressed Droid Sans Fallback");
}
