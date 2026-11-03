/// Report allocator high-water usage rather than a sampled peak.
#[cfg(feature = "heap-stats")]
pub fn report(stage: &str) {
    let stats = esp_alloc::HEAP.stats();
    log::info!(
        "HEAP stage={} size={} used={} peak={} free={} min_free={}",
        stage,
        stats.size,
        stats.current_usage,
        stats.max_usage,
        stats.size - stats.current_usage,
        stats.size - stats.max_usage
    );
}

#[cfg(not(feature = "heap-stats"))]
pub fn report(_stage: &str) {}
