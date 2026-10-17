use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Response, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::error;

const INDEX_HTML: &str = include_str!("../assets/index.html");
const APP_CSS: &str = include_str!("../assets/app.css");
const APP_JS: &str = include_str!("../assets/app.js");
const UPLOT_CSS: &str = include_str!("../assets/vendor/uPlot.min.css");
const UPLOT_JS: &str = include_str!("../assets/vendor/uPlot.iife.min.js");
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'";
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Storage {
    reports_dir: PathBuf,
    database_path: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Report {
    #[serde(with = "time::serde::timestamp")]
    pub timestamp: OffsetDateTime,
    pub weight: u32,
}

impl Storage {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        let data_dir = data_dir.as_ref();
        let storage = Self {
            reports_dir: data_dir.join("reports"),
            database_path: data_dir.join("reports.sqlite"),
            write_lock: Arc::new(Mutex::new(())),
        };

        fs::create_dir_all(&storage.reports_dir)?;
        let connection = storage.connect()?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS reports (
                 timestamp INTEGER PRIMARY KEY,
                 weight INTEGER NOT NULL CHECK (weight BETWEEN 0 AND 4294967295)
             ) STRICT;",
        )?;

        Ok(storage)
    }

    pub fn save(&self, report: &Report) -> Result<(), StorageError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| StorageError::WriteLockPoisoned)?;
        self.write_report_file(report)?;

        let connection = self.connect()?;
        connection.execute(
            "INSERT INTO reports (timestamp, weight)
             VALUES (?1, ?2)
             ON CONFLICT(timestamp) DO UPDATE SET weight = excluded.weight",
            params![report.timestamp.unix_timestamp(), i64::from(report.weight)],
        )?;

        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Report>, StorageError> {
        let connection = self.connect()?;
        let mut statement =
            connection.prepare("SELECT timestamp, weight FROM reports ORDER BY timestamp")?;
        let stored_reports = statement
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;

        stored_reports
            .into_iter()
            .map(|(timestamp, weight)| {
                let timestamp = OffsetDateTime::from_unix_timestamp(timestamp)
                    .map_err(|_| StorageError::InvalidTimestamp(timestamp))?;
                let weight =
                    u32::try_from(weight).map_err(|_| StorageError::InvalidWeight(weight))?;
                Ok(Report { timestamp, weight })
            })
            .collect()
    }

    fn connect(&self) -> Result<Connection, StorageError> {
        let connection = Connection::open(&self.database_path)?;
        connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
        Ok(connection)
    }

    fn write_report_file(&self, report: &Report) -> Result<(), StorageError> {
        let timestamp = report.timestamp.unix_timestamp();
        let destination = self.reports_dir.join(format!("{timestamp}.json"));
        let temp_id = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let temporary =
            self.reports_dir
                .join(format!(".{timestamp}.{}.{}.tmp", process::id(), temp_id));
        let mut json = serde_json::to_vec(report)?;
        json.push(b'\n');

        let result = (|| -> Result<(), std::io::Error> {
            let mut file = create_new_file(&temporary)?;
            file.write_all(&json)?;
            file.sync_all()?;
            fs::rename(&temporary, &destination)?;
            Ok(())
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }

        result.map_err(StorageError::Io)
    }
}

fn create_new_file(path: &Path) -> Result<File, std::io::Error> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

pub fn router(storage: Storage) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/report", post(create_report))
        .route("/api/reports", get(list_reports))
        .route("/assets/app.css", get(app_css))
        .route("/assets/app.js", get(app_js))
        .route("/assets/uPlot.min.css", get(uplot_css))
        .route("/assets/uPlot.iife.min.js", get(uplot_js))
        .fallback(not_found)
        .with_state(storage)
}

async fn create_report(
    State(storage): State<Storage>,
    Json(report): Json<Report>,
) -> Result<StatusCode, AppError> {
    tokio::task::spawn_blocking(move || storage.save(&report))
        .await
        .map_err(|join_error| AppError::new("persistence task failed", join_error))?
        .map_err(|storage_error| AppError::new("could not persist report", storage_error))?;

    Ok(StatusCode::CREATED)
}

async fn list_reports(State(storage): State<Storage>) -> Result<Json<Vec<Report>>, AppError> {
    let reports = tokio::task::spawn_blocking(move || storage.list())
        .await
        .map_err(|join_error| AppError::new("database task failed", join_error))?
        .map_err(|storage_error| AppError::new("could not load reports", storage_error))?;

    Ok(Json(reports))
}

async fn index() -> Response<Body> {
    embedded_response("text/html; charset=utf-8", INDEX_HTML, "no-cache")
}

async fn app_css() -> Response<Body> {
    embedded_response("text/css; charset=utf-8", APP_CSS, "no-cache")
}

async fn app_js() -> Response<Body> {
    embedded_response("text/javascript; charset=utf-8", APP_JS, "no-cache")
}

async fn uplot_css() -> Response<Body> {
    embedded_response(
        "text/css; charset=utf-8",
        UPLOT_CSS,
        "public, max-age=31536000, immutable",
    )
}

async fn uplot_js() -> Response<Body> {
    embedded_response(
        "text/javascript; charset=utf-8",
        UPLOT_JS,
        "public, max-age=31536000, immutable",
    )
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

fn embedded_response(
    content_type: &'static str,
    body: &'static str,
    cache_control: &'static str,
) -> Response<Body> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header("content-security-policy", CONTENT_SECURITY_POLICY)
        .body(Body::from(body))
        .expect("static response headers are valid")
}

#[derive(Debug)]
struct AppError;

impl AppError {
    fn new(context: &'static str, source: impl fmt::Display) -> Self {
        error!(%source, "{context}");
        Self
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
    }
}

#[derive(Debug)]
pub enum StorageError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Sqlite(rusqlite::Error),
    InvalidTimestamp(i64),
    InvalidWeight(i64),
    WriteLockPoisoned,
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "filesystem error: {error}"),
            Self::Json(error) => write!(formatter, "JSON error: {error}"),
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::InvalidTimestamp(timestamp) => {
                write!(formatter, "database contains invalid timestamp {timestamp}")
            }
            Self::InvalidWeight(weight) => {
                write!(formatter, "database contains invalid weight {weight}")
            }
            Self::WriteLockPoisoned => formatter.write_str("report write lock is poisoned"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::InvalidTimestamp(_) | Self::InvalidWeight(_) | Self::WriteLockPoisoned => None,
        }
    }
}

impl From<std::io::Error> for StorageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use tempfile::tempdir;
    use tower::ServiceExt;

    use super::{Report, Storage, router};

    #[tokio::test]
    async fn posting_a_report_persists_and_lists_it() {
        let data_dir = tempdir().unwrap();
        let storage = Storage::open(data_dir.path()).unwrap();
        let app = router(storage.clone());

        let response = app
            .clone()
            .oneshot(
                Request::post("/api/report")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"timestamp":1760616000,"weight":8085}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            fs::read_to_string(data_dir.path().join("reports/1760616000.json")).unwrap(),
            "{\"timestamp\":1760616000,\"weight\":8085}\n"
        );

        let response = app
            .oneshot(Request::get("/api/reports").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let reports: Vec<Report> = serde_json::from_slice(&body).unwrap();

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].timestamp.unix_timestamp(), 1_760_616_000);
        assert_eq!(reports[0].weight, 8_085);
        assert_eq!(storage.list().unwrap(), reports);
    }

    #[tokio::test]
    async fn retrying_a_timestamp_updates_both_copies() {
        let data_dir = tempdir().unwrap();
        let storage = Storage::open(data_dir.path()).unwrap();
        let app = router(storage.clone());

        for weight in [8_085, 8_086] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/api/report")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(format!(
                            r#"{{"timestamp":1760616000,"weight":{weight}}}"#
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        assert_eq!(storage.list().unwrap()[0].weight, 8_086);
        assert_eq!(
            fs::read_to_string(data_dir.path().join("reports/1760616000.json")).unwrap(),
            "{\"timestamp\":1760616000,\"weight\":8086}\n"
        );
    }

    #[tokio::test]
    async fn invalid_timestamp_is_rejected_before_persistence() {
        let data_dir = tempdir().unwrap();
        let storage = Storage::open(data_dir.path()).unwrap();
        let app = router(storage.clone());

        let response = app
            .oneshot(
                Request::post("/api/report")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"timestamp":999999999999999999,"weight":8085}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(storage.list().unwrap().is_empty());
        assert_eq!(
            fs::read_dir(data_dir.path().join("reports"))
                .unwrap()
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn ui_and_assets_are_embedded() {
        let data_dir = tempdir().unwrap();
        let app = router(Storage::open(data_dir.path()).unwrap());

        for (path, content_type) in [
            ("/", "text/html; charset=utf-8"),
            ("/assets/app.css", "text/css; charset=utf-8"),
            ("/assets/app.js", "text/javascript; charset=utf-8"),
            ("/assets/uPlot.min.css", "text/css; charset=utf-8"),
            (
                "/assets/uPlot.iife.min.js",
                "text/javascript; charset=utf-8",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
            assert!(
                !to_bytes(response.into_body(), 100_000)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
