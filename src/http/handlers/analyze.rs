//! POST `/v1/analyze` — multipart ECL → canonical `analyze_ecl_with_source` only.
//!
//! HTTP must not call the license inference-gate APIs directly; metering stays
//! inside the library entry via the process-wide LicenseGate.

use crate::analyze::{analyze_ecl_with_model, analyze_ecl_with_source, AnalyzeError};
use crate::http::error::HttpError;
use crate::http::response::ResponseCodec;
use crate::http::state::{AppState, SharedModel};
use crate::perf::AnalyzePerf;
use crate::phase2::{ExecutionProviderKind, InferenceOptions};
use crate::preprocess::parse_ecl_filename;
use axum::extract::rejection::QueryRejection;
use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Analyze HTTP adapter (design: AnalyzeHandler).
pub struct AnalyzeHandler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Csv,
    Json,
}

/// Query string of `POST /v1/analyze`; unknown parameters are ignored.
#[derive(Debug, Default, Deserialize)]
pub struct AnalyzeQuery {
    format: Option<String>,
}

struct AnalyzeRequestParts {
    ecl_dir: TempDir,
    ecl_path: PathBuf,
    format: OutputFormat,
    provider: Option<ExecutionProviderKind>,
    max_windows: Option<usize>,
}

impl AnalyzeHandler {
    /// Axum handler: multipart ECL → spawn_blocking → `analyze_ecl_with_source`.
    ///
    /// Output format precedence: multipart `format` field > `?format=` query >
    /// `Accept` header > CSV.
    pub async fn post(
        State(state): State<AppState>,
        query: Result<Query<AnalyzeQuery>, QueryRejection>,
        headers: HeaderMap,
        multipart: Multipart,
    ) -> Result<Response, HttpError> {
        reject_oversized_content_length(&headers, state.max_body_bytes())?;

        let Query(query) =
            query.map_err(|e| HttpError::invalid_input(format!("invalid query string: {e}")))?;
        let format_query = query
            .format
            .as_deref()
            .map(|raw| parse_output_format(raw, "format query"))
            .transpose()?;

        let parts =
            parse_multipart(multipart, format_query, &headers, state.max_body_bytes()).await?;
        // Validate ECL filename contract before calling the canonical entry (no meter).
        parse_ecl_filename(&parts.ecl_path).map_err(|e| HttpError::from(AnalyzeError::from(e)))?;

        let model_source = state.model_source.clone();
        let shared = state.shared_model().clone();
        let startup = state.config.inference_options();
        let provider = parts.provider.unwrap_or(startup.provider);
        let max_windows = parts.max_windows;
        let ecl_path = parts.ecl_path.clone();
        let format = parts.format;

        let out_dir = TempDir::new()
            .map_err(|e| HttpError::internal(format!("temp output dir failed: {e}")))?;
        let out_csv = out_dir.path().join("beat_results.csv");

        let analyze_result = tokio::task::spawn_blocking(move || {
            run_analyze(
                &shared,
                &model_source,
                &startup,
                provider,
                &ecl_path,
                &out_csv,
                max_windows,
            )
        })
        .await
        .map_err(|e| HttpError::internal(format!("analyze task join failed: {e}")))?;

        // Keep temp dirs alive until analyze returns.
        drop(parts.ecl_dir);
        drop(out_dir);

        let (rows, summary) = analyze_result.map_err(|e| {
            let err = HttpError::from(e);
            eprintln!("{}", failure_log_line(&err));
            err
        })?;

        let encode_started = Instant::now();
        let (content_type, body) = match format {
            OutputFormat::Csv => ("text/csv; charset=utf-8", ResponseCodec::to_csv(&rows)?),
            OutputFormat::Json => ("application/json", ResponseCodec::to_json(&rows, &summary)?),
        };
        eprintln!("{}", perf_log_line(&summary.perf, encode_started.elapsed()));

        Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, HeaderValue::from_static(content_type))],
            body,
        )
            .into_response())
    }

    /// Router fragment for `/v1/analyze` with body-size limit from AppState.
    pub fn routes(state: AppState) -> Router {
        let limit = state.max_body_bytes();
        Router::new()
            .route("/v1/analyze", post(Self::post))
            .layer(DefaultBodyLimit::max(limit))
            .with_state(state)
    }
}

/// Options for a per-request session: the request's provider with the
/// startup batch size and CUDA tuning.
fn ephemeral_options(
    startup: &InferenceOptions,
    provider: ExecutionProviderKind,
) -> InferenceOptions {
    InferenceOptions {
        provider,
        ..*startup
    }
}

/// Server-log line for one finished analyze request.
fn perf_log_line(perf: &AnalyzePerf, response_encode: Duration) -> String {
    format!(
        "holter-http-api: {} response_encode_ms={:.1}",
        perf.log_line(),
        response_encode.as_secs_f64() * 1000.0
    )
}

/// Server-log line for an analyze request that failed after upload
/// (license denial, inference or post-processing failure).
fn failure_log_line(err: &HttpError) -> String {
    format!(
        "holter-http-api: analyze failed status={} code={} message={}",
        err.status_code().as_u16(),
        err.error_code(),
        err.message()
    )
}

fn run_analyze(
    shared: &SharedModel,
    model_source: &crate::model_source::ModelSource,
    startup: &InferenceOptions,
    provider: ExecutionProviderKind,
    ecl_path: &Path,
    out_csv: &Path,
    max_windows: Option<usize>,
) -> Result<
    (
        Vec<crate::analyze::BeatResultRow>,
        crate::analyze::AnalyzeSummary,
    ),
    AnalyzeError,
> {
    match shared {
        SharedModel::Resident(model) => {
            let mut guard = model
                .lock()
                .map_err(|_| AnalyzeError::Post("resident model lock poisoned".into()))?;
            // Per-request provider override only when it disagrees with the loaded EP.
            // `Auto` always reuses the resident session (already resolved at startup).
            let need_reload = match provider {
                ExecutionProviderKind::Auto => false,
                other => other != guard.provider(),
            };
            if need_reload {
                eprintln!(
                    "holter-http-api: per-request provider={provider} != resident={}; loading ephemeral session",
                    guard.provider()
                );
                drop(guard);
                analyze_ecl_with_source(
                    ecl_path,
                    model_source,
                    out_csv,
                    max_windows,
                    &ephemeral_options(startup, provider),
                )
            } else {
                analyze_ecl_with_model(ecl_path, &mut guard, out_csv, max_windows)
            }
        }
        SharedModel::Ephemeral(source) => analyze_ecl_with_source(
            ecl_path,
            source,
            out_csv,
            max_windows,
            &ephemeral_options(startup, provider),
        ),
    }
}

fn reject_oversized_content_length(
    headers: &HeaderMap,
    max_body_bytes: usize,
) -> Result<(), HttpError> {
    if let Some(raw) = headers.get(header::CONTENT_LENGTH) {
        let len = raw
            .to_str()
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| HttpError::invalid_input("invalid Content-Length"))?;
        if len > max_body_bytes {
            return Err(HttpError::payload_too_large(format!(
                "request body exceeds max_body_bytes ({max_body_bytes})"
            )));
        }
    }
    Ok(())
}

async fn parse_multipart(
    mut multipart: Multipart,
    format_query: Option<OutputFormat>,
    headers: &HeaderMap,
    max_body_bytes: usize,
) -> Result<AnalyzeRequestParts, HttpError> {
    let mut ecl_bytes: Option<(String, Vec<u8>)> = None;
    let mut format_field: Option<String> = None;
    let mut provider_field: Option<String> = None;
    let mut max_windows_field: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| HttpError::invalid_input(format!("multipart parse failed: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "ecl" => {
                let filename = field
                    .file_name()
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        HttpError::invalid_input("multipart field 'ecl' requires a filename")
                    })?;
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| HttpError::invalid_input(format!("failed to read ecl: {e}")))?;
                if data.len() > max_body_bytes {
                    return Err(HttpError::payload_too_large(format!(
                        "ecl payload exceeds max_body_bytes ({max_body_bytes})"
                    )));
                }
                if data.is_empty() {
                    return Err(HttpError::invalid_input("ecl upload must not be empty"));
                }
                ecl_bytes = Some((filename, data.to_vec()));
            }
            "format" => {
                format_field = Some(field_text(field).await?);
            }
            "provider" => {
                provider_field = Some(field_text(field).await?);
            }
            "max_windows" => {
                max_windows_field = Some(field_text(field).await?);
            }
            _ => {
                // Ignore unknown fields for forward compatibility.
                let _ = field.bytes().await;
            }
        }
    }

    let (filename, data) = ecl_bytes
        .ok_or_else(|| HttpError::invalid_input("missing required multipart field 'ecl'"))?;

    let format = resolve_output_format(
        format_field.as_deref(),
        format_query,
        headers.get(header::ACCEPT),
    )?;
    let provider = match provider_field.as_deref() {
        Some(raw) => Some(
            ExecutionProviderKind::from_str(raw.trim())
                .map_err(|e| HttpError::invalid_input(format!("invalid provider: {e}")))?,
        ),
        None => None,
    };
    let max_windows = match max_windows_field.as_deref() {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(HttpError::invalid_input("max_windows must not be empty"));
            }
            Some(
                trimmed
                    .parse::<usize>()
                    .map_err(|_| HttpError::invalid_input("invalid max_windows"))?,
            )
        }
        None => None,
    };

    let ecl_dir =
        TempDir::new().map_err(|e| HttpError::internal(format!("temp ecl dir failed: {e}")))?;
    let ecl_path = ecl_dir.path().join(safe_basename(&filename));
    std::fs::write(&ecl_path, &data)
        .map_err(|e| HttpError::internal(format!("failed to write temp ecl: {e}")))?;

    Ok(AnalyzeRequestParts {
        ecl_dir,
        ecl_path,
        format,
        provider,
        max_windows,
    })
}

async fn field_text(field: axum::extract::multipart::Field<'_>) -> Result<String, HttpError> {
    let bytes = field
        .bytes()
        .await
        .map_err(|e| HttpError::invalid_input(format!("failed to read field: {e}")))?;
    String::from_utf8(bytes.to_vec())
        .map_err(|_| HttpError::invalid_input("field must be utf-8 text"))
}

fn parse_output_format(raw: &str, source: &str) -> Result<OutputFormat, HttpError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "csv" => Ok(OutputFormat::Csv),
        "json" => Ok(OutputFormat::Json),
        other => Err(HttpError::invalid_input(format!(
            "invalid {source} '{other}' (expected csv|json)"
        ))),
    }
}

fn resolve_output_format(
    format_field: Option<&str>,
    format_query: Option<OutputFormat>,
    accept: Option<&HeaderValue>,
) -> Result<OutputFormat, HttpError> {
    if let Some(raw) = format_field {
        return parse_output_format(raw, "format");
    }
    if let Some(format) = format_query {
        return Ok(format);
    }
    if let Some(accept) = accept.and_then(|v| v.to_str().ok()) {
        // Prefer JSON when explicitly accepted; otherwise default CSV.
        let wants_json = accept.split(',').any(|part| {
            let mime = part.split(';').next().unwrap_or("").trim();
            mime.eq_ignore_ascii_case("application/json")
        });
        if wants_json {
            return Ok(OutputFormat::Json);
        }
        let wants_csv = accept.split(',').any(|part| {
            let mime = part.split(';').next().unwrap_or("").trim();
            mime.eq_ignore_ascii_case("text/csv")
        });
        if wants_csv {
            return Ok(OutputFormat::Csv);
        }
    }
    Ok(OutputFormat::Csv)
}

/// Use only the final path component so uploads cannot escape the temp dir.
fn safe_basename(filename: &str) -> String {
    Path::new(filename)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("upload.ecl")
        .to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::http::config::HttpConfig;
    use crate::http::response::ResponseCodec;
    use crate::inference_options::{BatchSize, CudaTuning};
    use crate::license::{
        LicenseCheckResult, LicenseClient, LicenseError, LicenseFailureReason, LicenseGate,
        LicenseMeterResult, MockLicenseClient, MockOutcome, GLOBAL_TEST_LOCK,
    };
    use crate::model_source::ModelSource;
    use crate::phase2::ExecutionProviderKind;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use serde_json::Value;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    struct CountingMeterClient {
        inner: MockLicenseClient,
        meter_calls: Arc<AtomicUsize>,
        check_calls: Arc<AtomicUsize>,
    }

    impl CountingMeterClient {
        fn with_meter(meter: MockOutcome, meter_calls: Arc<AtomicUsize>) -> Self {
            Self {
                inner: MockLicenseClient::new(MockOutcome::Success { message: None }, meter),
                meter_calls,
                check_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl LicenseClient for CountingMeterClient {
        fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
            self.check_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.check_validity()
        }

        fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
            self.meter_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.authorize_and_meter()
        }
    }

    fn test_config(max_body_bytes: usize, model_path: Option<PathBuf>) -> HttpConfig {
        HttpConfig {
            bind: "127.0.0.1:0".into(),
            max_body_bytes,
            request_timeout: Duration::from_secs(1800),
            model_path,
            provider: ExecutionProviderKind::Cpu,
            batch_size: BatchSize::default(),
            cuda: CudaTuning::default(),
        }
    }

    fn app_with_state(state: AppState) -> Router {
        AnalyzeHandler::routes(state)
    }

    fn multipart_request(
        uri: &str,
        parts: &[(&str, Option<&str>, &[u8])],
        extra_headers: &[(&str, &str)],
    ) -> Request<Body> {
        let boundary = "----HolterAnalyzeBoundary9k3x";
        let mut body = Vec::new();
        for (name, filename, data) in parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match filename {
                Some(fname) => {
                    body.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n\
                             Content-Type: application/octet-stream\r\n\r\n"
                        )
                        .as_bytes(),
                    );
                }
                None => {
                    body.extend_from_slice(
                        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n")
                            .as_bytes(),
                    );
                }
            }
            body.extend_from_slice(data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let mut builder = Request::builder()
            .method("POST")
            .uri(uri)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header(header::CONTENT_LENGTH, body.len().to_string());
        for (k, v) in extra_headers {
            builder = builder.header(*k, *v);
        }
        builder.body(Body::from(body)).expect("request")
    }

    async fn oneshot(app: Router, req: Request<Body>) -> (StatusCode, Vec<u8>, HeaderMap) {
        let response = app.oneshot(req).await.expect("oneshot");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .expect("body")
            .to_vec();
        (status, bytes, headers)
    }

    fn install_counting_gate(meter: MockOutcome) -> Arc<AtomicUsize> {
        let meter_calls = Arc::new(AtomicUsize::new(0));
        let client = CountingMeterClient::with_meter(meter, Arc::clone(&meter_calls));
        LicenseGate::install(LicenseGate::new(client)).expect("install");
        meter_calls
    }

    /// Hold [`GLOBAL_TEST_LOCK`] for the whole HTTP exercise so parallel lib tests
    /// cannot clear the process-wide gate mid-request.
    fn with_gate<T>(meter: MockOutcome, f: impl FnOnce(Arc<AtomicUsize>) -> T) -> T {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        LicenseGate::clear_for_test();
        let meter_calls = install_counting_gate(meter);
        let out = f(Arc::clone(&meter_calls));
        LicenseGate::clear_for_test();
        out
    }

    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(fut)
    }

    #[test]
    fn analyze_handler_source_does_not_call_license_meter_directly() {
        let src = include_str!("analyze.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
        let code_lines = prod.lines().filter(|l| {
            let t = l.trim_start();
            !t.starts_with("//") && !t.is_empty()
        });
        let joined = code_lines.collect::<Vec<_>>().join("\n");
        assert!(
            !joined.contains("ensure_inference_allowed"),
            "HTTP analyze must not call the inference-gate API (canonical entry owns metering)"
        );
        assert!(
            !joined.contains("authorize_and_meter"),
            "HTTP analyze must not call authorize_and_meter"
        );
        assert!(
            joined.contains("analyze_ecl_with_source"),
            "handler must delegate to analyze_ecl_with_source"
        );
        assert!(
            joined.contains("spawn_blocking"),
            "sync analyze must run via spawn_blocking"
        );
    }

    fn startup_options() -> InferenceOptions {
        InferenceOptions {
            provider: ExecutionProviderKind::Cuda,
            batch_size: BatchSize::new(4).unwrap(),
            cuda: CudaTuning {
                tf32: Some(true),
                conv1d_pad_to_nc1d: Some(false),
                cuda_graph: Some(true),
            },
        }
    }

    #[test]
    fn ephemeral_options_keep_startup_batch_size_and_cuda_tuning() {
        let startup = startup_options();
        let opts = ephemeral_options(&startup, ExecutionProviderKind::Cpu);
        assert_eq!(opts.provider, ExecutionProviderKind::Cpu);
        assert_eq!(opts.batch_size, startup.batch_size);
        assert_eq!(opts.cuda, startup.cuda);
    }

    pub(crate) fn tiny_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phase2_tiny_dynamic.onnx")
    }

    /// 24 h ECL (minimum accepted size) with QRS-like pulses at 250 Hz;
    /// the filename declares an 11-minute recording.
    fn write_synthetic_ecl(dir: &Path) -> PathBuf {
        let n = crate::preprocess::EXPECTED_24H_SAMPLES_250;
        let mut bytes = Vec::with_capacity(n * 2);
        for i in 0..n {
            let phase = (i % 199) as f32 - 99.0;
            let value = 400.0 * (-(phase * phase) / 4.0).exp() + 20.0 * (i as f32 * 0.004).sin();
            let raw12 = (0x0800 + value.round() as i32).clamp(0, 0x0FFF) as u16;
            let word = ((raw12 & 0x0F00) << 4) | (raw12 & 0x00FF);
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let path = dir.join("1234567890_20250101_0000_0011.ecl");
        std::fs::write(&path, bytes).expect("write synthetic ECL");
        path
    }

    #[test]
    fn ephemeral_session_receives_startup_batch_size_and_cuda_tuning() {
        let dir = TempDir::new().expect("tempdir");
        let ecl = write_synthetic_ecl(dir.path());
        let out_csv = dir.path().join("beat_results.csv");
        let source = ModelSource::Path(tiny_fixture());
        let startup = startup_options();

        let (_rows, summary) = with_gate(MockOutcome::Success { message: None }, |meter_calls| {
            let result = run_analyze(
                &SharedModel::Ephemeral(source.clone()),
                &source,
                &startup,
                ExecutionProviderKind::Cpu,
                &ecl,
                &out_csv,
                Some(5),
            )
            .expect("ephemeral analyze");
            assert_eq!(meter_calls.load(Ordering::SeqCst), 1, "one meter per job");
            result
        });

        let eff = summary.perf.effective.as_ref().expect("effective settings");
        assert_eq!(eff.provider, ExecutionProviderKind::Cpu);
        assert_eq!(eff.requested_batch_size, 4);
        assert_eq!(eff.batch_size, 4);
        assert_eq!(eff.cuda, startup.cuda);
        assert!(!eff.cuda_applied, "CPU session must not apply CUDA tuning");
    }

    #[test]
    fn perf_log_line_prefixes_server_tag_and_appends_response_encode_ms() {
        let perf = AnalyzePerf {
            windows: 38,
            ..AnalyzePerf::default()
        };
        let line = perf_log_line(&perf, Duration::from_micros(2_345));
        assert_eq!(
            line,
            format!(
                "holter-http-api: {} response_encode_ms=2.3",
                perf.log_line()
            )
        );
        assert!(line.starts_with("holter-http-api: perf: "), "{line}");
        assert!(!line.contains('\n'), "{line}");
    }

    #[test]
    fn failure_log_line_reports_status_code_and_redacted_message() {
        let err = HttpError::from(LicenseError::InferenceDenied(
            crate::license::LicenseFailure::new(
                LicenseFailureReason::TemporaryFailure,
                "license server unreachable license_key=lk_secret",
            ),
        ));
        let line = failure_log_line(&err);
        assert!(
            line.starts_with(
                "holter-http-api: analyze failed status=503 code=license_temporarily_unavailable message=temporary_failure: license server unreachable"
            ),
            "{line}"
        );
        assert!(!line.contains("lk_secret"), "{line}");
    }

    #[test]
    fn missing_ecl_field_returns_400_without_metering() {
        with_gate(MockOutcome::Success { message: None }, |meter_calls| {
            block_on(async {
                let state = AppState::new(
                    test_config(
                        1024 * 1024,
                        Some(PathBuf::from("/tmp/missing-http-3-1.onnx")),
                    ),
                    ModelSource::Path(PathBuf::from("/tmp/missing-http-3-1.onnx")),
                );
                let app = app_with_state(state);
                let req = multipart_request("/v1/analyze", &[("format", None, b"csv")], &[]);
                let (status, bytes, _) = oneshot(app, req).await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
                let v: Value = serde_json::from_slice(&bytes).expect("json error");
                assert_eq!(v["error"]["code"], "invalid_input");
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    0,
                    "invalid input must not reach analyze / meter"
                );
            });
        });
    }

    #[test]
    fn invalid_ecl_filename_returns_400_without_metering() {
        with_gate(MockOutcome::Success { message: None }, |meter_calls| {
            block_on(async {
                let state = AppState::new(
                    test_config(
                        1024 * 1024,
                        Some(PathBuf::from("/tmp/missing-http-3-1.onnx")),
                    ),
                    ModelSource::Path(PathBuf::from("/tmp/missing-http-3-1.onnx")),
                );
                let app = app_with_state(state);
                let req = multipart_request(
                    "/v1/analyze",
                    &[("ecl", Some("not-an-ecl.txt"), b"not-ecl-bytes")],
                    &[],
                );
                let (status, bytes, _) = oneshot(app, req).await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
                let v: Value = serde_json::from_slice(&bytes).expect("json");
                assert_eq!(v["error"]["code"], "invalid_input");
                assert_eq!(meter_calls.load(Ordering::SeqCst), 0);
            });
        });
    }

    #[test]
    fn oversized_content_length_returns_413_before_analyze() {
        with_gate(MockOutcome::Success { message: None }, |meter_calls| {
            block_on(async {
                let max = 64usize;
                let state = AppState::new(
                    test_config(max, Some(PathBuf::from("/tmp/missing-http-3-1.onnx"))),
                    ModelSource::Path(PathBuf::from("/tmp/missing-http-3-1.onnx")),
                );
                let app = app_with_state(state);

                let boundary = "----HolterAnalyzeBoundary9k3x";
                let body = format!(
                    "--{boundary}\r\n\
                     Content-Disposition: form-data; name=\"ecl\"; filename=\"1234567890_20240101_0000_2359.ecl\"\r\n\
                     Content-Type: application/octet-stream\r\n\r\n\
                     xx\r\n\
                     --{boundary}--\r\n"
                );
                let req = Request::builder()
                    .method("POST")
                    .uri("/v1/analyze")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::CONTENT_LENGTH, (max + 1).to_string())
                    .body(Body::from(body))
                    .expect("req");

                let (status, bytes, _) = oneshot(app, req).await;
                assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
                let v: Value = serde_json::from_slice(&bytes).expect("json");
                assert_eq!(v["error"]["code"], "payload_too_large");
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    0,
                    "oversized body must not start analyze / meter"
                );
            });
        });
    }

    const SYNTH_BODY_LIMIT: usize = 64 * 1024 * 1024;

    fn tiny_state() -> AppState {
        AppState::new(
            test_config(SYNTH_BODY_LIMIT, Some(tiny_fixture())),
            ModelSource::Path(tiny_fixture()),
        )
    }

    /// Filename and bytes of the synthetic 38-window ECL (about 43 MB).
    pub(crate) fn synthetic_ecl_upload() -> (String, Vec<u8>) {
        let dir = TempDir::new().expect("tempdir");
        let path = write_synthetic_ecl(dir.path());
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .expect("ecl basename")
            .to_string();
        (name, std::fs::read(&path).expect("read synthetic ECL"))
    }

    #[test]
    fn license_deny_returns_403_and_meters_once() {
        let (ecl_name, ecl_bytes) = synthetic_ecl_upload();
        with_gate(
            MockOutcome::Reject {
                reason: LicenseFailureReason::MonthlyLimitReached,
                message: Some("quota exceeded".into()),
            },
            |meter_calls| {
                block_on(async {
                    let app = app_with_state(tiny_state());
                    let req = multipart_request(
                        "/v1/analyze",
                        &[
                            ("ecl", Some(ecl_name.as_str()), &ecl_bytes),
                            ("max_windows", None, b"2"),
                        ],
                        &[],
                    );
                    let (status, bytes, _) = oneshot(app, req).await;
                    assert_eq!(status, StatusCode::FORBIDDEN);
                    let v: Value = serde_json::from_slice(&bytes).expect("json");
                    assert_eq!(v["error"]["code"], "license_inference_denied");
                    assert!(
                        v.get("rows").is_none() && v.get("summary").is_none(),
                        "denied response must not leak analyze results"
                    );
                    assert_eq!(
                        meter_calls.load(Ordering::SeqCst),
                        1,
                        "canonical entry meters once; HTTP must not double-meter"
                    );
                });
            },
        );
    }

    #[test]
    fn analyze_meters_once_via_canonical_entry() {
        let (ecl_name, ecl_bytes) = synthetic_ecl_upload();
        with_gate(MockOutcome::Success { message: None }, |meter_calls| {
            block_on(async {
                let app = app_with_state(tiny_state());
                let req = multipart_request(
                    "/v1/analyze",
                    &[
                        ("ecl", Some(ecl_name.as_str()), &ecl_bytes),
                        ("format", None, b"json"),
                        ("max_windows", None, b"3"),
                    ],
                    &[],
                );
                let (status, bytes, _) = oneshot(app, req).await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "body: {}",
                    String::from_utf8_lossy(&bytes)
                );
                let v: Value = serde_json::from_slice(&bytes).expect("json");
                assert_eq!(v["summary"]["windows"], 3);
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    1,
                    "multiple windows in one request meter once"
                );
            });
        });
    }

    fn content_type_of(headers: &HeaderMap) -> String {
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    /// One synthetic analyze (2 windows) against the tiny model; returns status, body, Content-Type and meter calls.
    fn analyze_synthetic(
        uri: &str,
        format_field: Option<&[u8]>,
        extra_headers: &[(&str, &str)],
    ) -> (StatusCode, Vec<u8>, String, usize) {
        let (ecl_name, ecl_bytes) = synthetic_ecl_upload();
        with_gate(MockOutcome::Success { message: None }, |meter_calls| {
            block_on(async {
                let mut parts: Vec<(&str, Option<&str>, &[u8])> = vec![
                    ("ecl", Some(ecl_name.as_str()), &ecl_bytes),
                    ("max_windows", None, b"2"),
                ];
                if let Some(format) = format_field {
                    parts.push(("format", None, format));
                }
                let req = multipart_request(uri, &parts, extra_headers);
                let (status, bytes, headers) = oneshot(app_with_state(tiny_state()), req).await;
                (
                    status,
                    bytes,
                    content_type_of(&headers),
                    meter_calls.load(Ordering::SeqCst),
                )
            })
        })
    }

    #[test]
    fn format_query_json_returns_json() {
        let (status, bytes, ct, meters) =
            analyze_synthetic("/v1/analyze?format=JSON&unrelated=1", None, &[]);
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(ct.contains("application/json"), "got {ct}");
        let v: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(v["summary"]["windows"], 2);
        assert_eq!(meters, 1);
    }

    #[test]
    fn format_query_csv_overrides_accept_json() {
        let (status, bytes, ct, _) = analyze_synthetic(
            "/v1/analyze?format=csv",
            None,
            &[(header::ACCEPT.as_str(), "application/json")],
        );
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(ct.contains("text/csv"), "query must beat Accept; got {ct}");
        let csv = String::from_utf8(bytes).expect("utf8");
        assert!(csv.starts_with("record_id,"), "CSV result: {csv}");
    }

    #[test]
    fn format_field_overrides_format_query() {
        let (status, bytes, ct, _) = analyze_synthetic(
            "/v1/analyze?format=csv",
            Some(b"json"),
            &[(header::ACCEPT.as_str(), "text/csv")],
        );
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(
            ct.contains("application/json"),
            "field must beat query; got {ct}"
        );
        let v: Value = serde_json::from_slice(&bytes).expect("json");
        assert!(v["rows"].is_array());
    }

    #[test]
    fn invalid_format_query_returns_400_without_metering() {
        let (status, bytes, _, meters) = analyze_synthetic("/v1/analyze?format=xml", None, &[]);
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let v: Value = serde_json::from_slice(&bytes).expect("json error");
        assert_eq!(v["error"]["code"], "invalid_input");
        let message = v["error"]["message"].as_str().unwrap_or("");
        assert!(
            message.contains("xml") && message.contains("csv|json"),
            "message: {message}"
        );
        assert_eq!(meters, 0, "invalid query must not reach analyze / meter");
    }

    #[test]
    fn invalid_format_query_is_rejected_even_with_valid_format_field() {
        let (status, bytes, _, meters) =
            analyze_synthetic("/v1/analyze?format=xml", Some(b"json"), &[]);
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let v: Value = serde_json::from_slice(&bytes).expect("json error");
        assert_eq!(v["error"]["code"], "invalid_input");
        assert_eq!(meters, 0, "invalid query must not reach analyze / meter");
    }

    #[test]
    fn malformed_query_returns_json_invalid_input_without_metering() {
        with_gate(MockOutcome::Success { message: None }, |meter_calls| {
            block_on(async {
                let req = multipart_request(
                    "/v1/analyze?format=json&format=csv",
                    &[(
                        "ecl",
                        Some("1234567890_20240101_0000_2359.ecl"),
                        b"placeholder",
                    )],
                    &[],
                );
                let (status, bytes, _) = oneshot(app_with_state(tiny_state()), req).await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
                let v: Value = serde_json::from_slice(&bytes).expect("json error envelope");
                assert_eq!(v["error"]["code"], "invalid_input");
                assert_eq!(meter_calls.load(Ordering::SeqCst), 0);
            });
        });
    }

    #[test]
    fn short_ecl_or_missing_model_fails_without_metering() {
        let (ecl_name, ecl_bytes) = synthetic_ecl_upload();
        with_gate(MockOutcome::Success { message: None }, |meter_calls| {
            block_on(async {
                let app = app_with_state(tiny_state());
                let req = multipart_request(
                    "/v1/analyze",
                    &[(
                        "ecl",
                        Some("1234567890_20240101_0000_2359.ecl"),
                        b"placeholder",
                    )],
                    &[],
                );
                let (status, bytes, _) = oneshot(app, req).await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
                let v: Value = serde_json::from_slice(&bytes).expect("json");
                assert_eq!(v["error"]["code"], "invalid_input");

                let missing = PathBuf::from("/tmp/missing-http-10-1.onnx");
                let state = AppState::new(
                    test_config(SYNTH_BODY_LIMIT, Some(missing.clone())),
                    ModelSource::Path(missing),
                );
                let req = multipart_request(
                    "/v1/analyze",
                    &[
                        ("ecl", Some(ecl_name.as_str()), &ecl_bytes),
                        ("max_windows", None, b"2"),
                    ],
                    &[],
                );
                let (status, _, _) = oneshot(app_with_state(state), req).await;
                assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    0,
                    "input or model failures before inference must not consume usage"
                );
            });
        });
    }

    #[test]
    fn format_json_field_still_returns_json_error_envelope_on_deny() {
        let (ecl_name, ecl_bytes) = synthetic_ecl_upload();
        with_gate(
            MockOutcome::Reject {
                reason: LicenseFailureReason::LicenseInvalid,
                message: Some("deny".into()),
            },
            |meter_calls| {
                block_on(async {
                    let app = app_with_state(tiny_state());
                    let req = multipart_request(
                        "/v1/analyze",
                        &[
                            ("ecl", Some(ecl_name.as_str()), &ecl_bytes),
                            ("format", None, b"json"),
                            ("max_windows", None, b"2"),
                        ],
                        &[],
                    );
                    let (status, bytes, headers) = oneshot(app, req).await;
                    assert_eq!(status, StatusCode::FORBIDDEN);
                    let ct = headers
                        .get(header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    assert!(
                        ct.contains("application/json"),
                        "error envelope is JSON: {ct}"
                    );
                    let v: Value = serde_json::from_slice(&bytes).expect("json");
                    assert_eq!(v["error"]["code"], "license_inference_denied");
                    assert!(v.get("rows").is_none() && v.get("summary").is_none());
                    assert_eq!(meter_calls.load(Ordering::SeqCst), 1);
                });
            },
        );
    }

    #[test]
    fn valid_ecl_returns_csv_and_json_success_when_sample_present() {
        let sample_link = Path::new("resources/samples/sample.ecl");
        let onnx = Path::new("resources/models/phase2_rev1.onnx");
        if !sample_link.exists() || !onnx.exists() {
            eprintln!("skip: sample.ecl or ONNX not present");
            return;
        }
        let sample = match sample_link.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("skip: canonicalize sample.ecl: {e}");
                return;
            }
        };
        let ecl_bytes = std::fs::read(&sample).expect("read ecl");
        let ecl_name = sample
            .file_name()
            .and_then(|s| s.to_str())
            .expect("ecl basename")
            .to_string();

        with_gate(MockOutcome::Success { message: None }, |meter_calls| {
            block_on(async {
                let state = AppState::new(
                    test_config(512 * 1024 * 1024, Some(onnx.to_path_buf())),
                    ModelSource::Path(onnx.to_path_buf()),
                );

                // CSV (default)
                {
                    let app = app_with_state(state.clone());
                    let req = multipart_request(
                        "/v1/analyze",
                        &[
                            ("ecl", Some(ecl_name.as_str()), &ecl_bytes),
                            ("max_windows", None, b"1"),
                            ("provider", None, b"cpu"),
                        ],
                        &[],
                    );
                    let (status, bytes, headers) = oneshot(app, req).await;
                    assert_eq!(
                        status,
                        StatusCode::OK,
                        "csv body: {}",
                        String::from_utf8_lossy(&bytes)
                    );
                    let ct = headers
                        .get(header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    assert!(ct.contains("text/csv"), "expected text/csv, got {ct}");
                    let csv = String::from_utf8(bytes).expect("utf8");
                    for col in ResponseCodec::to_csv(&[])
                        .expect("hdr")
                        .lines()
                        .next()
                        .unwrap()
                        .split(',')
                    {
                        assert!(csv.contains(col), "CSV missing CLI column {col}");
                    }
                }

                // JSON via format field
                {
                    let app = app_with_state(state.clone());
                    let req = multipart_request(
                        "/v1/analyze",
                        &[
                            ("ecl", Some(ecl_name.as_str()), &ecl_bytes),
                            ("format", None, b"json"),
                            ("max_windows", None, b"1"),
                        ],
                        &[],
                    );
                    let (status, bytes, headers) = oneshot(app, req).await;
                    assert_eq!(status, StatusCode::OK);
                    let ct = headers
                        .get(header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    assert!(ct.contains("application/json"), "got {ct}");
                    let v: Value = serde_json::from_slice(&bytes).expect("json");
                    assert!(v["summary"]["windows"].as_u64().unwrap_or(0) >= 1);
                    assert!(v["rows"].is_array());
                }

                // JSON via Accept when format omitted
                {
                    let app = app_with_state(state);
                    let req = multipart_request(
                        "/v1/analyze",
                        &[
                            ("ecl", Some(ecl_name.as_str()), &ecl_bytes),
                            ("max_windows", None, b"1"),
                        ],
                        &[(header::ACCEPT.as_str(), "application/json")],
                    );
                    let (status, bytes, headers) = oneshot(app, req).await;
                    assert_eq!(status, StatusCode::OK);
                    let ct = headers
                        .get(header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    assert!(ct.contains("application/json"), "Accept negotiation: {ct}");
                    let v: Value = serde_json::from_slice(&bytes).expect("json");
                    assert!(v["rows"].is_array());
                }

                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    3,
                    "three successful analyze requests → three meters"
                );
            });
        });
    }
}
