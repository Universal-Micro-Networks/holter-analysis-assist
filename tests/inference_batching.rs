//! Batched Phase-2 inference on the tiny CPU fixtures (inference-acceleration task 2.3,
//! requirements 2.3, 2.4, 2.5).

use holter_analysis_assist::inference_options::{BatchSize, CudaTuning};
use holter_analysis_assist::phase2::{
    ExecutionProviderKind, InferError, InferenceOptions, ModelBatchShape, Phase2Model,
    WindowOutputs, WINDOW_SAMPLES,
};
use std::path::PathBuf;
use std::time::Duration;

const DYNAMIC: &str = "phase2_tiny_dynamic.onnx";
const FIXED1: &str = "phase2_tiny_fixed1.onnx";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn load_cpu(name: &str, batch: usize) -> Phase2Model {
    let options = InferenceOptions {
        provider: ExecutionProviderKind::Cpu,
        batch_size: BatchSize::new(batch).expect("valid batch size"),
        cuda: CudaTuning::default(),
    };
    Phase2Model::load_with_options(fixture(name), &options)
        .unwrap_or_else(|e| panic!("load {name}: {e}"))
}

/// Distinct deterministic window per index.
fn window(k: usize) -> Vec<f32> {
    let scale = 0.2 + 0.07 * k as f32;
    let phase = k as f32 * 0.37;
    (0..WINDOW_SAMPLES)
        .map(|i| scale * ((i as f32 * 0.013 + phase).sin() + 0.3 * (i as f32 / 997.0 - 5.0)))
        .collect()
}

fn flat_windows(n: usize) -> Vec<f32> {
    (0..n).flat_map(window).collect()
}

fn assert_same(ctx: &str, got: &WindowOutputs, want: &WindowOutputs) {
    assert_eq!(got.beat, want.beat, "{ctx}: beat");
    assert_eq!(got.event, want.event, "{ctx}: event");
    assert_eq!(got.rhythm, want.rhythm, "{ctx}: rhythm");
}

fn per_window_reference(n: usize) -> Vec<WindowOutputs> {
    let mut model = load_cpu(DYNAMIC, 1);
    (0..n)
        .map(|k| model.infer_window(&window(k)).expect("infer_window"))
        .collect()
}

#[test]
fn batch_of_16_matches_single_window_outputs() {
    let mut model = load_cpu(DYNAMIC, 16);
    assert_eq!(model.batch_size(), 16);
    let outputs = model
        .infer_batch(&flat_windows(16), 16)
        .expect("infer_batch 16");
    assert_eq!(outputs.len(), 16);

    let reference = per_window_reference(16);
    for (k, (got, want)) in outputs.iter().zip(&reference).enumerate() {
        assert_eq!(got.beat.len(), WINDOW_SAMPLES, "window {k}: beat length");
        assert_eq!(got.event.len(), WINDOW_SAMPLES, "window {k}: event length");
        assert_same(&format!("window {k}"), got, want);
    }

    // The batched model's own single-window path agrees too.
    let single = model.infer_window(&window(3)).expect("infer_window");
    assert_same("infer_window on batch-16 model", &single, &reference[3]);
}

#[test]
fn thirty_seven_windows_in_chunks_of_16_return_all_in_input_order() {
    let total = 37;
    let mut model = load_cpu(DYNAMIC, 16);
    let batch = model.batch_size();
    let flat = flat_windows(total);

    let mut outputs = Vec::with_capacity(total);
    let mut chunk_counts = Vec::new();
    for chunk in flat.chunks(batch * WINDOW_SAMPLES) {
        let count = chunk.len() / WINDOW_SAMPLES;
        chunk_counts.push(count);
        let got = model.infer_batch(chunk, count).expect("infer_batch chunk");
        assert_eq!(got.len(), count, "chunk returns exactly its windows");
        outputs.extend(got);
    }
    assert_eq!(chunk_counts, vec![16, 16, 5]);
    assert_eq!(outputs.len(), total);

    let reference = per_window_reference(total);
    for (k, (got, want)) in outputs.iter().zip(&reference).enumerate() {
        assert_same(&format!("window {k}"), got, want);
    }
}

#[test]
fn batch_size_one_and_default_batch_give_identical_results() {
    let total = 20;
    let flat = flat_windows(total);
    let run = |batch: usize| -> Vec<WindowOutputs> {
        let mut model = load_cpu(DYNAMIC, batch);
        let mut out = Vec::new();
        for chunk in flat.chunks(model.batch_size() * WINDOW_SAMPLES) {
            out.extend(
                model
                    .infer_batch(chunk, chunk.len() / WINDOW_SAMPLES)
                    .expect("infer_batch"),
            );
        }
        out
    };
    let one = run(1);
    let default = run(BatchSize::default().get());
    assert_eq!(one.len(), total);
    assert_eq!(default.len(), total);
    for (k, (a, b)) in one.iter().zip(&default).enumerate() {
        assert_same(&format!("window {k}"), a, b);
    }
}

#[test]
fn invalid_count_is_rejected_before_inference() {
    let mut model = load_cpu(DYNAMIC, 16);
    for count in [0, 17] {
        let windows = vec![0.0_f32; count * WINDOW_SAMPLES];
        let err = model
            .infer_batch(&windows, count)
            .expect_err("count outside 1..=batch_size must fail");
        assert!(
            matches!(err, InferError::InvalidBatch { count: c, max: 16 } if c == count),
            "count={count}: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains(&count.to_string()) && msg.contains("16"),
            "{msg}"
        );
    }
}

#[test]
fn length_mismatch_is_rejected_before_inference() {
    let mut model = load_cpu(DYNAMIC, 16);
    let windows = vec![0.0_f32; 3 * WINDOW_SAMPLES - 1];
    let err = model
        .infer_batch(&windows, 3)
        .expect_err("length must equal count * WINDOW_SAMPLES");
    assert!(
        matches!(
            err,
            InferError::InvalidWindow { expected, got }
                if expected == 3 * WINDOW_SAMPLES && got == 3 * WINDOW_SAMPLES - 1
        ),
        "{err}"
    );

    let err = model.infer_window(&[0.0; 10]).expect_err("short window");
    assert!(
        matches!(
            err,
            InferError::InvalidWindow { expected, got: 10 } if expected == WINDOW_SAMPLES
        ),
        "{err}"
    );
}

#[test]
fn fixed1_model_runs_one_window_per_call_and_rejects_more() {
    let mut model = load_cpu(FIXED1, 16);
    assert_eq!(model.effective().model_batch, ModelBatchShape::Fixed(1));
    assert_eq!(model.batch_size(), 1);
    assert!(
        model
            .effective()
            .notes
            .iter()
            .any(|n| n.contains("fixed batch 1")),
        "{:?}",
        model.effective().notes
    );

    let reference = per_window_reference(3);
    for (k, want) in reference.iter().enumerate() {
        let got = model
            .infer_batch(&window(k), 1)
            .expect("fixed1 infer_batch");
        assert_eq!(got.len(), 1);
        assert_same(&format!("fixed1 window {k}"), &got[0], want);
    }

    let err = model
        .infer_batch(&flat_windows(2), 2)
        .expect_err("fixed batch 1 cannot take 2 windows");
    assert!(
        matches!(err, InferError::InvalidBatch { count: 2, max: 1 }),
        "{err}"
    );
}

#[test]
fn warm_up_runs_effective_batch_and_returns_elapsed_time() {
    for (name, batch) in [(DYNAMIC, 16), (DYNAMIC, 1), (FIXED1, 16)] {
        let mut model = load_cpu(name, batch);
        let elapsed = model
            .warm_up()
            .unwrap_or_else(|e| panic!("warm_up {name} batch={batch}: {e}"));
        assert!(elapsed > Duration::ZERO, "{name}: elapsed {elapsed:?}");

        let out = model.infer_window(&window(0)).expect("infer after warm_up");
        assert_eq!(out.beat.len(), WINDOW_SAMPLES);
    }
}
