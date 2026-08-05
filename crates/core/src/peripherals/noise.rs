// LabWired - Firmware Simulation Platform
// Copyright (C) 2026 Andrii Shylenko
//
// This software is released under the MIT License.
// See the LICENSE file in the project root for full license information.

//! Seeded per-channel sensor noise: Gaussian noise + constant bias + optional
//! first-order thermal lag, keyed by `(run_seed, component_id, channel)` so a
//! run replays bit-identically. This is the one noise facility shared by the
//! declarative I²C interpreter and the hand-written Rust kits — sensor models
//! never roll their own RNG.

/// SplitMix64 — small, fast, deterministic. No external RNG dependency.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform on (0, 1) — never exactly 0, so `ln()` is safe.
    pub fn next_f64_open01(&mut self) -> f64 {
        let v = (self.next_u64() >> 11) as f64; // 53-bit mantissa on [0, 2^53)
        (v + 0.5) / 9_007_199_254_740_992.0
    }
}

/// Derive a channel seed from the run seed and stable string identity
/// (FNV-1a over the parts, then one SplitMix64 mixing step).
pub fn channel_seed(run_seed: u64, component_id: &str, channel: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in run_seed
        .to_le_bytes()
        .iter()
        .chain(component_id.as_bytes())
        .chain(b"/")
        .chain(channel.as_bytes())
    {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    SplitMix64::new(h).next_u64()
}

/// Per-channel noise state. `sample` is called once per engineered-unit value
/// the model produces (i.e. per register read); with `tau_s` set and a known
/// `now_us`, the noise passes through a first-order lag (thermal sensors).
#[derive(Debug, Clone)]
pub struct ChannelNoise {
    rng: SplitMix64,
    sigma: f64,
    bias: f64,
    tau_s: Option<f64>,
    filtered: Option<f64>,
    last_t_us: Option<u64>,
    spare: Option<f64>,
}

impl ChannelNoise {
    pub fn new(
        run_seed: u64,
        component_id: &str,
        channel: &str,
        sigma: f64,
        bias: f64,
        tau_s: Option<f64>,
    ) -> Self {
        Self {
            rng: SplitMix64::new(channel_seed(run_seed, component_id, channel)),
            sigma,
            bias,
            tau_s,
            filtered: None,
            last_t_us: None,
            spare: None,
        }
    }

    /// Config accessors — needed when a device re-keys its noise states after
    /// the component id is stamped at attach time.
    pub fn sigma(&self) -> f64 {
        self.sigma
    }

    pub fn bias(&self) -> f64 {
        self.bias
    }

    pub fn tau_s(&self) -> Option<f64> {
        self.tau_s
    }

    /// True when this channel adds nothing (sigma = 0, bias = 0, no lag) —
    /// callers skip the sampling path entirely and stay byte-identical to
    /// pre-noise behavior.
    pub fn is_noop(&self) -> bool {
        self.sigma == 0.0 && self.bias == 0.0 && self.tau_s.is_none()
    }

    fn gaussian(&mut self) -> f64 {
        if let Some(spare) = self.spare.take() {
            return spare;
        }
        let u1 = self.rng.next_f64_open01();
        let u2 = self.rng.next_f64_open01();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }

    /// Apply bias + Gaussian noise + optional thermal lag to `value` (in the
    /// channel's engineering unit). `now_us` is the device's current simulated
    /// time when known (`None` → lag is skipped, noise/bias still apply).
    pub fn sample(&mut self, value: f64, now_us: Option<u64>) -> f64 {
        let noisy = value + self.bias + self.sigma * self.gaussian();
        let Some(tau) = self.tau_s else { return noisy };
        let Some(now) = now_us else { return noisy };
        let (prev_out, prev_t) = match (self.filtered, self.last_t_us) {
            (Some(o), Some(t)) => (o, t),
            _ => {
                // First sample: start at the ideal value so a step input does
                // not jump to noisy on sample one.
                self.filtered = Some(value);
                self.last_t_us = Some(now);
                return value;
            }
        };
        let dt_s = now.saturating_sub(prev_t) as f64 / 1_000_000.0;
        let alpha = 1.0 - (-dt_s / tau).exp();
        let out = prev_out + alpha * (noisy - prev_out);
        self.filtered = Some(out);
        self.last_t_us = Some(now);
        out
    }
}
