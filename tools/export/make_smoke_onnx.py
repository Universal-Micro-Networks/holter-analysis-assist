#!/usr/bin/env python3
"""Create a tiny Phase-2-shaped ONNX for EP/CUDA smoke benches (not for accuracy).

Outputs identity-like placeholders matching phase2_rev1.onnx.json I/O names/shapes.
"""
from __future__ import annotations

from pathlib import Path

import numpy as np

try:
    import onnx
    from onnx import TensorProto, helper, numpy_helper
except ImportError as e:
    raise SystemExit("pip install onnx first") from e

REPO = Path(__file__).resolve().parents[2]
OUT = REPO / "resources" / "models" / "phase2_smoke.onnx"
WINDOW = 10_000


def main() -> None:
    # Inputs
    ecg = helper.make_tensor_value_info("ecg", TensorProto.FLOAT, [1, WINDOW, 1])

    # Constants for deterministic outputs
    beat_init = numpy_helper.from_array(
        np.full((1, WINDOW), 0.1, dtype=np.float32), name="beat_const"
    )
    event_init = numpy_helper.from_array(
        np.tile(np.array([0.1, 0.1, 0.8], dtype=np.float32), (1, WINDOW, 1)),
        name="event_const",
    )
    rhythm_init = numpy_helper.from_array(
        np.array([0.2], dtype=np.float32), name="rhythm_const"
    )

    # Use Identity so the graph still depends on input (keeps EP path non-empty).
    # Scale input mean into a tiny additive bias so CUDA must touch the tensor.
    reduce = helper.make_node(
        "ReduceMean",
        inputs=["ecg"],
        outputs=["ecg_mean"],
        axes=[1, 2],
        keepdims=0,
    )
    # beat = beat_const + 0 * ecg_mean (broadcast via Mul+Add)
    zero = numpy_helper.from_array(np.array(0.0, dtype=np.float32), name="zero")
    mul = helper.make_node("Mul", inputs=["ecg_mean", "zero"], outputs=["bias"])
    # Expand bias to beat shape via Add with const (ORT broadcasts scalar)
    add_beat = helper.make_node("Add", inputs=["beat_const", "bias"], outputs=["beat"])
    add_event = helper.make_node("Add", inputs=["event_const", "bias"], outputs=["event"])
    add_rhythm = helper.make_node(
        "Add", inputs=["rhythm_const", "bias"], outputs=["rhythm"]
    )

    graph = helper.make_graph(
        nodes=[reduce, mul, add_beat, add_event, add_rhythm],
        name="phase2_smoke",
        inputs=[ecg],
        outputs=[
            helper.make_tensor_value_info("beat", TensorProto.FLOAT, [1, WINDOW]),
            helper.make_tensor_value_info("event", TensorProto.FLOAT, [1, WINDOW, 3]),
            helper.make_tensor_value_info("rhythm", TensorProto.FLOAT, [1]),
        ],
        initializer=[beat_init, event_init, rhythm_init, zero],
    )
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
    model.ir_version = 8
    onnx.checker.check_model(model)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    onnx.save(model, OUT)
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
