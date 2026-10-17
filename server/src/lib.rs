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
const MAX_REPORT_SAMPLES: usize = 50;
const MIN_DISPLAY_SAMPLES: usize = 5;
const DISPLAY_DEDUP_SECONDS: i64 = 30;
const MIN_VALID_WEIGHT: u16 = 3_500;
const MAX_STABLE_SPAN: u16 = 100;
const MAX_RECENT_WEIGHT_CHANGE: u16 = 1_000;
const RECENT_MEASUREMENT_SECONDS: i64 = 60 * 24 * 60 * 60;
static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Storage {
    reports_dir: PathBuf,
    database_path: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SampleReport {
    #[serde(with = "time::serde::timestamp")]
    timestamp: OffsetDateTime,
    samples: Vec<u16>,
}

#[derive(Debug, Deserialize)]
struct ArchivedReport {
    #[serde(with = "time::serde::timestamp")]
    timestamp: OffsetDateTime,
    #[serde(default)]
    samples: Vec<u16>,
    #[serde(default)]
    weight: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    #[serde(with = "time::serde::timestamp")]
    pub timestamp: OffsetDateTime,
    pub weight: u32,
}

struct CalculatedReport {
    timestamp: OffsetDateTime,
    weight: Option<u32>,
    samples: Vec<u16>,
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
        storage.rebuild_database()?;

        Ok(storage)
    }

    fn save(&self, report: &SampleReport) -> Result<(), StorageError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| StorageError::WriteLockPoisoned)?;
        self.write_report_file(report)?;
        self.rebuild_database()
    }

    pub fn list(&self) -> Result<Vec<Report>, StorageError> {
        let connection = self.connect()?;
        let mut statement = connection
            .prepare(
                "SELECT timestamp, weight, samples
                 FROM reports
                 WHERE weight IS NOT NULL
                 ORDER BY timestamp",
            )?;
        let stored_reports = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let reports = stored_reports
            .into_iter()
            .map(|(timestamp, weight, samples)| {
                let timestamp = OffsetDateTime::from_unix_timestamp(timestamp)
                    .map_err(|_| StorageError::InvalidTimestamp(timestamp))?;
                let weight =
                    u32::try_from(weight).map_err(|_| StorageError::InvalidWeight(weight))?;
                let samples: Vec<u16> = serde_json::from_str(&samples)?;
                Ok((Report { timestamp, weight }, samples.len()))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;

        Ok(reports_for_display(reports))
    }

    fn connect(&self) -> Result<Connection, StorageError> {
        let connection = Connection::open(&self.database_path)?;
        connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
        Ok(connection)
    }

    fn rebuild_database(&self) -> Result<(), StorageError> {
        let reports = calculate_reports(self.read_report_files()?);

        let mut connection = self.connect()?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;",
        )?;
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "DROP TABLE IF EXISTS reports;
             CREATE TABLE reports (
                 timestamp INTEGER PRIMARY KEY,
                 weight INTEGER CHECK (weight IS NULL OR weight BETWEEN 0 AND 4294967295),
                 samples TEXT NOT NULL
             ) STRICT;",
        )?;

        for report in reports {
            let samples = serde_json::to_string(&report.samples)?;
            transaction.execute(
                "INSERT INTO reports (timestamp, weight, samples) VALUES (?1, ?2, ?3)",
                params![
                    report.timestamp.unix_timestamp(),
                    report.weight.map(i64::from),
                    samples
                ],
            )?;
        }

        transaction.commit()?;
        Ok(())
    }

    fn read_report_files(&self) -> Result<Vec<ArchivedReport>, StorageError> {
        let mut reports = Vec::new();

        for entry in fs::read_dir(&self.reports_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }

            let contents = fs::read(path)?;
            reports.push(serde_json::from_slice(&contents)?);
        }

        Ok(reports)
    }

    fn write_report_file(&self, report: &SampleReport) -> Result<(), StorageError> {
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

fn calculate_reports(mut reports: Vec<ArchivedReport>) -> Vec<CalculatedReport> {
    reports.sort_by_key(|report| report.timestamp);
    let mut last_accepted: Option<(i64, u16)> = None;

    reports
        .into_iter()
        .map(|report| {
            let can_establish_baseline =
                report.samples.is_empty() || report.samples.len() >= MIN_DISPLAY_SAMPLES;
            let candidate = if report.samples.is_empty() {
                report
                    .weight
                    .and_then(|weight| u16::try_from(weight).ok())
                    .filter(|weight| *weight >= MIN_VALID_WEIGHT)
            } else {
                calculate_weight(&report.samples)
            };

            let weight = candidate.filter(|candidate| {
                let Some((last_timestamp, last_weight)) = last_accepted else {
                    return true;
                };
                let age = report.timestamp.unix_timestamp() - last_timestamp;
                age > RECENT_MEASUREMENT_SECONDS
                    || candidate.abs_diff(last_weight) < MAX_RECENT_WEIGHT_CHANGE
            });

            if let Some(weight) = weight.filter(|_| can_establish_baseline) {
                last_accepted = Some((report.timestamp.unix_timestamp(), weight));
            }

            CalculatedReport {
                timestamp: report.timestamp,
                weight: weight.map(u32::from),
                samples: report.samples,
            }
        })
        .collect()
}

fn calculate_weight(samples: &[u16]) -> Option<u16> {
    let mut sorted = samples
        .iter()
        .copied()
        .filter(|weight| *weight >= MIN_VALID_WEIGHT)
        .collect::<Vec<_>>();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_unstable();

    let mut window_start = 0;
    let mut best_start = 0;
    let mut best_end = 1;

    for window_end in 0..sorted.len() {
        while sorted[window_end] - sorted[window_start] > MAX_STABLE_SPAN {
            window_start += 1;
        }

        let window_len = window_end + 1 - window_start;
        let best_len = best_end - best_start;
        let window_span = sorted[window_end] - sorted[window_start];
        let best_span = sorted[best_end - 1] - sorted[best_start];
        let equally_dense_but_better = window_len == best_len
            && (window_span < best_span
                || (window_span == best_span
                    && sorted[window_end] > sorted[best_end - 1]));

        if window_len > best_len || equally_dense_but_better {
            best_start = window_start;
            best_end = window_end + 1;
        }
    }

    let stable = &sorted[best_start..best_end];
    let middle = stable.len() / 2;

    if stable.len() % 2 == 0 {
        let sum = u32::from(stable[middle - 1]) + u32::from(stable[middle]);
        Some(sum.div_ceil(2) as u16)
    } else {
        Some(stable[middle])
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
    Json(report): Json<SampleReport>,
) -> Result<StatusCode, AppError> {
    if report.samples.is_empty() {
        return Err(AppError::invalid_report("samples must not be empty"));
    }
    if report.samples.len() > MAX_REPORT_SAMPLES {
        return Err(AppError::invalid_report("too many samples"));
    }

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

fn reports_for_display(reports: Vec<(Report, usize)>) -> Vec<Report> {
    let mut last_displayed_at = None;

    reports
        .into_iter()
        .filter_map(|(report, sample_count)| {
            // Empty sample lists belong to reports created before samples were
            // recorded. Keep those historical measurements visible.
            if sample_count != 0 && sample_count < MIN_DISPLAY_SAMPLES {
                return None;
            }

            let timestamp = report.timestamp.unix_timestamp();
            if last_displayed_at
                .is_some_and(|previous| timestamp - previous < DISPLAY_DEDUP_SECONDS)
            {
                return None;
            }

            last_displayed_at = Some(timestamp);
            Some(report)
        })
        .collect()
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
struct AppError {
    status: StatusCode,
    message: &'static str,
}

impl AppError {
    fn new(context: &'static str, source: impl fmt::Display) -> Self {
        error!(%source, "{context}");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error",
        }
    }

    fn invalid_report(message: &'static str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (self.status, self.message).into_response()
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
    use rusqlite::Connection;
    use tempfile::tempdir;
    use time::OffsetDateTime;
    use tower::ServiceExt;

    use super::{
        ArchivedReport, Report, Storage, calculate_reports, calculate_weight,
        reports_for_display, router,
    };

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
                    .body(Body::from(
                        r#"{"timestamp":1760616000,"samples":[8084,8085,8085,8086,8085]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            fs::read_to_string(data_dir.path().join("reports/1760616000.json")).unwrap(),
            "{\"timestamp\":1760616000,\"samples\":[8084,8085,8085,8086,8085]}\n"
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
                            r#"{{"timestamp":1760616000,"samples":[{weight},{weight},{weight},{weight},{weight}]}}"#
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
            "{\"timestamp\":1760616000,\"samples\":[8086,8086,8086,8086,8086]}\n"
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
                        r#"{"timestamp":999999999999999999,"samples":[8085]}"#,
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
    async fn rejects_more_than_fifty_samples() {
        let data_dir = tempdir().unwrap();
        let app = router(Storage::open(data_dir.path()).unwrap());
        let samples = vec!["8085"; 51].join(",");

        let response = app
            .oneshot(
                Request::post("/api/report")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"timestamp":1760616000,"samples":[{samples}]}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn startup_rebuilds_the_database_from_report_files() {
        let data_dir = tempdir().unwrap();
        let reports_dir = data_dir.path().join("reports");
        fs::create_dir(&reports_dir).unwrap();
        fs::write(
            reports_dir.join("1760616000.json"),
            r#"{"timestamp":1760616000,"weight":30,"samples":[7960,7960,7960,7960,330,195,0,75,195,295,220,295,30,25,20,30,45,50,50,0]}"#,
        )
        .unwrap();
        let database_path = data_dir.path().join("reports.sqlite");
        let connection = Connection::open(&database_path).unwrap();
        connection
            .execute_batch("CREATE TABLE stale (value INTEGER) STRICT;")
            .unwrap();
        drop(connection);

        let reports = Storage::open(data_dir.path()).unwrap().list().unwrap();

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].weight, 7_960);
    }

    #[test]
    fn display_reports_filters_short_and_duplicate_sessions() {
        let report = |timestamp| Report {
            timestamp: OffsetDateTime::from_unix_timestamp(timestamp).unwrap(),
            weight: 8_085,
        };
        let reports = vec![
            (report(100), 5),
            (report(110), 1),
            (report(120), 5),
            (report(130), 5),
            (report(200), 0),
        ];

        let displayed = reports_for_display(reports);
        let timestamps = displayed
            .iter()
            .map(|report| report.timestamp.unix_timestamp())
            .collect::<Vec<_>>();

        assert_eq!(timestamps, [100, 130, 200]);
    }

    #[test]
    fn weight_uses_the_valid_stable_cluster() {
        let samples = [
            7_960, 7_960, 7_960, 7_960, 330, 195, 0, 75, 195, 295, 220, 295, 30, 25, 20,
            30, 45, 50, 50, 0,
        ];

        assert_eq!(calculate_weight(&samples), Some(7_960));
        assert_eq!(calculate_weight(&[3_499, 100, 0]), None);
    }

    #[test]
    fn recent_ten_kilogram_outlier_is_ignored() {
        let report = |timestamp, weight| ArchivedReport {
            timestamp: OffsetDateTime::from_unix_timestamp(timestamp).unwrap(),
            samples: vec![weight; 5],
            weight: None,
        };
        let day = 24 * 60 * 60;
        let calculated = calculate_reports(vec![
            report(0, 8_000),
            report(day, 9_000),
            report(61 * day, 9_000),
        ]);

        assert_eq!(calculated[0].weight, Some(8_000));
        assert_eq!(calculated[1].weight, None);
        assert_eq!(calculated[2].weight, Some(9_000));
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
