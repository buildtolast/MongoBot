use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let frontend_dir = Path::new(&manifest_dir).join("src").join("frontend");

    // --- Git hash ---
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    // --- Build timestamp ---
    let build_time = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={git_hash}");
    println!("cargo:rustc-env=BUILD_TIME={build_time}");

    // --- Collect CSS files (sorted) ---
    let styles_dir = frontend_dir.join("styles");
    let css_files = collect_sorted_files(&styles_dir, "css");

    // --- Collect JS files (sorted) ---
    let js_dir = frontend_dir.join("js");
    let js_files = collect_sorted_files(&js_dir, "js");

    // --- Read shell fragments ---
    let shell_head = fs::read_to_string(frontend_dir.join("shell_head.html"))
        .expect("Failed to read shell_head.html");
    let shell_mid = fs::read_to_string(frontend_dir.join("shell_mid.html"))
        .expect("Failed to read shell_mid.html");
    let shell_foot = fs::read_to_string(frontend_dir.join("shell_foot.html"))
        .expect("Failed to read shell_foot.html");

    // --- Read CSS ---
    let css_content: String = css_files
        .iter()
        .map(|path| {
            let content = fs::read_to_string(path)
                .unwrap_or_else(|_| panic!("Failed to read CSS file: {}", path.display()));
            format!("/* === {} === */\n{}\n", path.file_name().unwrap().to_string_lossy(), content)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // --- Read JS ---
    let js_content: String = js_files
        .iter()
        .map(|path| {
            let content = fs::read_to_string(path)
                .unwrap_or_else(|_| panic!("Failed to read JS file: {}", path.display()));
            format!("// === {} ===\n{}\n", path.file_name().unwrap().to_string_lossy(), content)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // --- Assemble index.html ---
    let index_html = format!("{shell_head}\n{css_content}\n{shell_mid}\n{js_content}\n{shell_foot}");

    let dest_path = Path::new(&out_dir).join("index.html");
    fs::write(&dest_path, &index_html).expect("Failed to write index.html");

    // --- Rebuild triggers ---
    println!("cargo:rerun-if-changed=src/frontend/shell_head.html");
    println!("cargo:rerun-if-changed=src/frontend/shell_mid.html");
    println!("cargo:rerun-if-changed=src/frontend/shell_foot.html");
    for f in css_files.iter().chain(js_files.iter()) {
        println!("cargo:rerun-if-changed={}", f.display());
    }

    let file_count = 3 + css_files.len() + js_files.len();
    println!(
        "cargo:warning=Frontend assembled: {} files -> {} bytes",
        file_count,
        index_html.len()
    );
}

fn collect_sorted_files(dir: &Path, extension: &str) -> Vec<std::path::PathBuf> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|_| panic!("Failed to read directory: {}", dir.display()))
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some(extension) {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    files.sort();
    files
}
