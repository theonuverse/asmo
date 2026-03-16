//! Dynamic HTTP router — resolves arbitrary field paths from [`SystemStats`].
//!
//! New fields added to [`SystemStats`] (or its nested types) are automatically
//! exposed as endpoints without any routing changes.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value;
use tokio::sync::watch;
use tracing::debug;

use crate::types::{AtomicHealth, ServiceStatus, SystemStats};

// ─── State ─────────────────────────────────────────────────────────────────

/// Shared application state — cloned cheaply per request via `Arc` internals.
#[derive(Clone)]
struct AppState {
    stats: watch::Receiver<SystemStats>,
    health: Arc<AtomicHealth>,
    svc_status: Arc<ServiceStatus>,
}

// ─── Router construction ───────────────────────────────────────────────────

/// Builds the Axum router with fully dynamic endpoint resolution.
///
/// # Routes
///
/// | Method | Path                          | Description                           |
/// |--------|-------------------------------|---------------------------------------|
/// | `GET`  | `/`                           | API index — lists every endpoint      |
/// | `GET`  | `/stats`                      | Full system stats snapshot            |
/// | `GET`  | `/health`                     | Monitor health status                 |
/// | `GET`  | `/debug`                      | Deep service diagnostics              |
/// | `GET`  | `/<field>`                    | Single top-level field                |
/// | `GET`  | `/<f1>,<f2>,…`                | Multiple fields in one request        |
/// | `GET`  | `/cores/<name>`               | Single core by name                   |
/// | `GET`  | `/cores/<name>/<field>`       | Single field of a specific core       |
/// | `GET`  | `/cores/<name>/<f1>,<f2>,…`   | Multiple core fields                  |
/// | `GET`  | `/cores/*/<field>`            | Field from every core (wildcard)      |
/// | `GET`  | `/cores/all/<f1>,<f2>,…`      | Multiple fields from every core       |
pub fn build(rx: watch::Receiver<SystemStats>, health: Arc<AtomicHealth>, svc_status: Arc<ServiceStatus>) -> Router {
    let state = AppState { stats: rx, health, svc_status };

    Router::new()
        .route("/", get(index))
        .route("/stats", get(stats))
        .route("/health", get(health_check))
        .route("/debug", get(debug_status))
        .route("/*path", get(resolve))
        .with_state(state)
}

// ─── Handlers ──────────────────────────────────────────────────────────────

/// `GET /` — Returns the API index with every available endpoint.
async fn index(State(state): State<AppState>) -> Json<Value> {
    let tree = stats_to_value(&state.stats.borrow());
    let mut endpoints = vec!["/stats".to_owned(), "/health".to_owned()];
    enumerate_endpoints(&tree, "", &mut endpoints);

    Json(serde_json::json!({
        "name": "asmo",
        "version": env!("CARGO_PKG_VERSION"),
        "endpoints": endpoints,
        "multi_field": "Combine fields with commas: /battery_level,cpu_temp,gpu_load",
        "wildcard": "Use * or 'all' for arrays: /cores/*/usage  /cores/all/usage,cur_freq",
        "usage": "GET any endpoint to retrieve its data."
    }))
}

/// `GET /stats` — Returns the full system stats snapshot.
async fn stats(State(state): State<AppState>) -> Json<SystemStats> {
    Json(state.stats.borrow().clone())
}

/// `GET /health` — Returns the current monitor health status.
async fn health_check(State(state): State<AppState>) -> Json<Value> {
    let health = state.health.load();
    Json(serde_json::json!({ "status": health }))
}

/// `GET /debug` — Deep service diagnostics: runtime counters, monitor state,
/// sysfs probe results, and configuration. Useful for troubleshooting without
/// needing to tail raw logs.
async fn debug_status(State(state): State<AppState>) -> Json<Value> {
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let svc = &state.svc_status;
    let health = state.health.load();
    let tick_count = svc.tick_count.load(Ordering::Relaxed);
    let last_tick = svc.last_tick_unix_secs.load(Ordering::Relaxed);
    let last_tick_age_secs: Option<u64> = if last_tick == 0 {
        None
    } else {
        Some(now_unix.saturating_sub(last_tick))
    };

    Json(serde_json::json!({
        "asmo_version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": now_unix.saturating_sub(svc.started_unix_secs),
        "bind_addr": &*svc.bind_addr,
        "poll_interval_ms": svc.poll_interval_ms,
        "core_count": svc.core_count,
        "monitor": {
            "health": health,
            "rish_retry_count": svc.rish_retry_count.load(Ordering::Relaxed),
            "rish_session_count": svc.rish_session_count.load(Ordering::Relaxed),
            "tick_count": tick_count,
            "last_tick_age_secs": last_tick_age_secs,
        },
        "sysfs": {
            "cpu_temp_path": &*svc.cpu_temp_path,
            "cpu_temp_ok": svc.cpu_temp_ok.load(Ordering::Relaxed),
            "gpu_temp_path": &*svc.gpu_temp_path,
            "gpu_temp_ok": svc.gpu_temp_ok.load(Ordering::Relaxed),
            "gpu_load_ok": svc.gpu_load_ok.load(Ordering::Relaxed),
        }
    }))
}

/// `GET /{path}` — Resolves an arbitrary path against the current stats.
///
/// Supports comma-separated fields in the last segment and wildcards (`*` / `all`)
/// for array expansion, e.g. `/cores/*/usage` or `/cores/all/usage,cur_freq`.
async fn resolve(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Response {
    let tree = stats_to_value(&state.stats.borrow());

    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    match resolve_request(&tree, &segments) {
        Some(v) => {
            debug!(path = %path, "request resolved");
            Json(v).into_response()
        }
        None => {
            debug!(path = %path, "request not found — 404");
            error_response(StatusCode::NOT_FOUND, "not found", &path)
        }
    }
}

/// Build a JSON error response with a hint pointing to the index.
fn error_response(status: StatusCode, message: &str, path: &str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": message,
            "path": format!("/{path}"),
            "hint": "GET / for available endpoints"
        })),
    )
        .into_response()
}

// ─── Path resolution ───────────────────────────────────────────────────────

/// Serialize [`SystemStats`] into a JSON value tree with clean `f32` precision.
///
/// `serde_json::to_value` promotes `f32` → `f64`, introducing artifacts like
/// `556.7999877929688` instead of `556.8`.  This function walks the tree after
/// conversion and casts every float back through `f32` to recover the short
/// representation.
fn stats_to_value(stats: &SystemStats) -> Value {
    let mut tree = serde_json::to_value(stats).unwrap_or_default();
    clean_f32_precision(&mut tree);
    tree
}

/// Recursively round every float in a JSON tree to `f32` precision.
fn clean_f32_precision(value: &mut Value) {
    match value {
        Value::Number(n) => {
            // Only touch floats — leave integers untouched.
            if n.as_u64().is_none()
                && n.as_i64().is_none()
                && let Some(f) = n.as_f64()
                && let Some(clean) = serde_json::Number::from_f64((f as f32) as f64)
            {
                *n = clean;
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(clean_f32_precision),
        Value::Object(map) => map.values_mut().for_each(clean_f32_precision),
        _ => {}
    }
}

/// Returns `true` for wildcard tokens (`*` and `all`).
fn is_wildcard(s: &str) -> bool {
    s == "*" || s == "all"
}

/// Navigate the JSON tree and return the **raw** value at the given path.
fn navigate(value: &Value, segments: &[&str]) -> Option<Value> {
    if segments.is_empty() {
        return Some(value.clone());
    }

    let key = segments[0];
    let rest = &segments[1..];

    match value {
        Value::Object(map) => navigate(map.get(key)?, rest),
        Value::Array(arr) => {
            let item = arr
                .iter()
                .find(|v| v.get("name").and_then(Value::as_str) == Some(key))?;
            navigate(item, rest)
        }
        _ => None,
    }
}

/// Fully resolve a request path.  Handles all query patterns:
///
/// - Single field:      `/battery_level`           → `{"battery_level": 100}`
/// - Comma fields:      `/cpu_temp,gpu_temp`       → `{"cpu_temp": 34.4, …}`
/// - Wildcard:          `/cores/*/usage`            → `[{"usage":…}, …]`
/// - Wildcard + commas: `/cores/all/usage,cur_freq` → `[{"usage":…,"cur_freq":…}, …]`
fn resolve_request(value: &Value, segments: &[&str]) -> Option<Value> {
    if segments.is_empty() {
        return Some(value.clone());
    }

    let current = segments[0];
    let rest = &segments[1..];
    let is_last = rest.is_empty();

    // ── Comma-separated fields (last segment only) ──────────────────────
    if is_last && current.contains(',') {
        return resolve_comma_fields(value, current);
    }

    // ── Wildcard: expand over every item in an array ────────────────────
    if is_wildcard(current) {
        let Value::Array(arr) = value else { return None };
        let results: Vec<Value> = arr
            .iter()
            .filter_map(|item| {
                if is_last {
                    return Some(item.clone());
                }
                resolve_request(item, rest)
            })
            .collect();
        return if results.is_empty() { None } else { Some(Value::Array(results)) };
    }

    // ── Standard navigation ─────────────────────────────────────────────
    match value {
        Value::Object(map) => {
            let child = map.get(current)?;
            if is_last {
                Some(serde_json::json!({ current: child }))
            } else {
                resolve_request(child, rest)
            }
        }
        Value::Array(arr) => {
            let item = arr
                .iter()
                .find(|v| v.get("name").and_then(Value::as_str) == Some(current))?;
            if is_last {
                Some(item.clone())
            } else {
                resolve_request(item, rest)
            }
        }
        _ => None,
    }
}

/// Extract comma-separated fields from a value.
fn resolve_comma_fields(value: &Value, raw: &str) -> Option<Value> {
    let mut result = serde_json::Map::new();
    for field in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(val) = navigate(value, &[field]) {
            result.insert(field.to_string(), val);
        }
    }
    if result.is_empty() { None } else { Some(Value::Object(result)) }
}

// ─── Endpoint enumeration ──────────────────────────────────────────────────

/// Recursively discovers every addressable path in a JSON value tree.
fn enumerate_endpoints(value: &Value, prefix: &str, out: &mut Vec<String>) {
    let Value::Object(map) = value else { return };

    for (key, child) in map {
        let path = format!("{prefix}/{key}");
        out.push(path.clone());

        match child {
            Value::Object(_) => enumerate_endpoints(child, &path, out),
            Value::Array(arr) => {
                for item in arr {
                    let Some(name) = item.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    let item_path = format!("{path}/{name}");
                    out.push(item_path.clone());

                    if let Value::Object(fields) = item {
                        for field_key in fields.keys().filter(|k| k.as_str() != "name") {
                            out.push(format!("{item_path}/{field_key}"));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_f32_precision_roundtrips_f32() {
        // Values originating from f32 should survive the f32 round-trip unchanged.
        let original = 556.8_f32;
        let promoted = original as f64; // what serde_json does internally
        let mut val = serde_json::json!(promoted);
        clean_f32_precision(&mut val);
        assert_eq!(val.as_f64().unwrap(), promoted);
    }

    #[test]
    fn clean_f32_precision_preserves_integers() {
        let mut val = serde_json::json!(42);
        clean_f32_precision(&mut val);
        assert_eq!(val, serde_json::json!(42));
    }

    #[test]
    fn clean_f32_precision_walks_nested() {
        let mut val = serde_json::json!({"a": [1.100000023841858, 2]});
        clean_f32_precision(&mut val);
        assert_eq!(val, serde_json::json!({"a": [1.100000023841858_f64 as f32 as f64, 2]}));
    }

    #[test]
    fn wildcard_detection() {
        assert!(is_wildcard("*"));
        assert!(is_wildcard("all"));
        assert!(!is_wildcard("cpu0"));
        assert!(!is_wildcard(""));
    }

    #[test]
    fn resolve_single_field() {
        let tree = serde_json::json!({"battery_level": 85, "cpu_temp": 42.0});
        let result = resolve_request(&tree, &["battery_level"]);
        assert_eq!(result, Some(serde_json::json!({"battery_level": 85})));
    }

    #[test]
    fn resolve_comma_fields_extracts_multiple() {
        let tree = serde_json::json!({"a": 1, "b": 2, "c": 3});
        let result = resolve_comma_fields(&tree, "a,c");
        assert_eq!(result, Some(serde_json::json!({"a": 1, "c": 3})));
    }

    #[test]
    fn resolve_comma_fields_none_for_missing() {
        let tree = serde_json::json!({"a": 1});
        assert_eq!(resolve_comma_fields(&tree, "x,y"), None);
    }

    #[test]
    fn resolve_not_found_returns_none() {
        let tree = serde_json::json!({"a": 1});
        assert_eq!(resolve_request(&tree, &["nonexistent"]), None);
    }

    #[test]
    fn resolve_wildcard_collects_field() {
        let tree = serde_json::json!({
            "cores": [
                {"name": "cpu0", "usage": 10.0},
                {"name": "cpu1", "usage": 20.0}
            ]
        });
        let result = resolve_request(&tree, &["cores", "*", "usage"]).unwrap();
        assert!(result.is_array());
        assert_eq!(
            result,
            serde_json::json!([{"usage": 10.0}, {"usage": 20.0}])
        );
    }

    #[test]
    fn resolve_wildcard_all_items() {
        let tree = serde_json::json!({
            "items": [
                {"name": "a", "val": 1},
                {"name": "b", "val": 2}
            ]
        });
        let result = resolve_request(&tree, &["items", "all"]).unwrap();
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 2);
    }

    #[test]
    fn navigate_nested_path() {
        let tree = serde_json::json!({"a": {"b": {"c": 42}}});
        assert_eq!(navigate(&tree, &["a", "b", "c"]), Some(serde_json::json!(42)));
    }

    #[test]
    fn navigate_array_by_name() {
        let tree = serde_json::json!([
            {"name": "cpu0", "usage": 10.0},
            {"name": "cpu1", "usage": 20.0}
        ]);
        let result = navigate(&tree, &["cpu1", "usage"]);
        assert_eq!(result, Some(serde_json::json!(20.0)));
    }

    #[test]
    fn enumerate_endpoints_discovers_all() {
        let tree = serde_json::json!({
            "battery_level": 100,
            "cpu_temp": 42.0,
        });
        let mut out = vec![];
        enumerate_endpoints(&tree, "", &mut out);
        assert!(out.contains(&"/battery_level".to_owned()));
        assert!(out.contains(&"/cpu_temp".to_owned()));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn enumerate_endpoints_with_array() {
        let tree = serde_json::json!({
            "cores": [
                {"name": "cpu0", "usage": 10.0},
            ]
        });
        let mut out = vec![];
        enumerate_endpoints(&tree, "", &mut out);
        assert!(out.contains(&"/cores".to_owned()));
        assert!(out.contains(&"/cores/cpu0".to_owned()));
        assert!(out.contains(&"/cores/cpu0/usage".to_owned()));
    }

    #[test]
    fn resolve_handles_null_sensor_values() {
        let tree = serde_json::json!({"cpu_temp": null, "battery_level": 85});
        let result = resolve_request(&tree, &["cpu_temp"]);
        assert_eq!(result, Some(serde_json::json!({"cpu_temp": null})));
    }
}