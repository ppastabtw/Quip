use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RECENT_EVENTS: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebugEventView {
    pub ts_ms: u64,
    pub event: String,
    pub summary: String,
    pub payload: Value,
}

pub struct DebugSink {
    include_text: bool,
    events_path: PathBuf,
    recent: VecDeque<DebugEventView>,
}

impl DebugSink {
    pub fn new(debug_dir: impl AsRef<Path>, include_text: bool) -> Self {
        let debug_dir = debug_dir.as_ref();
        let _ = std::fs::create_dir_all(debug_dir);
        Self {
            include_text,
            events_path: debug_dir.join("events.jsonl"),
            recent: VecDeque::new(),
        }
    }

    pub fn record(&mut self, event: &str, summary: impl Into<String>, payload: Value) {
        let view = DebugEventView {
            ts_ms: now_ms(),
            event: event.to_string(),
            summary: summary.into(),
            payload: if self.include_text {
                payload
            } else {
                redact_text_payloads(payload)
            },
        };
        self.recent.push_back(view.clone());
        while self.recent.len() > MAX_RECENT_EVENTS {
            self.recent.pop_front();
        }
        let _ = append_jsonl(&self.events_path, &view);
    }

    pub fn recent(&self, limit: usize) -> Vec<DebugEventView> {
        let start = self.recent.len().saturating_sub(limit);
        self.recent.iter().skip(start).cloned().collect()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn append_jsonl(path: &Path, event: &DebugEventView) -> Result<(), std::io::Error> {
    let mut file = open_append(path)?;
    let Ok(line) = serde_json::to_string(event) else {
        return Ok(());
    };
    writeln!(file, "{line}")
}

fn open_append(path: &Path) -> Result<File, std::io::Error> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn redact_text_payloads(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(redact_object(object)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_text_payloads).collect()),
        other => other,
    }
}

fn redact_object(object: Map<String, Value>) -> Map<String, Value> {
    object
        .into_iter()
        .filter_map(|(key, value)| {
            if is_debug_text_key(&key) {
                None
            } else {
                Some((key, redact_text_payloads(value)))
            }
        })
        .collect()
}

fn is_debug_text_key(key: &str) -> bool {
    matches!(
        key,
        "draft_text" | "candidate_text" | "committed_text" | "visible_text" | "candidates"
    ) || key.ends_with("_text")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "quip-debug-events-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn record_writes_jsonl_and_keeps_recent_events() {
        let dir = temp_dir("record");
        let mut sink = DebugSink::new(&dir, false);

        sink.record("capture_ready", "ready", json!({ "draft_chars": 11 }));

        let events = sink.recent(10);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "capture_ready");
        let raw = std::fs::read_to_string(dir.join("events.jsonl")).unwrap();
        assert!(raw.contains("\"capture_ready\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn text_payloads_are_redacted_by_default() {
        let dir = temp_dir("redacted");
        let mut sink = DebugSink::new(&dir, false);

        sink.record(
            "prediction_result",
            "five candidates",
            json!({
                "draft_text": "cnt cm tmrw",
                "draft_chars": 11,
                "candidates": ["Can't come tomorrow."],
                "nested": { "visible_text": "secret" }
            }),
        );

        let event = sink.recent(1).pop().unwrap();
        assert_eq!(event.payload["draft_chars"], 11);
        assert!(event.payload.get("draft_text").is_none());
        assert!(event.payload.get("candidates").is_none());
        assert!(event.payload["nested"].get("visible_text").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn text_payloads_can_be_included_explicitly() {
        let dir = temp_dir("included");
        let mut sink = DebugSink::new(&dir, true);

        sink.record(
            "prediction_result",
            "one candidate",
            json!({
                "draft_text": "cnt cm tmrw",
                "candidates": ["Can't come tomorrow."]
            }),
        );

        let event = sink.recent(1).pop().unwrap();
        assert_eq!(event.payload["draft_text"], "cnt cm tmrw");
        assert_eq!(event.payload["candidates"][0], "Can't come tomorrow.");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_is_bounded_to_the_requested_limit() {
        let dir = temp_dir("recent");
        let mut sink = DebugSink::new(&dir, false);
        for index in 0..5 {
            sink.record("event", format!("event {index}"), json!({ "index": index }));
        }

        let events = sink.recent(2);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].payload["index"], 3);
        assert_eq!(events[1].payload["index"], 4);
        let _ = std::fs::remove_dir_all(dir);
    }
}
