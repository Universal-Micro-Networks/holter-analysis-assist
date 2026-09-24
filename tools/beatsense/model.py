"""BeatSense Phase-2 model definition and weight-loading reference.

このファイルは ``phase2_*.weights.h5`` から学習時モデルを復元するための
モデル定義だけを保持します。ECL前処理は ``preprocess.py``、推論後処理は
``postprocess.py`` に分離しています。

Public runtime I/O
------------------
input  : ecg, float32, (B, 10000, 1), NTC
beat   : float32, (B, 10000, 1), sigmoid
event  : float32, (B, 10000, 3), sigmoid, channel=[PAC, PVC, N]
rhythm : float32, (B, 1), sigmoid, positive=AF/AFL, negative=SR

補助headはweight-only H5をstrictに復元するためfull graph内に残しますが、
通常のRust推論I/Oとして使用するのは beat / event / rhythm の3 headです。
"""

from __future__ import annotations

import hashlib
from pathlib import Path

import tensorflow as tf
from tensorflow.keras import layers


# =============================================================================
# Model I/O and topology constants
# =============================================================================

MODEL_FS_HZ = 500
WINDOW_SEC = 20.0
WINDOW_SAMPLES = int(round(MODEL_FS_HZ * WINDOW_SEC))  # 10,000
INPUT_CHANNELS = 1

BASE_FILTERS = 256
USE_BEAT_CONTEXT_FOR_EVENT = True
BEAT_CONTEXT_POOL = 4
BEAT_CONTEXT_FILTERS = 64
BEAT_CONTEXT_DILATIONS = (1, 2, 4, 8, 16, 32, 64)
BEAT_CONTEXT_DROPOUT_RATE = 0.10
USE_BEAT_CLASSIFIER = True
ABLATE_AUX_TASKS = False
BOTTLENECK_DILATIONS = (1, 2, 4, 8, 16, 32, 64)

EVENT_CLASS_ORDER = ("PAC", "PVC", "N")
RHYTHM_NEGATIVE_CLASS = "SR"
RHYTHM_POSITIVE_CLASS = "AF/AFL"

FULL_OUTPUT_NAMES = {
    "event", "beat", "rhythm", "avb", "morph", "ectopy_segment", "beat_class"
}
INFERENCE_OUTPUT_NAMES = ("beat", "event", "rhythm")

def norm1d(name: str | None = None) -> layers.Layer:
    """Channel-wise LayerNorm used throughout the model."""
    return layers.LayerNormalization(axis=-1, epsilon=1e-5, name=name)


def inception_residual_1d(
    x: tf.Tensor,
    filters: int,
    *,
    stride: int = 1,
    res_scale: float = 0.4,
    name: str = "inc",
) -> tf.Tensor:
    """Multi-scale 1-D Inception-residual encoder block."""
    in_ch = int(x.shape[-1])
    bf = filters // 4

    # Shared 1x1 bottleneck. If stride=2, downsampling happens here.
    z = layers.Conv1D(
        bf, 1, strides=stride, padding="same", use_bias=False,
        name=f"{name}_bn_conv",
    )(x)
    z = norm1d(f"{name}_bn_norm")(z)
    z = layers.Activation("swish", name=f"{name}_bn_act")(z)

    # Branch 1: short temporal scale, 5 -> 3.
    b1 = layers.Conv1D(
        bf, 5, padding="same", use_bias=False, name=f"{name}_b1_k5"
    )(z)
    b1 = norm1d(f"{name}_b1_k5_norm")(b1)
    b1 = layers.Activation("swish", name=f"{name}_b1_k5_act")(b1)
    b1 = layers.Conv1D(
        bf, 3, padding="same", use_bias=False, name=f"{name}_b1_k3"
    )(b1)
    b1 = norm1d(f"{name}_b1_k3_norm")(b1)
    b1 = layers.Activation("swish", name=f"{name}_b1_k3_act")(b1)

    # Branch 2: medium temporal scale, 11 -> 7.
    b2 = layers.Conv1D(
        bf, 11, padding="same", use_bias=False, name=f"{name}_b2_k11"
    )(z)
    b2 = norm1d(f"{name}_b2_k11_norm")(b2)
    b2 = layers.Activation("swish", name=f"{name}_b2_k11_act")(b2)
    b2 = layers.Conv1D(
        bf, 7, padding="same", use_bias=False, name=f"{name}_b2_k7"
    )(b2)
    b2 = norm1d(f"{name}_b2_k7_norm")(b2)
    b2 = layers.Activation("swish", name=f"{name}_b2_k7_act")(b2)

    # Branch 3: long temporal scale, 21 -> 15.
    b3 = layers.Conv1D(
        bf, 21, padding="same", use_bias=False, name=f"{name}_b3_k21"
    )(z)
    b3 = norm1d(f"{name}_b3_k21_norm")(b3)
    b3 = layers.Activation("swish", name=f"{name}_b3_k21_act")(b3)
    b3 = layers.Conv1D(
        bf, 15, padding="same", use_bias=False, name=f"{name}_b3_k15"
    )(b3)
    b3 = norm1d(f"{name}_b3_k15_norm")(b3)
    b3 = layers.Activation("swish", name=f"{name}_b3_k15_act")(b3)

    # Branch 4: local max-pooling -> 1x1 projection.
    p = layers.MaxPooling1D(
        3, strides=1, padding="same", name=f"{name}_pool"
    )(z)
    p = layers.Conv1D(
        bf, 1, padding="same", use_bias=False, name=f"{name}_pool_proj"
    )(p)
    p = norm1d(f"{name}_pool_norm")(p)
    p = layers.Activation("swish", name=f"{name}_pool_act")(p)

    out = layers.Concatenate(name=f"{name}_concat")([b1, b2, b3, p])
    out = layers.Conv1D(
        filters, 1, padding="same", use_bias=False, name=f"{name}_mix"
    )(out)
    out = norm1d(f"{name}_mix_norm")(out)
    out = layers.Activation("swish", name=f"{name}_mix_act")(out)
    out = layers.SpatialDropout1D(0.10, name=f"{name}_drop")(out)

    if stride == 1 and in_ch == filters:
        shortcut = x
    else:
        shortcut = layers.Conv1D(
            filters,
            1,
            strides=stride,
            padding="same",
            use_bias=False,
            name=f"{name}_shortcut",
        )(x)
        shortcut = norm1d(f"{name}_shortcut_norm")(shortcut)

    out = layers.Lambda(
        lambda q: q * res_scale, name=f"{name}_res_scale"
    )(out)
    out = layers.Add(name=f"{name}_add")([shortcut, out])
    return layers.Activation("swish", name=f"{name}_out")(out)


def stage_inception_dilation(
    x: tf.Tensor,
    filters: int,
    stage_name: str,
    *,
    first_stride: int,
    n_inception: int,
    resscale: float = 0.4,
) -> tf.Tensor:
    """Build one encoder stage."""
    x = inception_residual_1d(
        x,
        filters,
        stride=first_stride,
        res_scale=resscale,
        name=f"{stage_name}_inc0",
    )
    for i in range(1, n_inception):
        x = inception_residual_1d(
            x,
            filters,
            stride=1,
            res_scale=resscale,
            name=f"{stage_name}_inc{i}",
        )
    return x


class SwiGLU1x1(layers.Layer):
    """Pointwise SwiGLU block used inside the TCN residual blocks."""

    def build(self, input_shape: tf.TensorShape) -> None:
        ch = int(input_shape[-1])
        # No explicit inner-layer name: this matches the training/inference source.
        self.pw = layers.Conv1D(
            2 * ch,
            1,
            padding="same",
            use_bias=True,
        )
        super().build(input_shape)

    def call(self, x: tf.Tensor) -> tf.Tensor:
        a, b = tf.split(self.pw(x), 2, axis=-1)
        return a * tf.nn.swish(b)


def dilated_residual_block(
    x: tf.Tensor,
    *,
    ch: int,
    k: int,
    dilation: int,
    drop: float,
    name: str,
    res_scale: float = 0.6,
) -> tf.Tensor:
    """Dilated residual TCN block.

    Note: the source comment mentions 0.4 in one place, but the actual function
    default used by the executed graph is ``res_scale=0.6``. This reference keeps
    the executable behavior (0.6), which is what matters for checkpoint parity.
    """
    shortcut = x

    z = layers.Conv1D(
        ch,
        kernel_size=k,
        padding="same",
        dilation_rate=dilation,
        use_bias=False,
        name=f"{name}_dconv",
    )(x)
    z = norm1d(f"{name}_norm1")(z)
    z = layers.Activation("swish", name=f"{name}_act1")(z)

    z = layers.Conv1D(
        ch,
        kernel_size=1,
        padding="same",
        use_bias=False,
        name=f"{name}_pw",
    )(z)
    z = norm1d(f"{name}_norm2")(z)
    z = SwiGLU1x1(name=f"{name}_swiglu")(z)

    if drop > 0.0:
        z = layers.SpatialDropout1D(rate=drop, name=f"{name}_drop")(z)

    if int(shortcut.shape[-1]) != ch:
        shortcut = layers.Conv1D(
            ch,
            kernel_size=1,
            padding="same",
            use_bias=False,
            name=f"{name}_shortcut",
        )(shortcut)
        shortcut = norm1d(f"{name}_shortcut_norm")(shortcut)

    z = layers.Lambda(
        lambda t: t * res_scale, name=f"{name}_res_scale"
    )(z)
    z = layers.Add(name=f"{name}_add")([shortcut, z])
    return layers.Activation("swish", name=f"{name}_out")(z)


def beat_context_block(
    beat_feature: tf.Tensor,
    beat_prob: tf.Tensor,
    *,
    name: str = "beat_context",
) -> tf.Tensor:
    """Build the long-context BEAT branch that is concatenated into EVENT."""
    # EVENT loss was intentionally blocked from back-propagating into BEAT.
    f = layers.Lambda(
        lambda x: tf.stop_gradient(x), name=f"{name}_feature_stopgrad"
    )(beat_feature)
    p = layers.Lambda(
        lambda x: tf.stop_gradient(x), name=f"{name}_prob_stopgrad"
    )(beat_prob)

    c = layers.Concatenate(name=f"{name}_input")([f, p])
    c = layers.AveragePooling1D(
        pool_size=BEAT_CONTEXT_POOL,
        strides=BEAT_CONTEXT_POOL,
        padding="valid",
        name=f"{name}_pool",
    )(c)
    c = layers.Conv1D(
        BEAT_CONTEXT_FILTERS,
        1,
        padding="same",
        use_bias=False,
        name=f"{name}_proj",
    )(c)
    c = norm1d(f"{name}_proj_norm")(c)
    c = layers.Activation("swish", name=f"{name}_proj_act")(c)

    for i, d in enumerate(BEAT_CONTEXT_DILATIONS):
        sc = c
        z = layers.Conv1D(
            BEAT_CONTEXT_FILTERS,
            5,
            padding="same",
            dilation_rate=d,
            use_bias=False,
            name=f"{name}_d{i}_{d}",
        )(c)
        z = norm1d(f"{name}_d{i}_{d}_norm")(z)
        z = layers.Activation("swish", name=f"{name}_d{i}_{d}_act")(z)
        z = layers.Conv1D(
            BEAT_CONTEXT_FILTERS,
            1,
            padding="same",
            use_bias=False,
            name=f"{name}_d{i}_{d}_pw",
        )(z)
        z = norm1d(f"{name}_d{i}_{d}_pw_norm")(z)
        c = layers.Add(name=f"{name}_d{i}_{d}_add")([sc, z])
        c = layers.Activation("swish", name=f"{name}_d{i}_{d}_out")(c)

    return layers.UpSampling1D(
        size=BEAT_CONTEXT_POOL,
        name=f"{name}_upsample",
    )(c)


class ZScoreNormalize1D(tf.keras.layers.Layer):
    """Model-internal time-axis z-score layer from the training graph."""

    def __init__(self, eps: float = 1e-6, **kwargs) -> None:
        super().__init__(**kwargs)
        self.eps = float(eps)

    def call(self, x: tf.Tensor) -> tf.Tensor:
        x = tf.cast(x, tf.float32)
        mean = tf.reduce_mean(x, axis=1, keepdims=True)
        std = tf.math.reduce_std(x, axis=1, keepdims=True)
        return (x - mean) / (std + self.eps)

    def get_config(self) -> dict:
        cfg = super().get_config()
        cfg.update({"eps": self.eps})
        return cfg


def build_model(
    *,
    length: int = WINDOW_SAMPLES,
    in_ch: int = INPUT_CHANNELS,
    base: int = BASE_FILTERS,
) -> tf.keras.Model:
    """Reconstruct the full Phase-2 Keras topology required by ``*.weights.h5``.

    Do not remove the auxiliary heads before ``load_weights``. H5 weight-only
    checkpoints require the same weight-bearing graph to be reconstructed first.
    """
    inp = tf.keras.Input(shape=(length, in_ch), name="ecg")

    # Active at inference. External preprocessing has already z-scored each window,
    # but this layer is also part of the trained graph and must remain present.
    x = ZScoreNormalize1D(name="z_score_normalize1d")(inp)

    # Input projection, full-resolution skip.
    x = layers.Conv1D(
        base, 1, padding="same", use_bias=False, name="input_proj"
    )(x)
    x = norm1d("input_proj_norm")(x)
    x = layers.Activation("swish", name="input_proj_act")(x)
    skip0 = x  # (B, 10000, base)

    # Encoder: 10000 -> 5000 -> 2500 -> 1250.
    depths = (2, 4, 8)

    x = stage_inception_dilation(
        x, base, "st1", first_stride=2, n_inception=depths[0], resscale=0.8
    )
    x = layers.SpatialDropout1D(0.1, name="st1_drop")(x)
    skip1 = x

    x = stage_inception_dilation(
        x, base, "st2", first_stride=2, n_inception=depths[1], resscale=0.8
    )
    x = layers.SpatialDropout1D(0.1, name="st2_drop")(x)
    skip2 = x

    x = stage_inception_dilation(
        x, base, "st3", first_stride=2, n_inception=depths[2], resscale=0.8
    )
    x = layers.SpatialDropout1D(0.1, name="st3_drop")(x)
    shared_feat = x  # (B, 1250, base)

    # Dilated TCN bottleneck at an effective 62.5-Hz time grid.
    b = shared_feat
    for i, d in enumerate(BOTTLENECK_DILATIONS):
        b = dilated_residual_block(
            b,
            ch=base,
            k=7,
            dilation=d,
            drop=0.1,
            name=f"bottleneck_tcn_{i}_d{d}",
        )

    # -------------------------------------------------------------------------
    # Segment-level rhythm + auxiliary heads
    # -------------------------------------------------------------------------
    rh_avg = layers.GlobalAveragePooling1D(name="rhythm_gap")(b)
    rh_max = layers.GlobalMaxPooling1D(name="rhythm_gmp")(b)
    rh = layers.Concatenate(name="rhythm_pool_concat")([rh_avg, rh_max])
    rh = layers.Dense(base // 2, activation="swish", name="rhythm_dense")(rh)
    rh = layers.Dropout(0.15, name="rhythm_drop")(rh)
    out_rhythm = layers.Dense(
        1, activation="sigmoid", dtype="float32", name="rhythm"
    )(rh)

    # Auxiliary segment heads are kept for H5 topology compatibility.
    segment_pool = layers.Concatenate(name="segment_pool_concat")([rh_avg, rh_max])
    segment_shared = layers.Dense(
        base // 2, activation="swish", name="segment_shared_dense"
    )(segment_pool)
    segment_shared = layers.Dropout(0.10, name="segment_shared_drop")(
        segment_shared
    )
    out_avb = layers.Dense(
        1, activation="sigmoid", dtype="float32", name="avb"
    )(segment_shared)
    out_morph = layers.Dense(
        2, activation="sigmoid", dtype="float32", name="morph"
    )(segment_shared)

    # -------------------------------------------------------------------------
    # U-Net style decoder: 1250 -> 2500 -> 5000 -> 10000
    # -------------------------------------------------------------------------
    u = layers.UpSampling1D(2, name="up_625_to_1250")(b)
    u = layers.Concatenate(name="up3_concat")([u, skip2])
    for j, k in enumerate((9, 5)):
        u = layers.Conv1D(
            base, k, padding="same", use_bias=False, name=f"up3_conv{j + 1}"
        )(u)
        u = norm1d(f"up3_norm{j + 1}")(u)
        u = layers.Activation("swish", name=f"up3_act{j + 1}")(u)

    u = layers.UpSampling1D(2, name="up_1250_to_2500")(u)
    u = layers.Concatenate(name="up2_concat")([u, skip1])
    for j, k in enumerate((9, 5)):
        u = layers.Conv1D(
            base, k, padding="same", use_bias=False, name=f"up2_conv{j + 1}"
        )(u)
        u = norm1d(f"up2_norm{j + 1}")(u)
        u = layers.Activation("swish", name=f"up2_act{j + 1}")(u)

    u = layers.UpSampling1D(2, name="up_2500_to_5000")(u)
    u = layers.Concatenate(name="up1_concat")([u, skip0])
    for j, k in enumerate((9, 5)):
        u = layers.Conv1D(
            base, k, padding="same", use_bias=False, name=f"up1_conv{j + 1}"
        )(u)
        u = norm1d(f"up1_norm{j + 1}")(u)
        u = layers.Activation("swish", name=f"up1_act{j + 1}")(u)

    feat = u  # (B, 10000, base)

    # -------------------------------------------------------------------------
    # BEAT head: one sigmoid probability for every 500-Hz sample.
    # -------------------------------------------------------------------------
    beat_feature = layers.Conv1D(
        base // 2, 5, padding="same", use_bias=False, name="beat_head_conv1"
    )(feat)
    beat_feature = norm1d("beat_head_norm1")(beat_feature)
    beat_feature = layers.Activation("swish", name="beat_head_act1")(
        beat_feature
    )
    beat_feature = layers.Conv1D(
        base // 2, 3, padding="same", use_bias=False, name="beat_head_conv2"
    )(beat_feature)
    beat_feature = norm1d("beat_head_norm2")(beat_feature)
    beat_feature = layers.Activation("swish", name="beat_head_act2")(
        beat_feature
    )
    out_beat = layers.Conv1D(
        1,
        1,
        padding="same",
        activation="sigmoid",
        dtype="float32",
        name="beat",
    )(beat_feature)

    # BEAT features enter EVENT, but EVENT gradients were intentionally stopped.
    beat_feature_sg = layers.Lambda(
        lambda z: tf.stop_gradient(z), name="event_beat_feature_stopgrad"
    )(beat_feature)
    beat_prob_sg = layers.Lambda(
        lambda z: tf.stop_gradient(z), name="event_beat_prob_stopgrad"
    )(out_beat)

    if ABLATE_AUX_TASKS:
        beat_feature_for_event = layers.Lambda(
            lambda z: tf.zeros_like(z), name="event_beat_feature_ablation"
        )(beat_feature_sg)
        beat_prob_for_event = layers.Lambda(
            lambda z: tf.zeros_like(z), name="event_beat_prob_ablation"
        )(beat_prob_sg)
    else:
        beat_feature_for_event = beat_feature_sg
        beat_prob_for_event = beat_prob_sg

    event_inputs = [feat, beat_feature_for_event, beat_prob_for_event]

    if USE_BEAT_CONTEXT_FOR_EVENT:
        beat_ctx = beat_context_block(
            beat_feature_for_event,
            beat_prob_for_event,
            name="event_beat_context",
        )

        if ABLATE_AUX_TASKS:
            beat_ctx = layers.Lambda(
                lambda z: tf.zeros_like(z), name="event_beat_context_ablation"
            )(beat_ctx)
        elif BEAT_CONTEXT_DROPOUT_RATE > 0.0:
            # Inference uses training=False, so Dropout is inactive at runtime.
            beat_ctx = layers.Dropout(
                rate=BEAT_CONTEXT_DROPOUT_RATE,
                noise_shape=(None, 1, 1),
                name="event_beat_context_branch_dropout",
            )(beat_ctx)

        event_inputs.append(beat_ctx)

    event_shared = layers.Concatenate(name="event_input_concat")(event_inputs)

    # EVENT dense sigmoid segmentation trunk.
    ev = layers.Conv1D(
        base, 9, padding="same", use_bias=False, name="event_head_conv1"
    )(event_shared)
    ev = norm1d("event_head_norm1")(ev)
    ev = layers.Activation("swish", name="event_head_act1")(ev)

    ev = layers.Conv1D(
        base // 2, 5, padding="same", use_bias=False, name="event_head_conv2"
    )(ev)
    ev = norm1d("event_head_norm2")(ev)
    ev = layers.Activation("swish", name="event_head_act2")(ev)

    # Independent sigmoid channels, class order [PAC, PVC, N].
    out_event = layers.Conv1D(
        3,
        1,
        padding="same",
        activation="sigmoid",
        dtype="float32",
        name="event",
    )(ev)

    # Auxiliary segment-level ectopy head; kept for checkpoint compatibility.
    ect_gap = layers.GlobalAveragePooling1D(name="ectopy_segment_gap")(ev)
    ect_gmp = layers.GlobalMaxPooling1D(name="ectopy_segment_gmp")(ev)
    ect = layers.Concatenate(name="ectopy_segment_pool_concat")([ect_gap, ect_gmp])
    ect = layers.Dense(
        base // 4, activation="swish", name="ectopy_segment_dense"
    )(ect)
    ect = layers.Dropout(0.10, name="ectopy_segment_drop")(ect)
    out_ectopy_segment = layers.Dense(
        2, activation="sigmoid", dtype="float32", name="ectopy_segment"
    )(ect)

    outputs: dict[str, tf.Tensor] = {
        "event": out_event,
        "beat": out_beat,
        "rhythm": out_rhythm,
        "avb": out_avb,
        "morph": out_morph,
        "ectopy_segment": out_ectopy_segment,
    }

    # Checkpoint互換のため学習時の追加headもfull graph内には残す。
    # Rust側の公開推論I/Oには含めず、eventから最終PAC/PVC/Nを決める。
    if USE_BEAT_CLASSIFIER:
        bc = layers.Conv1D(
            base // 4,
            5,
            padding="same",
            use_bias=False,
            name="beat_class_conv1",
        )(event_shared)
        bc = norm1d("beat_class_norm1")(bc)
        bc = layers.Activation("swish", name="beat_class_act1")(bc)
        out_beat_class = layers.Conv1D(
            3,
            1,
            padding="same",
            activation="softmax",
            dtype="float32",
            name="beat_class",
        )(bc)
        outputs["beat_class"] = out_beat_class

    return tf.keras.Model(
        inp,
        outputs,
        name=f"Icentia_Inception_tcn_UNet_B{base}",
    )


# =============================================================================
# Strict weight loading and inference submodel
# =============================================================================

def sha256_file(path: str | Path, chunk_size: int = 1024 * 1024) -> str:
    """Return SHA-256 of a weight artifact for handoff/version verification."""
    h = hashlib.sha256()
    with Path(path).open("rb") as f:
        while True:
            chunk = f.read(chunk_size)
            if not chunk:
                break
            h.update(chunk)
    return h.hexdigest()


def validate_model_fingerprint(model: tf.keras.Model) -> None:
    """Fail early if the reconstructed graph is not the expected Phase-2 graph."""
    if model.input_shape != (None, WINDOW_SAMPLES, INPUT_CHANNELS):
        raise RuntimeError(f"Unexpected input shape: {model.input_shape}")

    if set(model.output_names) != FULL_OUTPUT_NAMES:
        raise RuntimeError(
            f"Unexpected full outputs: {model.output_names}; "
            f"expected set={sorted(FULL_OUTPUT_NAMES)}"
        )

    # Characteristic layers used by the original strict-load code.
    for i, d in enumerate(BOTTLENECK_DILATIONS):
        model.get_layer(f"bottleneck_tcn_{i}_d{d}_dconv")
    model.get_layer("st3_inc7_out")
    model.get_layer("z_score_normalize1d")
    for name in (
        "segment_shared_dense",
        "avb",
        "morph",
        "ectopy_segment_dense",
        "ectopy_segment",
        "beat_class",
    ):
        model.get_layer(name)


def load_phase2_model(
    weights_path: str | Path,
) -> tuple[tf.keras.Model, tf.keras.Model, str]:
    """Build the full graph, strictly load ``*.weights.h5``, then make runtime model.

    Returns
    -------
    full_model:
        Complete checkpoint-compatible graph including auxiliary heads.
    inference_model:
        Three-output submodel: beat, event, rhythm.
    weights_sha256:
        SHA-256 fingerprint of the loaded H5 file.
    """
    weights_path = Path(weights_path)
    if not weights_path.exists():
        raise FileNotFoundError(weights_path)

    # Reset Keras naming state before rebuilding the graph. This is particularly
    # useful because SwiGLU1x1 contains an inner Conv1D without an explicit name.
    tf.keras.backend.clear_session()

    full_model = build_model()
    validate_model_fingerprint(full_model)

    # No skip_mismatch / partial loading: a mismatch should fail visibly.
    full_model.load_weights(str(weights_path), skip_mismatch=False)

    inference_model = tf.keras.Model(
        inputs=full_model.input,
        outputs={
            "beat": full_model.get_layer("beat").output,
            "event": full_model.get_layer("event").output,
            "rhythm": full_model.get_layer("rhythm").output,
        },
        name=f"{full_model.name}_holter_eval",
    )

    return full_model, inference_model, sha256_file(weights_path)


def print_io_spec(model: tf.keras.Model) -> None:
    """Print the most important runtime I/O information."""
    print("input:", model.input_shape, model.input.dtype)
    print("outputs:")
    for name, tensor in zip(model.output_names, model.outputs):
        print(f"  {name:12s} shape={tuple(tensor.shape)} dtype={tensor.dtype}")
    print("EVENT class order    :", EVENT_CLASS_ORDER)
    print(
        "rhythm classes      :",
        f"0={RHYTHM_NEGATIVE_CLASS}, 1={RHYTHM_POSITIVE_CLASS}",
    )
