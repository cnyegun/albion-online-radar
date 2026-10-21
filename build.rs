//! Embeds `assets/resources/*.webp` as `ICONS: &[(name, bytes)]`.

use std::fmt::Write;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/resources");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("read assets/resources")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "webp"))
        .collect();
    files.sort();

    let mut out = String::from("pub static ICONS: &[(&str, &[u8])] = &[\n");
    for p in files {
        let name = p.file_stem().unwrap().to_string_lossy();
        writeln!(out, "    ({name:?}, include_bytes!({:?})),", p.display().to_string()).unwrap();
    }
    out.push_str("];\n");
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("icons.rs"), out).unwrap();
}
