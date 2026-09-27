//! Effective batch size and tail padding derived from the model's input
//! batch dimension, the requested batch size, and whether execution needs a
//! fixed input shape (CUDA Graph).

use super::ModelBatchShape;
use crate::inference_options::{BatchSize, KEY_BATCH_SIZE};

/// Batch settings actually used for inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchPlan {
    /// Windows per inference call.
    pub size: usize,
    /// Fill a short final batch up to `size` with padding windows.
    pub pad_tail: bool,
    /// Warning when the requested batch size cannot be honored.
    pub note: Option<String>,
}

/// A dynamic model runs the requested size and pads only when a fixed shape
/// is required; a fixed-batch model always runs its own size, pads when that
/// size exceeds 1, and warns when the request differs.
pub fn plan_batch(
    model: ModelBatchShape,
    requested: BatchSize,
    fixed_shape_required: bool,
) -> BatchPlan {
    let requested = requested.get();
    match model {
        ModelBatchShape::Dynamic => BatchPlan {
            size: requested,
            pad_tail: fixed_shape_required,
            note: None,
        },
        ModelBatchShape::Fixed(n) => BatchPlan {
            size: n,
            pad_tail: n > 1,
            note: (requested != n).then(|| {
                format!("model has fixed batch {n}; {KEY_BATCH_SIZE}={requested} is ignored")
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(n: usize) -> BatchSize {
        BatchSize::new(n).unwrap()
    }

    #[test]
    fn matrix_matches_rules() {
        use ModelBatchShape::{Dynamic, Fixed};
        // (model, requested, fixed_shape_required) -> (size, pad_tail, note present)
        let cases = [
            ((Dynamic, 1, false), (1, false, false)),
            ((Dynamic, 1, true), (1, true, false)),
            ((Dynamic, 16, false), (16, false, false)),
            ((Dynamic, 16, true), (16, true, false)),
            ((Fixed(1), 1, false), (1, false, false)),
            ((Fixed(1), 1, true), (1, false, false)),
            ((Fixed(1), 16, false), (1, false, true)),
            ((Fixed(1), 16, true), (1, false, true)),
            ((Fixed(8), 1, false), (8, true, true)),
            ((Fixed(8), 1, true), (8, true, true)),
            ((Fixed(8), 16, false), (8, true, true)),
            ((Fixed(8), 16, true), (8, true, true)),
        ];
        for ((model, requested, fixed), (size, pad_tail, has_note)) in cases {
            let plan = plan_batch(model, batch(requested), fixed);
            let ctx = format!("{model:?} requested={requested} fixed_shape_required={fixed}");
            assert_eq!(plan.size, size, "size for {ctx}");
            assert_eq!(plan.pad_tail, pad_tail, "pad_tail for {ctx}");
            assert_eq!(
                plan.note.is_some(),
                has_note,
                "note for {ctx}: {:?}",
                plan.note
            );
        }
    }

    #[test]
    fn fixed_batch_note_names_model_batch_and_ignored_request() {
        let plan = plan_batch(ModelBatchShape::Fixed(1), batch(16), false);
        let note = plan
            .note
            .expect("note when requested differs from fixed batch");
        assert!(note.contains("fixed batch 1"), "{note}");
        assert!(note.contains("batch_size=16"), "{note}");
        assert!(note.contains("ignored"), "{note}");
    }

    #[test]
    fn dynamic_uses_default_request() {
        let plan = plan_batch(ModelBatchShape::Dynamic, BatchSize::default(), false);
        assert_eq!(
            plan,
            BatchPlan {
                size: 16,
                pad_tail: false,
                note: None
            }
        );
    }
}
