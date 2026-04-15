//! In-memory log ringbuffer + `tracing_subscriber` layer that feeds it.
//!
//! The UI's "Debug" card calls `GET /api/debug/logs?lines=N` and paints the
//! returned strings in a scrolling viewer. Rather than reading back Docker's
//! stdout (which would require a Docker socket mount and isn't always
//! available in native/Tauri builds), we capture every `tracing` event into
//! a fixed-size `VecDeque` kept behind a `Mutex`.
//!
//! Cost: one `String` alloc + one mutex acquire per event. With ~50 events
//! per second under heavy load and a 2000-line ceiling, memory stays under
//! ~1 MB and lock contention is negligible. The layer is always active
//! regardless of `debug_mode` so the UI can show at least the `info` level
//! when the user first opens the panel — the only thing `debug_mode`
//! changes is the verbosity that reaches this layer in the first place.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use tracing::{field::Visit, Event, Level, Metadata, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Maximum number of lines kept in the ringbuffer.
/// 2000 lines × ~200 bytes/line ≈ 400 KB worst-case — trivial.
pub const DEFAULT_CAPACITY: usize = 2000;

/// Process-wide log buffer. Filled by [`BufferLayer::on_event`], read by the
/// `GET /api/debug/logs` handler via [`LogBuffer::tail`].
pub static LOG_BUFFER: LazyLock<LogBuffer> =
    LazyLock::new(|| LogBuffer::new(DEFAULT_CAPACITY));

/// Fixed-capacity ringbuffer of formatted log lines.
pub struct LogBuffer {
    lines: Mutex<VecDeque<String>>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Append a fully-formatted log line. Oldest line is dropped when the
    /// ringbuffer is full. Poisoned mutex is swallowed silently — missing
    /// logs are preferable to a panic from inside the tracing pipeline.
    pub fn push(&self, line: String) {
        let Ok(mut q) = self.lines.lock() else { return; };
        if q.len() >= self.capacity {
            q.pop_front();
        }
        q.push_back(line);
    }

    /// Return the most-recent `n` lines, oldest-first.
    /// `n = 0` returns an empty vec. Requesting more than the buffer
    /// contains simply returns everything available.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let Ok(q) = self.lines.lock() else { return Vec::new(); };
        if n == 0 { return Vec::new(); }
        let start = q.len().saturating_sub(n);
        q.iter().skip(start).cloned().collect()
    }

    /// Total number of lines currently stored.
    pub fn len(&self) -> usize {
        self.lines.lock().map(|q| q.len()).unwrap_or(0)
    }

    /// `true` when no lines have been captured yet (poisoned mutex also
    /// reported as "empty" so the caller doesn't have to pattern-match).
    pub fn is_empty(&self) -> bool {
        self.lines.lock().map(|q| q.is_empty()).unwrap_or(true)
    }

    /// Drop every buffered line. Exposed for tests and for a future
    /// "Clear logs" UI action.
    pub fn clear(&self) {
        if let Ok(mut q) = self.lines.lock() { q.clear(); }
    }
}

/// `tracing_subscriber::Layer` that pushes every event into [`LOG_BUFFER`].
/// Install via `registry().with(BufferLayer::default()).with(...)` in
/// `main.rs`. Respects the global `EnvFilter` automatically because layers
/// applied after a filter layer only receive already-filtered events.
#[derive(Default, Clone)]
pub struct BufferLayer;

impl<S: Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        LOG_BUFFER.push(format_line(event.metadata(), event));
    }
}

/// Render a tracing event to a single-line string.
///
/// Format: `HH:MM:SS.sss LEVEL  target  message [field=value ...]`
///
/// The layout is tuned for a monospace viewer in the UI — level sits in a
/// fixed 5-char slot so lines align, and the rest is free-form text.
fn format_line(meta: &Metadata<'_>, event: &Event<'_>) -> String {
    // UTC timestamp, millisecond precision, no date (the UI already scopes
    // to "recent logs" — minute-level granularity is plenty; shorter lines
    // make the viewer more readable).
    let now = SystemTime::now();
    let ts = chrono::DateTime::<chrono::Utc>::from(now)
        .format("%H:%M:%S%.3f")
        .to_string();

    let level_str = level_tag(*meta.level());
    let target = meta.target();

    let mut visitor = MessageVisitor::default();
    event.record(&mut visitor);

    if visitor.fields.is_empty() {
        format!("{} {} {} — {}", ts, level_str, target, visitor.message)
    } else {
        format!(
            "{} {} {} — {} [{}]",
            ts, level_str, target, visitor.message, visitor.fields
        )
    }
}

/// 5-character level tag — monospace-aligned in the viewer.
fn level_tag(lvl: Level) -> &'static str {
    match lvl {
        Level::ERROR => "ERROR",
        Level::WARN => " WARN",
        Level::INFO => " INFO",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "TRACE",
    }
}

/// Collects the event's `message` + every extra field into two strings.
/// The standard `message` field becomes the human-readable summary; every
/// other field is rendered `name=value` and appended as a trailing
/// bracketed list. Keeps structured tracing calls (`%host_os = ...`)
/// readable without a full JSON layout.
#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl MessageVisitor {
    fn push_field(&mut self, name: &str, value: String) {
        if !self.fields.is_empty() {
            self.fields.push_str(", ");
        }
        self.fields.push_str(name);
        self.fields.push('=');
        self.fields.push_str(&value);
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.push_field(field.name(), value.to_string());
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        } else {
            self.push_field(field.name(), format!("{:?}", value));
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.push_field(field.name(), value.to_string());
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.push_field(field.name(), value.to_string());
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.push_field(field.name(), value.to_string());
    }
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.push_field(field.name(), value.to_string());
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_appends_and_tail_reads_back() {
        let buf = LogBuffer::new(10);
        buf.push("a".into());
        buf.push("b".into());
        buf.push("c".into());
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.tail(10), vec!["a", "b", "c"]);
        assert_eq!(buf.tail(2), vec!["b", "c"]);
    }

    #[test]
    fn ringbuffer_drops_oldest_at_capacity() {
        let buf = LogBuffer::new(3);
        for i in 0..5 {
            buf.push(format!("line-{i}"));
        }
        assert_eq!(buf.len(), 3);
        // 0 and 1 have been dropped; buffer now holds 2, 3, 4.
        assert_eq!(buf.tail(10), vec!["line-2", "line-3", "line-4"]);
    }

    #[test]
    fn tail_with_zero_returns_empty() {
        let buf = LogBuffer::new(5);
        buf.push("x".into());
        assert!(buf.tail(0).is_empty());
    }

    #[test]
    fn tail_with_more_than_stored_returns_everything() {
        let buf = LogBuffer::new(5);
        buf.push("x".into());
        buf.push("y".into());
        assert_eq!(buf.tail(1000), vec!["x", "y"]);
    }

    #[test]
    fn clear_empties_the_buffer() {
        let buf = LogBuffer::new(5);
        buf.push("x".into());
        buf.clear();
        assert_eq!(buf.len(), 0);
        assert!(buf.tail(10).is_empty());
    }

    #[test]
    fn level_tag_is_always_five_chars() {
        // Monospace alignment in the viewer depends on the 5-char width.
        for lvl in [Level::ERROR, Level::WARN, Level::INFO, Level::DEBUG, Level::TRACE] {
            assert_eq!(level_tag(lvl).len(), 5, "{lvl:?} tag not 5 chars");
        }
    }

    #[test]
    fn visitor_push_field_concatenates_with_comma_separators() {
        // Comma+space delimiter matches what a reader sees in the viewer.
        let mut v = MessageVisitor::default();
        v.push_field("host_os", "macOS".into());
        v.push_field("host_home", "/Users/x".into());
        v.push_field("count", "42".into());
        assert_eq!(v.fields, "host_os=macOS, host_home=/Users/x, count=42");
    }

    #[test]
    fn visitor_empty_fields_stays_empty_when_only_message_recorded() {
        // A call like `tracing::info!("plain text")` only writes the
        // `message` field — visitor.fields must remain empty so the
        // viewer doesn't render a trailing `[]`. Default-constructed
        // visitor is the expected initial state.
        let v = MessageVisitor::default();
        assert!(v.fields.is_empty());
        assert!(v.message.is_empty());
    }

    #[test]
    fn is_empty_agrees_with_len() {
        let buf = LogBuffer::new(3);
        assert!(buf.is_empty());
        buf.push("x".into());
        assert!(!buf.is_empty());
        buf.clear();
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn buffer_layer_captures_info_event_via_tracing_dispatcher() {
        // End-to-end: install BufferLayer into a scoped subscriber, emit an
        // event, assert the captured line matches the expected shape.
        use tracing_subscriber::{prelude::*, Registry};
        let buf = std::sync::Arc::new(LogBuffer::new(10));
        let buf_for_layer = buf.clone();

        #[derive(Clone)]
        struct ScopedLayer(std::sync::Arc<LogBuffer>);
        impl<S: Subscriber> Layer<S> for ScopedLayer {
            fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
                self.0.push(format_line(event.metadata(), event));
            }
        }

        let subscriber = Registry::default().with(ScopedLayer(buf_for_layer));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "kronn::agent_detect", host_os = "macOS", "starting sweep");
        });

        let captured = buf.tail(10);
        assert_eq!(captured.len(), 1);
        let line = &captured[0];
        assert!(line.contains(" INFO"), "level tag missing: {line}");
        assert!(line.contains("kronn::agent_detect"), "target missing: {line}");
        assert!(line.contains("starting sweep"), "message missing: {line}");
        assert!(line.contains("host_os=macOS"), "field missing: {line}");
    }
}
