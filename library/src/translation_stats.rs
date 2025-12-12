use std::path::Path;

use foyer::{
    BlockEngineBuilder, DeviceBuilder, FsDeviceBuilder, HybridCache, HybridCacheBuilder,
    HybridCachePolicy,
};
use isolang::Language;
use serde::{Deserialize, Serialize};

/// Kalman filter state for estimating translation size ratio.
///
/// The ratio represents: output_json_size / input_source_length
#[derive(Clone, Serialize, Deserialize)]
pub struct TranslationSizeStats {
    /// Estimated ratio (output_size / input_size)
    pub ratio: f64,
    /// Kalman error covariance (uncertainty)
    pub p: f64,
    /// Number of observations
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
    /// Process noise (Q) - how much the true ratio can change between observations.
    /// Low value since translation ratios are relatively stable for a language pair.
    const PROCESS_NOISE: f64 = 0.01;

    /// Measurement noise (R) - variance in individual translation ratios.
    /// Higher value to handle natural variation and outliers.
    const MEASUREMENT_NOISE: f64 = 0.1;

    /// Get the estimated output size for a given source length.
    pub fn estimate(&self, source_len: usize) -> usize {
        (source_len as f64 * self.ratio).ceil() as usize
    }

    /// Update the estimate with a new observation using Kalman filter.
    ///
    /// Arguments:
    /// - `source_len`: Length of the source text
    /// - `output_len`: Actual length of the translation JSON output
    pub fn update(&mut self, source_len: usize, output_len: usize) {
        if source_len == 0 {
            return;
        }

        let measured_ratio = output_len as f64 / source_len as f64;

        // Kalman filter predict step
        // State doesn't change (ratio is assumed constant), but uncertainty increases
        let p_predicted = self.p + Self::PROCESS_NOISE;

        // Kalman filter update step
        let kalman_gain = p_predicted / (p_predicted + Self::MEASUREMENT_NOISE);
        self.ratio = self.ratio + kalman_gain * (measured_ratio - self.ratio);
        self.p = (1.0 - kalman_gain) * p_predicted;

        self.n += 1;
    }
}

/// Cache for storing translation size statistics per language pair.
pub struct TranslationSizeCache {
    cache: HybridCache<String, TranslationSizeStats>,
}

impl TranslationSizeCache {
    /// Create a new translation size cache in the given directory.
    pub async fn create(cache_dir: &Path) -> anyhow::Result<Self> {
        let stats_dir = cache_dir.join("translation_stats");
        std::fs::create_dir_all(&stats_dir)?;

        let device = FsDeviceBuilder::new(&stats_dir)
            .with_capacity(16 * 1024 * 1024) // 16MB should be plenty for stats
            .build()?;
        let cache = HybridCacheBuilder::new()
            .with_policy(HybridCachePolicy::WriteOnInsertion)
            .memory(1024 * 1024) // 1MB memory cache
            .storage()
            .with_engine_config(BlockEngineBuilder::new(device))
            .build()
            .await?;
        Ok(Self { cache })
    }

    fn make_key(source_language: &Language, target_language: &Language) -> String {
        format!(
            "{}\n{}",
            source_language.to_639_3(),
            target_language.to_639_3()
        )
    }

    /// Get statistics for a language pair, returning default if not found.
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
            .map(|r| r.value().clone())
            .unwrap_or_default()
    }

    /// Update statistics for a language pair with a new observation.
    pub async fn record_observation(
        &self,
        source_language: &Language,
        target_language: &Language,
        source_len: usize,
        output_len: usize,
    ) {
        let key = Self::make_key(source_language, target_language);

        // Get existing stats or default
        let mut stats = self.get(source_language, target_language).await;

        // Update with new observation
        stats.update(source_len, output_len);

        // Store back
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

        // Feed consistent observations with ratio ~30
        for _ in 0..20 {
            stats.update(100, 3000);
        }

        // Estimate should move toward 30
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

        // Establish a baseline with consistent observations
        for _ in 0..10 {
            stats.update(100, 3000); // ratio = 30
        }
        let estimate_before = stats.ratio;

        // Single extreme outlier
        stats.update(100, 50000); // ratio = 500 (extreme outlier)

        let estimate_after = stats.ratio;

        // The estimate should change, but not jump to the outlier value
        // If there were no filtering, it would jump by (500 - estimate_before)
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
