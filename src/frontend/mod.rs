/// The embedded single-page frontend HTML, assembled at compile time by `build.rs`.
pub const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.html"));
