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

Add ``--dynamic-batch`` to export a variable batch dimension (FP32 only).
The Keras vs ONNX Runtime comparison then runs at batch ``--verify-batch``
and also checks every window of that batch against a batch-1 run.

Notes
-----
- Weight-only H5 cannot be converted without ``tools/beatsense/model.py``.
- ``Lambda`` / custom layers (ZScoreNormalize1D, SwiGLU1x1) must convert;
  if tf2onnx fails, inspect the failing op and replace before retrying.
- Heavy dependencies (numpy / TensorFlow / tf2onnx / onnxruntime) are
  imported inside functions so ``--help`` works with the stdlib only.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_WEIGHTS = (
    REPO_ROOT
    / "resources"
    / "models"
    / "phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5"
)
DEFAULT_OUTPUT = REPO_ROOT / "resources" / "models" / "phase2_rev1.onnx"
DEFAULT_OPSET = 17
DEFAULT_BATCH_SIZE = 1
DEFAULT_VERIFY_BATCH = 4
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


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
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
        default=None,
        help=(
            "Fixed batch size baked into ONNX input shape "
            f"(default: {DEFAULT_BATCH_SIZE}; not allowed with --dynamic-batch)"
        ),
    )
    p.add_argument(
        "--dynamic-batch",
        action="store_true",
        help=(
            "Export with a variable input batch dimension (None) and record "
            '"dynamic_batch": true in the metadata JSON. FP32 only.'
        ),
    )
    p.add_argument(
        "--verify-batch",
        type=int,
        default=None,
        metavar="N",
        help=(
            "With --dynamic-batch: compare Keras vs ONNX Runtime at batch N and "
            "check each window of batch N against a batch-1 run "
            f"(default: {DEFAULT_VERIFY_BATCH}; must be >= 2)"
        ),
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
    args = p.parse_args(argv)

    if args.dynamic_batch:
        if args.batch_size is not None:
            p.error("--batch-size cannot be combined with --dynamic-batch")
        if args.verify_batch is None:
            args.verify_batch = DEFAULT_VERIFY_BATCH
        if args.verify_batch < 2:
            p.error("--verify-batch must be >= 2")
    else:
        if args.verify_batch is not None:
            p.error("--verify-batch requires --dynamic-batch")
        if args.batch_size is None:
            args.batch_size = DEFAULT_BATCH_SIZE
        if args.batch_size < 1:
            p.error("--batch-size must be >= 1")
    return args


def input_batch_dim(*, dynamic_batch: bool, batch_size: int | None) -> int | None:
    """Batch dimension for the ONNX input signature (``None`` = variable)."""
    return None if dynamic_batch else batch_size


def build_metadata(
    *,
    weights_rel: str,
    weights_sha256: str,
    onnx_rel: str,
    opset: int,
    batch_dim: int | None,
    window_samples: int,
    event_class_order,
    rhythm_negative: str,
    rhythm_positive: str,
    inference_output_names,
) -> dict:
    """I/O contract written next to the ONNX file (``*.onnx.json``).

    ``batch_dim=None`` means a variable batch: shapes carry ``null`` in the
    batch position and ``"dynamic_batch": true`` is added.
    """
    meta: dict = {
        "weights": weights_rel,
        "weights_sha256": weights_sha256,
        "onnx": onnx_rel,
        "opset": opset,
    }
    if batch_dim is None:
        meta["dynamic_batch"] = True
    meta["input"] = {
        "name": INPUT_NAME,
        "shape": [batch_dim, window_samples, 1],
        "dtype": "float32",
        "layout": "NTC",
    }
    meta["outputs"] = [
        {
            "name": "beat",
            "shape": [batch_dim, window_samples, 1],
            "activation": "sigmoid",
        },
        {
            "name": "event",
            "shape": [batch_dim, window_samples, 3],
            "activation": "sigmoid",
            "channels": list(event_class_order),
        },
        {
            "name": "rhythm",
            "shape": [batch_dim, 1],
            "activation": "sigmoid",
            "negative": rhythm_negative,
            "positive": rhythm_positive,
        },
    ]
    meta["inference_output_names"] = list(inference_output_names)
    return meta


def export_onnx(
    *,
    weights: Path,
    output: Path,
    opset: int,
    batch_size: int | None,
    dynamic_batch: bool = False,
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

    batch_dim = input_batch_dim(dynamic_batch=dynamic_batch, batch_size=batch_size)
    input_shape = (batch_dim, WINDOW_SAMPLES, 1)
    spec = (tf.TensorSpec(input_shape, tf.float32, name=INPUT_NAME),)
    output.parent.mkdir(parents=True, exist_ok=True)

    batch_label = "dynamic" if batch_dim is None else str(batch_dim)
    print(f"Converting to ONNX (opset={opset}, batch={batch_label}) -> {output}")
    tf2onnx.convert.from_keras(
        export_model,
        input_signature=spec,
        opset=opset,
        output_path=str(output),
    )

    meta = build_metadata(
        weights_rel=_rel_to_repo(weights),
        weights_sha256=weight_sha256,
        onnx_rel=_rel_to_repo(output),
        opset=opset,
        batch_dim=batch_dim,
        window_samples=WINDOW_SAMPLES,
        event_class_order=EVENT_CLASS_ORDER,
        rhythm_negative=RHYTHM_NEGATIVE_CLASS,
        rhythm_positive=RHYTHM_POSITIVE_CLASS,
        inference_output_names=INFERENCE_OUTPUT_NAMES,
    )
    meta_path = output.with_suffix(".onnx.json")
    meta_path.write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote metadata: {meta_path}")
    return {"meta": meta, "export_model": export_model, "meta_path": meta_path}


def _check_allclose(
    *,
    label: str,
    expected_outs,
    actual_outs,
    rtol: float,
    atol: float,
) -> None:
    import numpy as np

    for name, k, o in zip(OUTPUT_ORDER, expected_outs, actual_outs):
        max_abs = float(np.max(np.abs(k - o)))
        ok = np.allclose(k, o, rtol=rtol, atol=atol)
        print(
            f"{label} {name:6s}: shape={tuple(o.shape)} "
            f"max_abs_diff={max_abs:.6g} allclose={ok}"
        )
        if not ok:
            raise SystemExit(
                f"Numeric mismatch on '{name}' ({label}) "
                f"(max_abs_diff={max_abs}, rtol={rtol}, atol={atol})"
            )


def _keras_predict(export_model, x) -> list:
    keras_outs = export_model.predict(x, verbose=0)
    if isinstance(keras_outs, dict):
        keras_outs = [keras_outs[n] for n in OUTPUT_ORDER]
    return list(keras_outs)


def _open_checked_session(onnx_path: Path):
    import onnx
    import onnxruntime as ort

    print("Checking ONNX model...")
    onnx.checker.check_model(onnx.load(str(onnx_path)))

    sess = ort.InferenceSession(
        str(onnx_path), providers=["CPUExecutionProvider"]
    )
    print("ONNX inputs :", [(i.name, i.shape, i.type) for i in sess.get_inputs()])
    print("ONNX outputs:", [(o.name, o.shape, o.type) for o in sess.get_outputs()])
    return sess


def smoke_and_compare(
    *,
    onnx_path: Path,
    export_model,
    batch_size: int,
    rtol: float,
    atol: float,
) -> None:
    import numpy as np
    from beatsense.model import WINDOW_SAMPLES

    sess = _open_checked_session(onnx_path)

    rng = np.random.default_rng(0)
    x = rng.standard_normal((batch_size, WINDOW_SAMPLES, 1), dtype=np.float32)

    keras_outs = _keras_predict(export_model, x)
    input_name = sess.get_inputs()[0].name
    ort_outs = sess.run(None, {input_name: x})

    _check_allclose(
        label="compare",
        expected_outs=keras_outs,
        actual_outs=ort_outs,
        rtol=rtol,
        atol=atol,
    )


def smoke_and_compare_dynamic(
    *,
    onnx_path: Path,
    export_model,
    verify_batch: int,
    rtol: float,
    atol: float,
) -> None:
    """Verify a ``--dynamic-batch`` export.

    1. The ONNX input batch dimension is symbolic (not baked in).
    2. Keras vs ONNX Runtime agree at batch ``verify_batch``.
    3. Each window of the batch-N ONNX output equals a batch-1 run of the
       same window (catches fixed-batch Reshape and cross-window mixing).
    """
    import numpy as np
    from beatsense.model import WINDOW_SAMPLES

    sess = _open_checked_session(onnx_path)
    input_meta = sess.get_inputs()[0]
    batch_axis = input_meta.shape[0]
    if isinstance(batch_axis, int):
        raise SystemExit(
            f"Input batch dimension is fixed ({batch_axis}); expected a dynamic "
            "dimension. The model likely contains a fixed-batch op; re-export "
            "with --batch-size N instead (Rust handles fixed batch N)."
        )
    print(f"ONNX input batch dimension is dynamic: {batch_axis!r}")

    rng = np.random.default_rng(0)
    x = rng.standard_normal((verify_batch, WINDOW_SAMPLES, 1), dtype=np.float32)
    input_name = input_meta.name

    keras_outs = _keras_predict(export_model, x)
    ort_outs = sess.run(None, {input_name: x})
    _check_allclose(
        label=f"compare[batch={verify_batch}]",
        expected_outs=keras_outs,
        actual_outs=ort_outs,
        rtol=rtol,
        atol=atol,
    )

    for i in range(verify_batch):
        single_outs = sess.run(None, {input_name: x[i : i + 1]})
        _check_allclose(
            label=f"batch{verify_batch}[{i}]-vs-batch1",
            expected_outs=single_outs,
            actual_outs=[o[i : i + 1] for o in ort_outs],
            rtol=rtol,
            atol=atol,
        )


def main() -> int:
    args = parse_args()
    _ensure_tools_on_path()

    result = export_onnx(
        weights=args.weights.resolve(),
        output=args.output.resolve(),
        opset=args.opset,
        batch_size=args.batch_size,
        dynamic_batch=args.dynamic_batch,
    )

    if not args.skip_compare:
        if args.dynamic_batch:
            smoke_and_compare_dynamic(
                onnx_path=args.output.resolve(),
                export_model=result["export_model"],
                verify_batch=args.verify_batch,
                rtol=args.rtol,
                atol=args.atol,
            )
        else:
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
