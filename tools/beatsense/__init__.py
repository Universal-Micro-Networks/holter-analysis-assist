"""BeatSense reference package.

Export / model-only consumers can import ``beatsense.model`` without installing
pandas/scipy. Full ECL analysis still uses ``analyze_ecl`` from ``analyzer``.
"""

from .model import build_model, load_phase2_model

__all__ = [
    "analyze_ecl",
    "build_model",
    "load_phase2_model",
]


def __getattr__(name: str):
    if name == "analyze_ecl":
        from .analyzer import analyze_ecl

        return analyze_ecl
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
