//! Legacy deployment health contract: HTTP 200 even when degraded. Readiness
//! remains /health/ready (503 when not ready), not a lifecycle action.
use crate::auth::AuthState;
use axum::{Json, Router, extract::State, routing::get};
use serde_json::{Value, json};
use yoink_application::SystemService;

#[derive(Clone)]
struct HealthState {
    system: SystemService,
    identities: Option<AuthState>,
}

pub(crate) fn router(system: SystemService, identities: Option<AuthState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(HealthState { system, identities })
}

async fn health(State(state): State<HealthState>) -> Json<Value> {
    let start = std::time::Instant::now();
    let identities = state.identities.as_ref().and_then(|state| {
        state
            .identities()
            .ok()
            .or_else(|| state.service().ok().map(|service| &service.identities))
    });
    let disk_directory =
        identities.and_then(|identities| identities.configuration_health_directory());
    let db = match identities {
        Some(identities) => match identities.configuration_health().await {
            Ok(()) => {
                json!({"status":"ok", "latency_ms":(start.elapsed().as_secs_f64()*10000.0).round()/10.0})
            }
            Err(_) => json!({"status":"error", "message":"database probe failed"}),
        },
        None => json!({"status":"error", "message":"no database health probe configured"}),
    };
    let bot = if state.system.bot_running() {
        match state.system.bot_username() {
            Some(username) => {
                json!({"status":"ok", "username":format!("@{}", username.trim_start_matches('@'))})
            }
            None => json!({"status":"ok", "message":"bot present, no cached info"}),
        }
    } else {
        json!({"status":"ok", "message":"standalone API mode"})
    };
    let disk = match disk_directory {
        Some(directory) => {
            disk_task_result(tokio::task::spawn_blocking(move || disk_check(&directory)).await)
        }
        None => json!({"status":"error", "message":"no database disk probe configured"}),
    };
    let status = if [&db, &bot, &disk].iter().all(|v| v["status"] == "ok") {
        "ok"
    } else {
        "degraded"
    };
    Json(json!({"status":status, "checks":{"db":db,"bot":bot,"disk":disk}}))
}

fn disk_task_result(result: Result<Value, tokio::task::JoinError>) -> Value {
    result.unwrap_or_else(|_| json!({"status":"error", "message":"disk health probe task failed"}))
}

fn disk_check(path: &std::path::Path) -> Value {
    match rustix::fs::statvfs(path) {
        Ok(stats) if stats.f_blocks > 0 => {
            let total = stats.f_blocks as f64 * stats.f_frsize as f64;
            let free = stats.f_bavail as f64 * stats.f_frsize as f64;
            let used = (total - stats.f_bfree as f64 * stats.f_frsize as f64) / total * 100.0;
            json!({"status":if used < 90.0 { "ok" } else { "warning" }, "free_gb":(free/1073741824.0*100.0).round()/100.0,"total_gb":(total/1073741824.0*100.0).round()/100.0,"used_pct":(used*10.0).round()/10.0})
        }
        _ => json!({"status":"error", "message":"disk usage unavailable or unsupported"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn failed_blocking_task_becomes_sanitized_diagnostic() {
        let task = tokio::task::spawn_blocking(|| -> Value { panic!("private fixture detail") });
        let diagnostic = disk_task_result(task.await);
        assert_eq!(diagnostic["status"], "error");
        assert_eq!(diagnostic["message"], "disk health probe task failed");
        assert!(!diagnostic.to_string().contains("private fixture detail"));
    }

    #[tokio::test]
    async fn no_database_health_is_public_200_but_readiness_remains_503() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;
        let app = crate::router(SystemService::new("health-fixture"));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let health: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(health["status"], "degraded");
        assert_eq!(
            health["checks"]["db"]["message"],
            "no database health probe configured"
        );
        assert_eq!(
            health["checks"]["disk"]["message"],
            "no database disk probe configured"
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn disk_probe_is_read_only_and_reports_missing_path() {
        let directory = tempfile::tempdir().unwrap();
        assert!(disk_check(directory.path())["free_gb"].is_number());
        let diagnostic = disk_check(std::path::Path::new("/nonexistent/yoink-health-fixture"));
        assert_eq!(diagnostic["status"], "error");
        assert_eq!(
            diagnostic["message"],
            "disk usage unavailable or unsupported"
        );
        assert!(!diagnostic.to_string().contains("yoink-health-fixture"));
    }
}
