use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => panic!("failed to read UI dist directory {}: {err}", dir.display()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("asset inside dist root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
}

fn mime_for_rel_path(rel: &str) -> &'static str {
    let ext = Path::new(rel)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "json" => "application/json; charset=utf-8",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        _ => "application/octet-stream",
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let dist_dir = fs::canonicalize(manifest_dir.join("../../apps/flowscale-ui/dist"))
        .expect("apps/flowscale-ui/dist missing — run `npm run build` in apps/flowscale-ui first");
    let icon_path = fs::canonicalize(manifest_dir.join("../../assets/flowscale.ico"))
        .expect("assets/flowscale.ico missing");

    println!("cargo:rerun-if-changed={}", dist_dir.display());
    println!("cargo:rerun-if-changed={}", icon_path.display());

    let mut files = Vec::new();
    collect_files(&dist_dir, &dist_dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let generated = out_dir.join("embedded_dist.rs");
    let mut file = fs::File::create(&generated).expect("create embedded_dist.rs");

    writeln!(
        file,
        "pub fn get_embedded_asset(rel_path: &str) -> Option<(&'static [u8], &'static str)> {{"
    )
    .unwrap();
    writeln!(file, "    match rel_path {{").unwrap();
    for (rel, full) in &files {
        println!("cargo:rerun-if-changed={}", full.display());
        let mime = mime_for_rel_path(rel);
        let full_canon = fs::canonicalize(full).unwrap_or_else(|_| full.clone());
        let clean_path = full_canon.to_string_lossy();
        let clean_path = clean_path.strip_prefix(r"\\?\").unwrap_or(&clean_path);
        writeln!(
            file,
            "        {rel:?} => Some((include_bytes!({clean_path:?}), {mime:?})),"
        )
        .unwrap();
    }
    writeln!(file, "        _ => None,").unwrap();
    writeln!(file, "    }}").unwrap();
    writeln!(file, "}}").unwrap();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon_str = icon_path.to_string_lossy();
        let icon_str = icon_str.strip_prefix(r"\\?\").unwrap_or(&icon_str);
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id(icon_str, "1");
        res.set("ProductName", "FlowScale");
        res.set("FileDescription", "FlowScale GPU Frame Generation & Upscaling");
        res.compile().expect("failed to compile Windows resources");
    }
}
