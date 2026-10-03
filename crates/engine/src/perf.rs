//! `perf.stats`: performance counters an agent can read (headless engine part). The desktop UI
//! extends the same query with playback, frame-worker and UI timings (`ui-egui` `perf.rs`).

use serde_json::{Value, json};

use crate::Session;

/// Decoder / GOP-cache counters since the process started (they only grow: diff two readings to
/// measure an interval).
pub fn decode_json() -> Value {
    let g = filmcraft_codecs::gop_stats();
    json!({
        "requests": g.hits + g.misses,
        "cacheHits": g.hits,
        "cacheMisses": g.misses,
        "cacheHitRate": g.hit_rate(),
        "seeks": g.seeks,
        "samplesDecoded": g.decoded,
        "samplesSkipped": g.skipped,
        "evicted": g.evicted,
        "decodeMs": g.decode_ns as f64 / 1e6,
        "decodeMsPerSample": g.decode_ms_per_sample(),
    })
}

/// The engine's `perf.stats`.
pub fn stats(s: &Session) -> Value {
    let running = s.jobs.iter().filter(|j| j.result.lock().map(|r| r.is_none()).unwrap_or(false)).count();
    json!({
        "decode": decode_json(),
        "media": {"openSources": s.media.open_sources()},
        "jobs": {"total": s.jobs.len(), "running": running},
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn perf_stats_query_reports_decode_counters() {
        let mut s = Session::default();
        assert!(s.execute("perf.stats", json!({})).is_ok(), "available without a project");
        s.execute("file.openDemoProject", json!({})).unwrap();
        let undo = s.history.undo.len();
        let v = s.execute("perf.stats", json!({})).unwrap();
        for k in ["requests", "cacheHitRate", "seeks", "samplesDecoded", "samplesSkipped", "decodeMs", "decodeMsPerSample"] {
            assert!(v["decode"][k].is_number(), "decode.{k} in {v}");
        }
        assert!(v["media"]["openSources"].is_number());
        assert_eq!(v["jobs"]["running"], json!(0));
        assert_eq!(s.history.undo.len(), undo, "a query adds no undo step");
    }
}
