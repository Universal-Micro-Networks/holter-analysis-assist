#!/usr/bin/env python3
"""Generate the tiny synthetic Phase-2 ONNX test fixtures (batch-inference tests).

SYNTHETIC MODELS — NO TRAINED WEIGHTS. The graphs only apply a few hand-picked
affine + Sigmoid ops so that tests get deterministic, input-dependent outputs
with the same I/O contract as ``resources/models/phase2_rev1.onnx.json``:

- input  ``ecg``    : (B, 10000, 1) float32 (NTC)
- output ``beat``   : (B, 10000, 1) = Sigmoid(ecg * 1.5 + 0.25)
- output ``event``  : (B, 10000, 3) = Sigmoid(ecg * [0.8, -1.2, 2.0] + [-0.3, 0.1, 0.4])
- output ``rhythm`` : (B, 1)        = Sigmoid(ReduceMean(ecg, axis=1) * 3.0 - 0.5)

Every output is computed independently per batch element, so a batched run must
equal the per-window runs element by element.

Writes (default ``tests/fixtures/``):
- ``phase2_tiny_dynamic.onnx`` — batch dimension symbolic (``batch``)
- ``phase2_tiny_fixed1.onnx``  — batch dimension fixed to 1

Regeneration requires Python with ``onnx`` and ``numpy`` (not needed at test time)::

    python3 -m venv .venv-onnx && .venv-onnx/bin/pip install onnx numpy
    .venv-onnx/bin/python tools/export/make_tiny_batch_onnx.py
"""
from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np

try:
    import onnx
    from onnx import TensorProto, helper, numpy_helper
except ImportError as e:
    raise SystemExit("pip install onnx numpy first") from e

REPO = Path(__file__).resolve().parents[2]
DEFAULT_OUT_DIR = REPO / "tests" / "fixtures"
WINDOW = 10_000
EVENT_CHANNELS = 3
OPSET = 17
IR_VERSION = 8

BEAT_SCALE, BEAT_BIAS = 1.5, 0.25
EVENT_SCALE = (0.8, -1.2, 2.0)
EVENT_BIAS = (-0.3, 0.1, 0.4)
RHYTHM_SCALE, RHYTHM_BIAS = 3.0, -0.5


def _const(name: str, value) -> onnx.TensorProto:
    return numpy_helper.from_array(np.asarray(value, dtype=np.float32), name=name)


def build_model(batch: int | str) -> onnx.ModelProto:
    """Build the fixture graph; ``batch`` is an int (fixed) or a symbolic dim name."""
    nodes = [
        helper.make_node("Mul", ["ecg", "beat_scale"], ["beat_lin"]),
        helper.make_node("Add", ["beat_lin", "beat_bias"], ["beat_pre"]),
        helper.make_node("Sigmoid", ["beat_pre"], ["beat"]),
        # (B, T, 1) * (3,) broadcasts to (B, T, 3): one affine per event channel.
        helper.make_node("Mul", ["ecg", "event_scale"], ["event_lin"]),
        helper.make_node("Add", ["event_lin", "event_bias"], ["event_pre"]),
        helper.make_node("Sigmoid", ["event_pre"], ["event"]),
        # Opset 17 ReduceMean takes axes as an attribute: (B, T, 1) -> (B, 1).
        helper.make_node("ReduceMean", ["ecg"], ["ecg_mean"], axes=[1], keepdims=0),
        helper.make_node("Mul", ["ecg_mean", "rhythm_scale"], ["rhythm_lin"]),
        helper.make_node("Add", ["rhythm_lin", "rhythm_bias"], ["rhythm_pre"]),
        helper.make_node("Sigmoid", ["rhythm_pre"], ["rhythm"]),
    ]
    initializers = [
        _const("beat_scale", BEAT_SCALE),
        _const("beat_bias", BEAT_BIAS),
        _const("event_scale", EVENT_SCALE),
        _const("event_bias", EVENT_BIAS),
        _const("rhythm_scale", RHYTHM_SCALE),
        _const("rhythm_bias", RHYTHM_BIAS),
    ]
    f32 = TensorProto.FLOAT
    graph = helper.make_graph(
        nodes=nodes,
        name="phase2_tiny",
        inputs=[helper.make_tensor_value_info("ecg", f32, [batch, WINDOW, 1])],
        outputs=[
            helper.make_tensor_value_info("beat", f32, [batch, WINDOW, 1]),
            helper.make_tensor_value_info("event", f32, [batch, WINDOW, EVENT_CHANNELS]),
            helper.make_tensor_value_info("rhythm", f32, [batch, 1]),
        ],
        initializer=initializers,
    )
    model = helper.make_model(
        graph,
        producer_name="make_tiny_batch_onnx.py",
        doc_string="Synthetic Phase-2 test fixture; no trained weights.",
        opset_imports=[helper.make_opsetid("", OPSET)],
    )
    model.ir_version = IR_VERSION
    onnx.checker.check_model(model, full_check=True)
    return model


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    args = parser.parse_args()

    args.out_dir.mkdir(parents=True, exist_ok=True)
    for file_name, batch in (
        ("phase2_tiny_dynamic.onnx", "batch"),
        ("phase2_tiny_fixed1.onnx", 1),
    ):
        out = args.out_dir / file_name
        onnx.save(build_model(batch), out)
        print(f"wrote {out} ({out.stat().st_size} bytes, batch={batch})")


if __name__ == "__main__":
    main()
