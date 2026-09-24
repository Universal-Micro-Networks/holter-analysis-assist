#!/usr/bin/env python3
"""Export BeatSense Phase-2 inference heads to ONNX for Rust ``ort``.

Pipeline
--------
1. ``build_model()`` + strict ``load_weights(*.weights.h5)``
2. Keep only runtime heads: beat / event / rhythm
3. ``tf2onnx.convert.from_keras`` → ``resources/models/phase2_rev1.onnx``
4. Smoke-check with ONNX Runtime and compare against Keras (optional)

Usage (from repository root)::

    python3 -m venv .venv-export
    source .venv-export/bin/activate
    pip install -r tools/export/requirements.txt
    PYTHONPATH=tools python tools/export/export_onnx.py \\
        --weights resources/models/phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5

Notes
-----
- Weight-only H5 cannot be converted without ``tools/beatsense/model.py``.
- ``Lambda`` / custom layers (ZScoreNormalize1D, SwiGLU1x1) must convert;
  if tf2onnx fails, inspect the failing op and replace before retrying.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_WEIGHTS = (
    REPO_ROOT
    / "resources"
    / "models"
    / "phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5"
)
DEFAULT_OUTPUT = REPO_ROOT / "resources" / "models" / "phase2_rev1.onnx"
DEFAULT_OPSET = 17
INPUT_NAME = "ecg"
OUTPUT_ORDER = ("beat", "event", "rhythm")


def _rel_to_repo(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def _ensure_tools_on_path() -> None:
    tools = str(REPO_ROOT / "tools")
    if tools not in sys.path:
        sys.path.insert(0, tools)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument(
        "--weights",
        type=Path,
        default=DEFAULT_WEIGHTS,
        help="Path to Phase-2 weight-only H5",
    )
    p.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help="Output ONNX path",
    )
    p.add_argument("--opset", type=int, default=DEFAULT_OPSET)
    p.add_argument(
        "--batch-size",
        type=int,
        default=1,
        help="Fixed batch size baked into ONNX input shape",
    )
    p.add_argument(
        "--skip-compare",
        action="store_true",
        help="Skip Keras vs ONNX Runtime numeric comparison",
    )
    p.add_argument(
        "--rtol",
        type=float,
        default=1e-3,
        help="Relative tolerance for numeric compare",
    )
    p.add_argument(
        "--atol",
        type=float,
        default=1e-4,
        help="Absolute tolerance for numeric compare",
    )
    return p.parse_args()


def export_onnx(
    *,
    weights: Path,
    output: Path,
    opset: int,
    batch_size: int,
) -> dict:
    import tensorflow as tf
    import tf2onnx

    from beatsense.model import (
        EVENT_CLASS_ORDER,
        INFERENCE_OUTPUT_NAMES,
        RHYTHM_NEGATIVE_CLASS,
        RHYTHM_POSITIVE_CLASS,
        WINDOW_SAMPLES,
        load_phase2_model,
        print_io_spec,
    )

    if not weights.exists():
        raise FileNotFoundError(weights)

    print(f"Loading weights: {weights}")
    _full, inference_model, weight_sha256 = load_phase2_model(weights)
    print(f"weights sha256: {weight_sha256}")
    print_io_spec(inference_model)

    # Named single-tensor outputs in a stable order for ort consumers.
    ordered = [
        inference_model.get_layer(name).output for name in OUTPUT_ORDER
    ]
    export_model = tf.keras.Model(
        inputs=inference_model.input,
        outputs=ordered,
        name="beatsense_phase2_inference",
    )
    # Help tf2onnx / ORT use stable names.
    export_model.output_names = list(OUTPUT_ORDER)

    input_shape = (batch_size, WINDOW_SAMPLES, 1)
    spec = (tf.TensorSpec(input_shape, tf.float32, name=INPUT_NAME),)
    output.parent.mkdir(parents=True, exist_ok=True)

    print(f"Converting to ONNX (opset={opset}) → {output}")
    tf2onnx.convert.from_keras(
        export_model,
        input_signature=spec,
        opset=opset,
        output_path=str(output),
    )

    meta = {
        "weights": _rel_to_repo(weights),
        "weights_sha256": weight_sha256,
        "onnx": _rel_to_repo(output),
        "opset": opset,
        "input": {
            "name": INPUT_NAME,
            "shape": list(input_shape),
            "dtype": "float32",
            "layout": "NTC",
        },
        "outputs": [
            {
                "name": "beat",
                "shape": [batch_size, WINDOW_SAMPLES, 1],
                "activation": "sigmoid",
            },
            {
                "name": "event",
                "shape": [batch_size, WINDOW_SAMPLES, 3],
                "activation": "sigmoid",
                "channels": list(EVENT_CLASS_ORDER),
            },
            {
                "name": "rhythm",
                "shape": [batch_size, 1],
                "activation": "sigmoid",
                "negative": RHYTHM_NEGATIVE_CLASS,
                "positive": RHYTHM_POSITIVE_CLASS,
            },
        ],
        "inference_output_names": list(INFERENCE_OUTPUT_NAMES),
    }
    meta_path = output.with_suffix(".onnx.json")
    meta_path.write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote metadata: {meta_path}")
    return {"meta": meta, "export_model": export_model, "meta_path": meta_path}


def smoke_and_compare(
    *,
    onnx_path: Path,
    export_model,
    batch_size: int,
    rtol: float,
    atol: float,
) -> None:
    import onnx
    import onnxruntime as ort
    from beatsense.model import WINDOW_SAMPLES

    print("Checking ONNX model...")
    onnx.checker.check_model(onnx.load(str(onnx_path)))

    rng = np.random.default_rng(0)
    x = rng.standard_normal((batch_size, WINDOW_SAMPLES, 1), dtype=np.float32)

    keras_outs = export_model.predict(x, verbose=0)
    if isinstance(keras_outs, dict):
        keras_outs = [keras_outs[n] for n in OUTPUT_ORDER]

    sess = ort.InferenceSession(
        str(onnx_path), providers=["CPUExecutionProvider"]
    )
    input_name = sess.get_inputs()[0].name
    ort_outs = sess.run(None, {input_name: x})

    print("ONNX inputs :", [(i.name, i.shape, i.type) for i in sess.get_inputs()])
    print("ONNX outputs:", [(o.name, o.shape, o.type) for o in sess.get_outputs()])

    for name, k, o in zip(OUTPUT_ORDER, keras_outs, ort_outs):
        max_abs = float(np.max(np.abs(k - o)))
        ok = np.allclose(k, o, rtol=rtol, atol=atol)
        print(
            f"compare {name:6s}: shape={tuple(o.shape)} "
            f"max_abs_diff={max_abs:.6g} allclose={ok}"
        )
        if not ok:
            raise SystemExit(
                f"Numeric mismatch on '{name}' "
                f"(max_abs_diff={max_abs}, rtol={rtol}, atol={atol})"
            )


def main() -> int:
    args = parse_args()
    _ensure_tools_on_path()

    result = export_onnx(
        weights=args.weights.resolve(),
        output=args.output.resolve(),
        opset=args.opset,
        batch_size=args.batch_size,
    )

    if not args.skip_compare:
        smoke_and_compare(
            onnx_path=args.output.resolve(),
            export_model=result["export_model"],
            batch_size=args.batch_size,
            rtol=args.rtol,
            atol=args.atol,
        )

    print("OK: ONNX export completed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
