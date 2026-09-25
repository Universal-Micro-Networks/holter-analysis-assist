//! Embedded console static assets (`static/console`).
//!
//! Design component: EmbeddedConsoleAssets.
//! Route wiring belongs to StaticUiHandler (tasks 2.2 / 3.1) — not here.

use rust_embed::Embed;

/// Build-time embedded console UI assets under `static/console/`.
///
/// Release builds ship these bytes inside the HTTP binary (requirements 1.2, 9.1).
/// Debug builds may read from the filesystem via rust-embed defaults.
#[derive(Embed)]
#[folder = "static/console/"]
pub struct EmbeddedConsoleAssets;

#[cfg(test)]
mod tests {
    use super::EmbeddedConsoleAssets;
    use rust_embed::Embed;

    #[test]
    fn embeds_placeholder_index_html() {
        let file = EmbeddedConsoleAssets::get("index.html")
            .expect("index.html must be embedded from static/console");
        let html = std::str::from_utf8(file.data.as_ref()).expect("index.html must be UTF-8");
        assert!(
            !html.trim().is_empty(),
            "placeholder index.html must not be empty"
        );
        assert!(
            html.to_ascii_lowercase().contains("<html"),
            "placeholder must be HTML"
        );
    }

    #[test]
    fn console_asset_tree_has_no_spa_bundle() {
        // Requirement 10.1–10.3 / design: no SPA / node_modules in embed folder.
        let names: Vec<_> = EmbeddedConsoleAssets::iter().collect();
        assert!(
            !names.iter().any(|n| {
                let s = n.as_ref();
                s.contains("node_modules")
                    || s.ends_with("package.json")
                    || s.ends_with(".map")
                    || s.contains("chunk-")
            }),
            "embedded console assets must not include SPA/Node build artifacts: {names:?}"
        );
        assert!(
            names.iter().any(|n| n.as_ref() == "index.html"),
            "expected index.html among {names:?}"
        );
    }

    #[test]
    fn embed_trait_is_available_at_compile_time() {
        // Requirement 1.2 / 9.1: assets resolve from the binary embed, not an
        // external UI server directory at runtime.
        fn assert_embed<T: Embed>() {}
        assert_embed::<EmbeddedConsoleAssets>();
    }
}
