use std::path::Path;

use isolang::Language;
use serde::{Deserialize, Serialize};

use crate::cache::DiskCache;

/// Kalman filter over `output_json_size / input_source_length`.
#[derive(Clone, Serialize, Deserialize)]
pub struct TranslationSizeStats {
    pub ratio: f64,
    /// Error covariance (uncertainty).
    pub p: f64,
    pub n: u64,
}

impl Default for TranslationSizeStats {
    fn default() -> Self {
        Self {
            ratio: 50.0, // Initial estimate
            p: 100.0,    // High initial uncertainty
            n: 0,
        }
    }
}

impl TranslationSizeStats {
    /// Process noise: low, since a pair's ratio is fairly stable.
    const PROCESS_NOISE: f64 = 0.01;

    /// Measurement noise: high, to absorb variation and outliers.
    const MEASUREMENT_NOISE: f64 = 0.1;

    pub fn estimate(&self, source_len: usize) -> usize {
        (source_len as f64 * self.ratio).ceil() as usize
    }

    /// Fold one observed `(source_len, output_len)` pair into the estimate.
    pub fn update(&mut self, source_len: usize, output_len: usize) {
        if source_len == 0 {
            return;
        }

        let measured_ratio = output_len as f64 / source_len as f64;

        // Predict: the ratio is assumed constant, uncertainty grows.
        let p_predicted = self.p + Self::PROCESS_NOISE;

        let kalman_gain = p_predicted / (p_predicted + Self::MEASUREMENT_NOISE);
        self.ratio = self.ratio + kalman_gain * (measured_ratio - self.ratio);
        self.p = (1.0 - kalman_gain) * p_predicted;

        self.n += 1;
    }
}

/// Translation size statistics per language pair.
pub struct TranslationSizeCache {
    cache: DiskCache<TranslationSizeStats>,
}

impl TranslationSizeCache {
    pub async fn create(cache_dir: &Path) -> anyhow::Result<Self> {
        let stats_dir = cache_dir.join("translation_stats");
        let cache = DiskCache::open(&stats_dir, 16 * 1024 * 1024).await?;
        Ok(Self { cache })
    }

    pub async fn close(&self) {
        self.cache.close().await;
    }

    fn make_key(source_language: &Language, target_language: &Language) -> String {
        format!(
            "{}\n{}",
            source_language.to_639_3(),
            target_language.to_639_3()
        )
    }

    pub async fn get(
        &self,
        source_language: &Language,
        target_language: &Language,
    ) -> TranslationSizeStats {
        let key = Self::make_key(source_language, target_language);
        self.cache
            .get(&key)
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    pub async fn record_observation(
        &self,
        source_language: &Language,
        target_language: &Language,
        source_len: usize,
        output_len: usize,
    ) {
        let key = Self::make_key(source_language, target_language);
        let mut stats = self.get(source_language, target_language).await;
        stats.update(source_len, output_len);
        self.cache.insert(key, stats);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_estimate() {
        let stats = TranslationSizeStats::default();
        assert_eq!(stats.estimate(100), 5000); // 100 * 50
    }

    #[test]
    fn test_update_moves_estimate() {
        let mut stats = TranslationSizeStats::default();

        for _ in 0..20 {
            stats.update(100, 3000);
        }

        let estimate = stats.estimate(100);
        assert!(
            estimate < 5000,
            "Estimate should decrease from initial 5000"
        );
        assert!(
            estimate > 2500,
            "Estimate should be above 2500 (halfway to 3000)"
        );
    }

    #[test]
    fn test_outlier_resistance() {
        let mut stats = TranslationSizeStats::default();

        for _ in 0..10 {
            stats.update(100, 3000); // ratio = 30
        }
        let estimate_before = stats.ratio;

        stats.update(100, 50000); // ratio = 500 (extreme outlier)

        let estimate_after = stats.ratio;

        // Unfiltered, the estimate would jump by (500 - estimate_before).
        let change = (estimate_after - estimate_before).abs();
        let unfiltered_change = (500.0 - estimate_before).abs();
        assert!(
            change < unfiltered_change * 0.5,
            "Outlier impact ({:.1}) should be less than 50% of unfiltered ({:.1})",
            change,
            unfiltered_change
        );
    }

    #[test]
    fn test_zero_source_length() {
        let mut stats = TranslationSizeStats::default();
        let ratio_before = stats.ratio;
        stats.update(0, 1000);
        assert_eq!(
            stats.ratio, ratio_before,
            "Zero source length should be ignored"
        );
    }
}
