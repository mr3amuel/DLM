use std::time::Duration;
/// A controlled throughput trial follows a stable baseline, with rollback and cooldown.
pub(crate) struct Adaptive {
    pub target: u32,
    maximum: u32,
    automatic: bool,
    baseline: Option<f64>,
    trial: Option<(u32, f64)>,
    cooldown: Duration,
}
impl Adaptive {
    pub fn new(maximum: u32, automatic: bool) -> Self {
        Self {
            target: if automatic { 1 } else { maximum },
            maximum,
            automatic,
            baseline: None,
            trial: None,
            cooldown: Duration::ZERO,
        }
    }
    pub fn resume_retry(&mut self, target: u32) {
        self.target = target.clamp(1, self.maximum);
        if self.automatic {
            self.cooldown = Duration::from_secs(15);
        }
    }
    pub fn sample_capped(
        &mut self,
        bytes_per_second: f64,
        elapsed: Duration,
        remaining: u64,
        errors: bool,
        capped: bool,
    ) {
        if !self.automatic {
            if errors {
                self.target = (self.target / 2).max(1);
            }
            return;
        }
        if errors {
            self.target = (self.target / 2).max(1);
            self.trial = None;
            self.baseline = None;
            self.cooldown = Duration::from_secs(15);
            return;
        }
        if capped {
            // A policy ceiling is not evidence that extra connections help.
            // Forget pre-cap measurements so removing a cap needs a fresh baseline.
            if let Some((previous, _)) = self.trial.take() {
                self.target = previous;
            }
            self.baseline = None;
            return;
        }
        if !self.cooldown.is_zero() {
            self.cooldown = self.cooldown.saturating_sub(elapsed);
            return;
        }
        if bytes_per_second < 128.0 * 1024.0 {
            return;
        }
        if let Some((previous, rate)) = self.trial.take() {
            if bytes_per_second < rate * 1.15 {
                self.target = previous;
                self.cooldown = Duration::from_secs(30);
                self.baseline = None;
                return;
            }
            self.baseline = Some(bytes_per_second);
            return;
        }
        // Do not spend the final seconds of a download on a new trial.
        if (remaining as f64) < bytes_per_second * 4.0 {
            return;
        }
        let stable = self
            .baseline
            .is_some_and(|rate| (bytes_per_second - rate).abs() / rate.max(1.0) < 0.20);
        if stable && self.target < self.maximum {
            self.trial = Some((self.target, bytes_per_second));
            self.target = (self.target * 2).min(self.maximum);
        }
        self.baseline = Some(bytes_per_second);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capped_windows_never_scale_and_clear_trial_measurements() {
        let mut a = Adaptive::new(8, true);
        for _ in 0..20 {
            a.sample_capped(1e6, Duration::from_secs(2), 100_000_000, false, true);
            assert_eq!(a.target, 1);
        }
        for expected in [1, 2] {
            a.sample_capped(1e6, Duration::from_secs(2), 100_000_000, false, false);
            assert_eq!(a.target, expected);
        }
        a.sample_capped(1e6, Duration::from_secs(2), 100_000_000, false, true);
        assert_eq!(a.target, 1);
        a.sample_capped(1e6, Duration::from_secs(2), 100_000_000, false, false);
        assert_eq!(a.target, 1);
    }
    #[test]
    fn measured_trial_rollback_and_throttling() {
        let mut a = Adaptive::new(8, true);
        for expected in [1, 2] {
            a.sample_capped(1e6, Duration::from_secs(2), 100_000_000, false, false);
            assert_eq!(a.target, expected);
        }
        a.sample_capped(1.01e6, Duration::from_secs(2), 100_000_000, false, false);
        assert_eq!(a.target, 1);
        for _ in 0..5 {
            a.sample_capped(2e6, Duration::from_secs(2), 100_000_000, false, false);
            assert_eq!(a.target, 1);
        }
        let mut a = Adaptive::new(8, false);
        a.sample_capped(1e6, Duration::from_secs(2), 100000, true, false);
        assert_eq!(a.target, 4);
    }
}
