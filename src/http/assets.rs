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
    use std::collections::HashSet;

    fn asset_text(name: &str) -> String {
        let file = EmbeddedConsoleAssets::get(name)
            .unwrap_or_else(|| panic!("{name} must be embedded from static/console"));
        String::from_utf8(file.data.as_ref().to_vec())
            .unwrap_or_else(|e| panic!("{name} must be UTF-8: {e}"))
    }

    fn asset_names() -> HashSet<String> {
        EmbeddedConsoleAssets::iter()
            .map(|n| n.as_ref().to_string())
            .collect()
    }

    #[test]
    fn embeds_console_index_html() {
        let html = asset_text("index.html");
        assert!(!html.trim().is_empty(), "index.html must not be empty");
        assert!(
            html.to_ascii_lowercase().contains("<html"),
            "index.html must be HTML"
        );
        assert!(
            html.contains("lang=\"ja\"") || html.contains("lang='ja'"),
            "console must declare Japanese lang"
        );
    }

    #[test]
    fn embeds_console_css_and_js() {
        // Design File Structure: static/console/{index.html,console.css,console.js}
        let names = asset_names();
        assert!(
            names.contains("console.css"),
            "expected console.css among {names:?}"
        );
        assert!(
            names.contains("console.js"),
            "expected console.js among {names:?}"
        );
        assert!(
            !asset_text("console.css").trim().is_empty(),
            "console.css must not be empty"
        );
        assert!(
            !asset_text("console.js").trim().is_empty(),
            "console.js must not be empty"
        );
    }

    #[test]
    fn console_html_has_japanese_guidance_and_controls() {
        // Requirements 2.1, 3.1, 4.1–4.2, 7.1–7.2 — Japanese labels / short guidance.
        let html = asset_text("index.html");
        let js = asset_text("console.js");
        let combined = format!("{html}\n{js}");

        for needle in [
            "ヘルス",
            "解析",
            "ECL",
            "ダウンロード",
            "処理中",
            "選択",
        ] {
            assert!(
                combined.contains(needle),
                "console must include Japanese UI text containing '{needle}'"
            );
        }

        // Short guidance on first open (7.2).
        assert!(
            html.contains("案内") || html.contains("使い方") || html.contains("この画面"),
            "index.html must include a short Japanese guidance block"
        );

        // Layout: left ~1/3 input, right ~2/3 output (Bulma columns).
        assert!(
            html.contains("is-one-third") && html.contains("is-two-thirds"),
            "index.html must use a ~1/3 + ~2/3 column layout"
        );

        // Health is auto-polled (no dedicated health button).
        assert!(
            !html.contains("id=\"health-btn\"") && !html.contains("id='health-btn'"),
            "health must not use a click button; use top-right status + polling"
        );
        assert!(
            js.contains("setInterval") && js.contains("10000"),
            "console.js must poll /health every 10 seconds"
        );

        // JSON is the console default output format.
        assert!(
            html.contains("value=\"json\"") && html.contains("selected"),
            "JSON must be the default selected output format"
        );
        assert!(
            js.contains("\"json\"") || js.contains("'json'"),
            "console.js must default analyze format to json"
        );

        // Interactive controls for file / analyze / download / status regions.
        assert!(
            html.contains("id=\"") || html.contains("id='"),
            "index.html must expose element ids for ConsoleClient JS"
        );
    }

    #[test]
    fn console_js_calls_only_health_and_analyze_same_origin() {
        // Requirements 2.1–2.2, 3.1–3.2, 6.1, 6.3 — relative same-origin only; no license URLs.
        let js = asset_text("console.js");

        assert!(js.contains("/health"), "console.js must call GET /health");
        assert!(
            js.contains("/v1/analyze"),
            "console.js must call POST /v1/analyze"
        );

        // Multipart field name per design ConsoleClient API contract.
        assert!(
            js.contains("\"ecl\"") || js.contains("'ecl'"),
            "console.js must send multipart field name 'ecl'"
        );

        let forbidden = [
            "/v1/license",
            "/license",
            "license/meter",
            "meter_usage",
            "authorize",
            "http://",
            "https://",
        ];
        for needle in forbidden {
            assert!(
                !js.contains(needle),
                "console.js must not reference '{needle}' (same-origin relative URLs only; no license metering)"
            );
        }
    }

    #[test]
    fn console_js_handles_loading_errors_download_and_missing_file() {
        // Requirements 3.3–3.4, 4.1–4.2, 5.1–5.3, 8.2 — loading, download, JP errors.
        let js = asset_text("console.js");
        let html = asset_text("index.html");
        let combined = format!("{html}\n{js}");

        assert!(
            combined.contains("処理中"),
            "must show Japanese in-progress / loading text"
        );
        assert!(
            js.contains("download") || js.contains("ダウンロード"),
            "must offer local download of analysis result"
        );
        assert!(
            js.contains("FormData") && (js.contains("append") || js.contains(".set(")),
            "analyze must build multipart FormData"
        );

        // Missing-file guard before send (3.4).
        let missing_hints = [
            "選択されていません",
            "選択してください",
            "ファイルがありません",
            "未選択",
        ];
        assert!(
            missing_hints.iter().any(|h| combined.contains(h)),
            "must show Japanese message when file is missing; looked for {missing_hints:?}"
        );

        // Failure messaging (5.x / health 2.3).
        let fail_hints = ["失敗", "エラー", "到達できません", "タイムアウト"];
        assert!(
            fail_hints.iter().any(|h| combined.contains(h)),
            "must include Japanese failure wording; looked for {fail_hints:?}"
        );

        // Prefer safe text insertion for results (design Security).
        assert!(
            js.contains("textContent") || js.contains("textContent ="),
            "result display should use textContent (XSS-safe)"
        );
    }

    #[test]
    fn console_assets_exclude_out_of_scope_and_spa() {
        // Requirements 10.1–10.3 / design Non-goals: no clinical / auth product / billing UI / SPA.
        let names = asset_names();
        assert!(
            !names.iter().any(|s| {
                s.contains("node_modules")
                    || s.ends_with("package.json")
                    || s.ends_with(".map")
                    || s.contains("chunk-")
            }),
            "embedded console assets must not include SPA/Node build artifacts: {names:?}"
        );
        assert!(
            names.contains("index.html"),
            "expected index.html among {names:?}"
        );

        let combined = format!(
            "{}\n{}\n{}",
            asset_text("index.html"),
            asset_text("console.js"),
            asset_text("console.css")
        );
        let lower = combined.to_ascii_lowercase();
        for needle in ["oauth", "react", "vue.", "angular"] {
            assert!(
                !lower.contains(needle),
                "out-of-scope SPA/IAM marker '{needle}' must not appear"
            );
        }
        for needle in [
            "波形編集",
            "帳票",
            "臨床最終判定",
            "課金ダッシュボード",
            "ライセンスサーバー",
        ] {
            assert!(
                !combined.contains(needle),
                "out-of-scope Japanese feature '{needle}' must not appear"
            );
        }
    }

    #[test]
    fn console_js_does_not_impose_client_size_limit() {
        // Requirement 8.1 — size/timeout follow upstream; UI must not set a looser client max.
        let js = asset_text("console.js");
        let banned = [
            "maxSize",
            "MAX_SIZE",
            "max_body",
            "file.size >",
            "sizeLimit",
        ];
        for needle in banned {
            assert!(
                !js.contains(needle),
                "console.js must not enforce a UI-local size cap ({needle}); defer to http-api"
            );
        }
    }

    #[test]
    fn embed_trait_is_available_at_compile_time() {
        // Requirement 1.2 / 9.1: assets resolve from the binary embed, not an
        // external UI server directory at runtime.
        fn assert_embed<T: Embed>() {}
        assert_embed::<EmbeddedConsoleAssets>();
    }
}
