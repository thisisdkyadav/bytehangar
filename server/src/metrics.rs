//! Lightweight in-process counters exposed in Prometheus text format at
//! `GET /metrics` (internal plane). Atomic, dependency-free.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Metrics {
    uploads_total: AtomicU64,
    downloads_total: AtomicU64,
    deletes_total: AtomicU64,
    bytes_uploaded_total: AtomicU64,
    bytes_downloaded_total: AtomicU64,
}

impl Metrics {
    pub fn record_upload(&self, bytes: i64) {
        self.uploads_total.fetch_add(1, Ordering::Relaxed);
        self.bytes_uploaded_total
            .fetch_add(bytes.max(0) as u64, Ordering::Relaxed);
    }

    pub fn record_download(&self, bytes: i64) {
        self.downloads_total.fetch_add(1, Ordering::Relaxed);
        self.bytes_downloaded_total
            .fetch_add(bytes.max(0) as u64, Ordering::Relaxed);
    }

    pub fn record_delete(&self) {
        self.deletes_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn render_prometheus(&self) -> String {
        let counter = |name: &str, help: &str, value: u64| {
            format!("# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n")
        };
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        [
            counter("bytehangar_uploads_total", "Total successful uploads", load(&self.uploads_total)),
            counter("bytehangar_downloads_total", "Total successful downloads", load(&self.downloads_total)),
            counter("bytehangar_deletes_total", "Total file deletes", load(&self.deletes_total)),
            counter("bytehangar_bytes_uploaded_total", "Total bytes uploaded", load(&self.bytes_uploaded_total)),
            counter("bytehangar_bytes_downloaded_total", "Total bytes downloaded", load(&self.bytes_downloaded_total)),
        ]
        .concat()
    }
}
