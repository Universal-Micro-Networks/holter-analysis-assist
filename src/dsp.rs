//! DSP helpers matching SciPy used by BeatSense preprocess/postprocess.
//!
//! - `sosfiltfilt`: zero-phase SOS IIR (Butterworth BPF coeffs embedded)
//! - `resample_poly_2x`: SciPy `resample_poly(x, 2, 1)` with Kaiser FIR
//! - `find_peaks`: SciPy-like peak picking with `height` + `distance`

/// Butterworth order-4 band-pass 0.3–100 Hz @ 250 Hz (`scipy.signal.butter`, SOS).
pub const BPF_SOS_250: [[f64; 6]; 4] = [
    [
        0.428_340_395_315_388_53,
        0.856_680_790_630_777_1,
        0.428_340_395_315_388_53,
        1.0,
        1.047_169_004_843_189_4,
        0.295_532_532_881_997_66,
    ],
    [
        1.0,
        2.0,
        1.0,
        1.0,
        1.321_149_074_160_525,
        0.633_279_421_291_207_2,
    ],
    [
        1.0,
        -2.0,
        1.0,
        1.0,
        -1.986_084_210_244_221_8,
        0.986_140_901_879_669_1,
    ],
    [
        1.0,
        -2.0,
        1.0,
        1.0,
        -1.994_199_152_353_315_4,
        0.994_255_878_265_328,
    ],
];

/// SciPy default FIR for `resample_poly(..., up=2, down=1)` before pre-pad.
const RESAMPLE_FIR_BASE: [f64; 41] = [
    -1.431_794_190_177_291_7e-18,
    -0.002_102_917_545_350_243,
    3.708_599_648_663_8e-18,
    0.005_017_933_624_295_429,
    -6.988_290_467_901_935e-18,
    -0.009_789_668_678_575_142,
    1.121_290_232_474_962e-17,
    0.017_113_118_004_962_808,
    -1.618_202_618_890_599e-17,
    -0.027_979_946_476_870_295,
    2.156_226_402_874_920_4e-17,
    0.044_046_241_148_149_22,
    -2.691_935_484_579_478e-17,
    -0.068_680_359_763_489_84,
    3.176_920_136_590_834e-17,
    0.110_576_567_823_714_03,
    -3.564_041_417_589_72e-17,
    -0.201_860_394_795_467_63,
    3.813_859_367_692_727e-17,
    0.633_400_691_360_601_4,
    1.000_517_470_596_060_2,
    0.633_400_691_360_601_4,
    3.813_859_367_692_727e-17,
    -0.201_860_394_795_467_63,
    -3.564_041_417_589_72e-17,
    0.110_576_567_823_714_03,
    3.176_920_136_590_834e-17,
    -0.068_680_359_763_489_84,
    -2.691_935_484_579_478e-17,
    0.044_046_241_148_149_22,
    2.156_226_402_874_920_4e-17,
    -0.027_979_946_476_870_295,
    -1.618_202_618_890_599e-17,
    0.017_113_118_004_962_808,
    1.121_290_232_474_962e-17,
    -0.009_789_668_678_575_142,
    -6.988_290_467_901_935e-18,
    0.005_017_933_624_295_429,
    3.708_599_648_663_8e-18,
    -0.002_102_917_545_350_243,
    -1.431_794_190_177_291_7e-18,
];

/// Zero-phase SOS filtering (`scipy.signal.sosfiltfilt`).
pub fn sosfiltfilt(sos: &[[f64; 6]], x: &[f32]) -> Vec<f32> {
    let n_sections = sos.len();
    let mut ntaps = 2 * n_sections + 1;
    let mut b2_zeros = 0usize;
    let mut a2_zeros = 0usize;
    for row in sos {
        if row[2] == 0.0 {
            b2_zeros += 1;
        }
        if row[5] == 0.0 {
            a2_zeros += 1;
        }
    }
    ntaps -= b2_zeros.min(a2_zeros);
    let edge = 3 * ntaps;
    assert!(
        x.len() > edge,
        "signal too short for sosfiltfilt (len={}, need > {})",
        x.len(),
        edge
    );

    // Odd extension padding (scipy default).
    let mut ext = Vec::with_capacity(x.len() + 2 * edge);
    for i in 0..edge {
        let v = x[edge - i] as f64;
        ext.push(2.0 * x[0] as f64 - v);
    }
    ext.extend(x.iter().map(|v| *v as f64));
    for i in 0..edge {
        let v = x[x.len() - 2 - i] as f64;
        ext.push(2.0 * *x.last().unwrap() as f64 - v);
    }

    let mut y = sosfilt(sos, &ext);
    y.reverse();
    y = sosfilt(sos, &y);
    y.reverse();

    y[edge..edge + x.len()].iter().map(|v| *v as f32).collect()
}

fn sosfilt(sos: &[[f64; 6]], x: &[f64]) -> Vec<f64> {
    let mut y = x.to_vec();
    for section in sos {
        y = sosfilt_section(section, &y);
    }
    y
}

fn sosfilt_section(s: &[f64; 6], x: &[f64]) -> Vec<f64> {
    let (b0, b1, b2, _a0, a1, a2) = (s[0], s[1], s[2], s[3], s[4], s[5]);
    let mut y = vec![0.0; x.len()];
    let mut z0 = 0.0;
    let mut z1 = 0.0;
    for (n, &xn) in x.iter().enumerate() {
        let yn = b0 * xn + z0;
        z0 = b1 * xn - a1 * yn + z1;
        z1 = b2 * xn - a2 * yn;
        y[n] = yn;
    }
    y
}

/// SciPy `resample_poly(x, up=2, down=1)` for 1-D f32.
pub fn resample_poly_2x(x: &[f32]) -> Vec<f32> {
    const UP: usize = 2;
    const DOWN: usize = 1;
    let half_len = 10 * UP.max(DOWN); // 20
    let n_in = x.len();
    let n_out = n_in * UP / DOWN;

    // SciPy: n_pre_pad = (down - half_len % down); for down=1 → 1
    let n_pre_pad = 1usize;
    let mut h = Vec::with_capacity(RESAMPLE_FIR_BASE.len() + n_pre_pad);
    h.extend(std::iter::repeat(0.0).take(n_pre_pad));
    h.extend_from_slice(&RESAMPLE_FIR_BASE);

    let n_pre_remove = (half_len + n_pre_pad) / DOWN; // 21
    let y = upfirdn(&h, x, UP, DOWN);
    let end = (n_pre_remove + n_out).min(y.len());
    y[n_pre_remove..end].to_vec()
}

/// Upsample-by-`up`, FIR filter, then downsample-by-`down` (constant-0 edges).
fn upfirdn(h: &[f64], x: &[f32], up: usize, down: usize) -> Vec<f32> {
    // Output length of upfirdn: ((len(x) - 1) * up + len(h) - 1) / down + 1
    let out_len = ((x.len().saturating_sub(1)) * up + h.len().saturating_sub(1)) / down + 1;
    let mut y = vec![0.0_f32; out_len];
    for (i, &xi) in x.iter().enumerate() {
        let base = i * up;
        for (k, &hk) in h.iter().enumerate() {
            let idx = (base + k) / down;
            if (base + k) % down == 0 && idx < out_len {
                y[idx] += (xi as f64 * hk) as f32;
            }
        }
    }
    y
}

/// SciPy-like `find_peaks(x, height, distance)`.
pub fn find_peaks(x: &[f32], height: f32, distance: usize) -> Vec<(usize, f32)> {
    if x.len() < 3 {
        return Vec::new();
    }
    let mut peaks = Vec::new();
    for i in 1..x.len() - 1 {
        if x[i] >= height && x[i] > x[i - 1] && x[i] >= x[i + 1] {
            // Prefer left plateau edge similar to scipy for flat tops.
            if x[i] == x[i + 1] && x[i] > x[i - 1] {
                // keep
            }
            peaks.push((i, x[i]));
        }
    }
    if distance <= 1 || peaks.is_empty() {
        return peaks;
    }
    // Greedy keep-highest with distance, scanning by descending height.
    let mut order: Vec<usize> = (0..peaks.len()).collect();
    order.sort_by(|&a, &b| {
        peaks[b]
            .1
            .partial_cmp(&peaks[a].1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| peaks[a].0.cmp(&peaks[b].0))
    });
    let mut selected = Vec::new();
    for idx in order {
        let (pos, _) = peaks[idx];
        let conflict = selected.iter().any(|&(p, _)| pos.abs_diff(p) < distance);
        if !conflict {
            selected.push(peaks[idx]);
        }
    }
    selected.sort_by_key(|(p, _)| *p);
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sosfiltfilt_runs_on_long_signal() {
        let mut long = Vec::with_capacity(2000);
        let seed = [
            0.125_730_22_f32,
            -0.132_104_86,
            0.640_422_64,
            0.104_900_11,
            -0.535_669_4,
            0.361_595_06,
            1.304,
            0.947_080_97,
        ];
        while long.len() < 2000 {
            long.extend_from_slice(&seed);
        }
        long.truncate(2000);
        let y = sosfiltfilt(&BPF_SOS_250, &long);
        assert_eq!(y.len(), 2000);
        assert!(y.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn find_peaks_sine_matches_expected_positions() {
        let n = 1000usize;
        let sig: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / (n as f32 - 1.0);
                let s = (40.0 * std::f32::consts::PI * t).sin();
                (s + 1.0) / 2.0
            })
            .collect();
        let peaks = find_peaks(&sig, 0.9, 60);
        let positions: Vec<usize> = peaks.iter().map(|(p, _)| *p).collect();
        assert_eq!(
            positions,
            vec![12, 112, 212, 312, 412, 512, 612, 712, 812, 912]
        );
    }

    #[test]
    fn resample_poly_2x_length() {
        let x: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let y = resample_poly_2x(&x);
        assert_eq!(y.len(), 20);
    }
}
