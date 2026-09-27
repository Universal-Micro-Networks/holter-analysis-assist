"""Stdlib-only tests for ``export_onnx.py`` argument handling and metadata.

Run from repository root::

    python3 -m unittest tools/export/test_export_onnx.py
"""

from __future__ import annotations

import contextlib
import importlib.util
import io
import sys
import unittest
from pathlib import Path

_SCRIPT = Path(__file__).resolve().parent / "export_onnx.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("export_onnx", _SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


export_onnx = _load_module()

_HEAVY_MODULES = ("numpy", "tensorflow", "tf2onnx", "onnx", "onnxruntime")


def _parse_fails(argv: list[str]) -> bool:
    with contextlib.redirect_stderr(io.StringIO()):
        try:
            export_onnx.parse_args(argv)
        except SystemExit as exc:
            return exc.code != 0
    return False


class ParseArgsTest(unittest.TestCase):
    def test_default_is_fixed_batch_one(self) -> None:
        args = export_onnx.parse_args([])
        self.assertFalse(args.dynamic_batch)
        self.assertEqual(args.batch_size, 1)

    def test_fixed_batch_size_is_kept(self) -> None:
        args = export_onnx.parse_args(["--batch-size", "8"])
        self.assertFalse(args.dynamic_batch)
        self.assertEqual(args.batch_size, 8)

    def test_dynamic_batch_uses_default_verify_batch(self) -> None:
        args = export_onnx.parse_args(["--dynamic-batch"])
        self.assertTrue(args.dynamic_batch)
        self.assertEqual(args.verify_batch, export_onnx.DEFAULT_VERIFY_BATCH)
        self.assertGreaterEqual(export_onnx.DEFAULT_VERIFY_BATCH, 2)

    def test_dynamic_batch_accepts_verify_batch(self) -> None:
        args = export_onnx.parse_args(["--dynamic-batch", "--verify-batch", "16"])
        self.assertEqual(args.verify_batch, 16)

    def test_dynamic_batch_rejects_explicit_batch_size(self) -> None:
        self.assertTrue(_parse_fails(["--dynamic-batch", "--batch-size", "2"]))

    def test_verify_batch_requires_dynamic_batch(self) -> None:
        self.assertTrue(_parse_fails(["--verify-batch", "4"]))

    def test_verify_batch_must_be_at_least_two(self) -> None:
        self.assertTrue(_parse_fails(["--dynamic-batch", "--verify-batch", "1"]))

    def test_batch_size_must_be_positive(self) -> None:
        self.assertTrue(_parse_fails(["--batch-size", "0"]))

    def test_help_lists_new_options(self) -> None:
        out = io.StringIO()
        with contextlib.redirect_stdout(out), self.assertRaises(SystemExit):
            export_onnx.parse_args(["--help"])
        self.assertIn("--dynamic-batch", out.getvalue())
        self.assertIn("--verify-batch", out.getvalue())


class InputBatchDimTest(unittest.TestCase):
    def test_dynamic_is_none(self) -> None:
        self.assertIsNone(export_onnx.input_batch_dim(dynamic_batch=True, batch_size=1))

    def test_fixed_is_batch_size(self) -> None:
        self.assertEqual(
            export_onnx.input_batch_dim(dynamic_batch=False, batch_size=3), 3
        )


class BuildMetadataTest(unittest.TestCase):
    def _meta(self, batch_dim):
        return export_onnx.build_metadata(
            weights_rel="w.weights.h5",
            weights_sha256="abc",
            onnx_rel="m.onnx",
            opset=17,
            batch_dim=batch_dim,
            window_samples=10000,
            event_class_order=("PAC", "PVC", "N"),
            rhythm_negative="non_af",
            rhythm_positive="af",
            inference_output_names=("beat", "event", "rhythm"),
        )

    def test_fixed_metadata_keeps_existing_contract(self) -> None:
        meta = self._meta(1)
        self.assertNotIn("dynamic_batch", meta)
        self.assertEqual(meta["input"]["shape"], [1, 10000, 1])
        shapes = {o["name"]: o["shape"] for o in meta["outputs"]}
        self.assertEqual(shapes["beat"], [1, 10000, 1])
        self.assertEqual(shapes["event"], [1, 10000, 3])
        self.assertEqual(shapes["rhythm"], [1, 1])
        self.assertEqual(
            list(meta),
            [
                "weights",
                "weights_sha256",
                "onnx",
                "opset",
                "input",
                "outputs",
                "inference_output_names",
            ],
        )

    def test_dynamic_metadata_records_dynamic_batch(self) -> None:
        meta = self._meta(None)
        self.assertIs(meta["dynamic_batch"], True)
        self.assertEqual(meta["input"]["shape"], [None, 10000, 1])
        shapes = {o["name"]: o["shape"] for o in meta["outputs"]}
        self.assertEqual(shapes["beat"], [None, 10000, 1])
        self.assertEqual(shapes["event"], [None, 10000, 3])
        self.assertEqual(shapes["rhythm"], [None, 1])


class LightweightImportTest(unittest.TestCase):
    def test_module_import_does_not_pull_heavy_deps(self) -> None:
        for name in _HEAVY_MODULES:
            sys.modules.pop(name, None)
        _load_module()
        loaded = [name for name in _HEAVY_MODULES if name in sys.modules]
        self.assertEqual(loaded, [])


if __name__ == "__main__":
    unittest.main()
