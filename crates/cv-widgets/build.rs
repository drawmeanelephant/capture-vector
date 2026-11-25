use flate2::read::GzDecoder;
use std::io::Read;
use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = manifest.join("assets/fonts/DroidSansFallback.ttf.gz");
    println!("cargo:rerun-if-changed={}", source.display());
    let compressed = std::fs::read(&source).expect("read compressed Droid Sans Fallback");
    let mut raw = Vec::new();
    GzDecoder::new(&compressed[..])
        .read_to_end(&mut raw)
        .expect("decompress Droid Sans Fallback");
    assert_eq!(
        raw.len(),
        3_451_900,
        "Droid Sans Fallback length changed; update fonts.rs and the license/cost notes"
    );
    let out =
        Path::new(&std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("DroidSansFallback.ttf");
    std::fs::write(out, raw).expect("write decompressed Droid Sans Fallback");
}
