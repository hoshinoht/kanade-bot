//! CSP report sink. Accepts both `report-uri` (application/csp-report) and
//! Reporting API (application/reports+json) bodies, logs them, and keeps a
//! bounded in-memory list the e2e suite reads through `/__mock/reports`.

use axum::{Json, body::Bytes, extract::State, http::StatusCode};
use serde_json::Value;
use std::sync::{Arc, Mutex};

const MAX_REPORTS: usize = 500;

#[derive(Clone, Default)]
pub struct Log(Arc<Mutex<Vec<Value>>>);

pub async fn receive(State(app): State<super::App>, body: Bytes) -> StatusCode {
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let items = match value {
        Value::Array(items) => items,
        other => vec![other],
    };
    let mut log = app.reports.0.lock().unwrap_or_else(|e| e.into_inner());
    for item in items {
        eprintln!("csp-report {item}");
        if log.len() < MAX_REPORTS {
            log.push(item);
        }
    }
    StatusCode::NO_CONTENT
}

pub async fn list(State(app): State<super::App>) -> Json<Vec<Value>> {
    Json(
        app.reports
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
    )
}

pub async fn clear(State(app): State<super::App>) -> StatusCode {
    app.reports
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    StatusCode::NO_CONTENT
}
