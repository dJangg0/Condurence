//! Event egress for the runtime. The app routes events to the Tauri event bus; tests
//! record them instead. Keeping this behind a trait lets the orchestration path run
//! headless under `cargo test`.

use serde_json::Value;

pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

impl<R: tauri::Runtime> EventSink for tauri::AppHandle<R> {
    fn emit(&self, event: &str, payload: Value) {
        let _ = tauri::Emitter::emit(self, event, payload);
    }
}
