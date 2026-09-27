//! IoBinding execution for CUDA Graph. A captured graph replays against the
//! device addresses seen at capture, so the input and the three outputs stay
//! bound to the same device tensors and every run has shape `[batch, 10000, 1]`.

use super::{
    extract_flat, InferError, RawBatchOutputs, INPUT_CHANNELS, INPUT_NAME, WINDOW_SAMPLES,
};
use ort::memory::{AllocationDevice, Allocator, AllocatorType, MemoryInfo, MemoryType};
use ort::session::{IoBinding, Session};
use ort::value::{Shape, Tensor};
use std::fmt::Display;

const OUTPUT_NAMES: [&str; 3] = ["beat", "event", "rhythm"];

pub(crate) struct CudaGraphRunner {
    binding: IoBinding,
    input: Tensor<f32>,
    batch: usize,
    _allocator: Allocator,
}

impl CudaGraphRunner {
    pub(crate) fn new(session: &Session, batch: usize) -> Result<Self, InferError> {
        let allocator = MemoryInfo::new(
            AllocationDevice::CUDA,
            0,
            AllocatorType::Device,
            MemoryType::Default,
        )
        .and_then(|info| Allocator::new(session, info))
        .map_err(graph_error("create CUDA allocator"))?;
        let input = Tensor::<f32>::new(&allocator, [batch, WINDOW_SAMPLES, INPUT_CHANNELS])
            .map_err(graph_error("allocate device input"))?;

        let mut binding = session
            .create_binding()
            .map_err(graph_error("create binding"))?;
        binding
            .bind_input(INPUT_NAME, &input)
            .map_err(graph_error("bind input"))?;
        for name in OUTPUT_NAMES {
            let output = Tensor::<f32>::new(&allocator, output_shape(session, name, batch)?)
                .map_err(graph_error("allocate device output"))?;
            binding
                .bind_output(name, output)
                .map_err(graph_error("bind output"))?;
        }

        Ok(Self {
            binding,
            input,
            batch,
            _allocator: allocator,
        })
    }

    /// `flat.len() == batch * WINDOW_SAMPLES` (already padded). Copies into the
    /// bound device input in place, runs the binding (capture on the first
    /// graph-eligible run, replay afterwards) and copies the outputs to host.
    pub(crate) fn run(
        &mut self,
        session: &mut Session,
        flat: &[f32],
    ) -> Result<RawBatchOutputs, InferError> {
        let batch = self.batch;
        debug_assert_eq!(flat.len(), batch * WINDOW_SAMPLES);
        let host = Tensor::from_array((
            [batch, WINDOW_SAMPLES, INPUT_CHANNELS],
            flat.to_vec().into_boxed_slice(),
        ))?;
        host.copy_into(&mut self.input)
            .map_err(graph_error("copy input to device"))?;

        let outputs = session
            .run_binding(&self.binding)
            .map_err(graph_error("run"))?;
        let fetch = |name: &'static str, len: usize| -> Result<Vec<f32>, InferError> {
            let device = outputs.get(name).ok_or(InferError::MissingOutput(name))?;
            let host = device
                .to(AllocationDevice::CPU, 0)
                .map_err(graph_error("copy output to host"))?;
            extract_flat(&host, name, len)
        };
        Ok(RawBatchOutputs {
            beat: fetch("beat", batch * WINDOW_SAMPLES)?,
            event: fetch("event", batch * WINDOW_SAMPLES * 3)?,
            rhythm: fetch("rhythm", batch)?,
        })
    }
}

/// Declared output shape with the batch dimension set to `batch`; other
/// dimensions must be static to pre-allocate the bound buffer.
fn output_shape(session: &Session, name: &'static str, batch: usize) -> Result<Shape, InferError> {
    let output = session
        .outputs()
        .iter()
        .find(|o| o.name() == name)
        .ok_or(InferError::MissingOutput(name))?;
    let dims = output
        .dtype()
        .tensor_shape()
        .ok_or_else(|| InferError::CudaGraph(format!("output {name} is not a tensor")))?;
    dims.iter()
        .enumerate()
        .map(|(i, &d)| match (i, d) {
            (0, _) => Ok(batch as i64),
            (_, d) if d > 0 => Ok(d),
            _ => Err(InferError::CudaGraph(format!(
                "output {name} has a dynamic non-batch dimension ({dims:?})"
            ))),
        })
        .collect::<Result<Vec<i64>, _>>()
        .map(Shape::from)
}

fn graph_error<E: Display>(stage: &'static str) -> impl Fn(E) -> InferError {
    move |err| InferError::CudaGraph(format!("{stage}: {err}"))
}
