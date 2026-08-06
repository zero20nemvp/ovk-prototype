use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;

use crate::config::{Config, Runner};
use crate::flow::FlowClient;
use crate::i18n::{t, Lang, T};
use crate::store::{Run, Store};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub flow: Arc<FlowClient>,
    pub cfg: Arc<Config>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/runs", post(submit))
        .route("/runs/:item", get(run_page))
        .route("/runs/:item/notes", post(save_notes))
        .route("/runs/:item/outcome", post(record_outcome))
        .route("/personas/:id/lock", post(toggle_lock))
        .route("/fragments/runs", get(runs_fragment))
        .route("/fragments/runs/:item", get(run_fragment))
        .route("/fragments/calibration", get(calibration_fragment))
        .route("/static/htmx.min.js", get(htmx_js))
        .route("/mockup", get(static_mockup))
        .with_state(state)
}

// ── language resolution (query param → cookie → EN) ─────────────────────────

#[derive(Deserialize)]
struct LangQuery {
    lang: Option<String>,
}

fn resolve_lang(q: &LangQuery, headers: &HeaderMap) -> Lang {
    if let Some(l) = q.lang.as_deref().and_then(Lang::from_code) {
        return l;
    }
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|c| {
                let (k, v) = c.trim().split_once('=')?;
                (k == "lang").then(|| Lang::from_code(v)).flatten()
            })
        })
        .unwrap_or(Lang::En)
}

/// Full pages persist the language in a cookie so fragments inherit it.
fn with_lang_cookie(lang: Lang, resp: Response) -> Response {
    let mut resp = resp;
    if let Ok(v) = format!("lang={}; Path=/; Max-Age=31536000", lang.code()).parse() {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    resp
}

// ── templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    t: &'static T,
    queue: String,
    runner: String,
    engine_default: String,
}

#[derive(Template)]
#[template(path = "run_page.html")]
struct RunPageTemplate {
    t: &'static T,
    item: u64,
}

#[derive(Template)]
#[template(path = "runs_list.html")]
struct RunsListTemplate {
    t: &'static T,
    runs: Vec<RunCard>,
}

struct RunCard {
    item: u64,
    title: String,
    status: String,
    reply_count: u64,
    fav: String,
    verdict: String,
    verdict_class: String,
    has_outcome: bool,
}

#[derive(Template)]
#[template(path = "run_detail.html")]
struct RunDetailTemplate {
    t: &'static T,
    item: u64,
    pending: bool,
    draft: String,
    reply_count: u64,
    replies: Vec<ReplyView>,
    prediction: Option<PredView>,
    engine_used: String,
    outcome: Option<OutcomeView>,
    outcome_error: String,
    notes: String,
    saved: bool,
}

struct ReplyView {
    persona_id: String,
    display_name: String,
    role: String,
    archetype: String,
    locked: bool,
    body: String,
}

struct PredView {
    verdict: String,
    verdict_class: String,
    weighted: f64,
    weighted_pct: String,
    unweighted_pct: String,
    delta: String,
    pos: String,
    neu: String,
    neg: String,
    responders: String,
    confidence: String,
    strata_arch: Vec<StratumView>,
    strata_loc: Vec<StratumView>,
    raw: String,
}

struct StratumView {
    name: String,
    weight: String,
    fav: String,
}

struct OutcomeView {
    predicted_pct: String,
    actual_pct: String,
    error: String,
    metrics: Vec<(String, String)>,
}

#[derive(Template)]
#[template(path = "notes_form.html")]
struct NotesFormTemplate {
    t: &'static T,
    item: u64,
    notes: String,
    saved: bool,
}

#[derive(Template)]
#[template(path = "flash.html")]
struct FlashTemplate {
    ok: bool,
    msg: String,
}

#[derive(Template)]
#[template(path = "calibration.html")]
struct CalibrationTemplate {
    t: &'static T,
    rows: Vec<CalRow>,
    mean_error: String,
}

struct CalRow {
    item: u64,
    title: String,
    predicted: String,
    actual: String,
    error: String,
}

// ── prediction / outcome JSON (OVK formats, parsed leniently) ───────────────

#[derive(Deserialize)]
struct Prediction {
    responders: Option<u64>,
    weighted_favorability: Option<f64>,
    unweighted_favorability: Option<f64>,
    weighting_delta: Option<f64>,
    weighted_sentiment_pct: Option<SentimentPct>,
    by_archetype: Option<BTreeMap<String, Stratum>>,
    by_locality: Option<BTreeMap<String, Stratum>>,
    confidence: Option<String>,
}

#[derive(Deserialize)]
struct SentimentPct {
    positive: f64,
    neutral: f64,
    negative: f64,
}

#[derive(Deserialize)]
struct Stratum {
    weight_pct: f64,
    favorability: f64,
}

#[derive(Deserialize)]
struct Outcome {
    metrics: Option<BTreeMap<String, f64>>,
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

fn pred_view(json: &str, lang: Lang) -> Option<PredView> {
    let p: Prediction = serde_json::from_str(json).ok()?;
    let w = p.weighted_favorability.unwrap_or(0.0);
    let (verdict, verdict_class) = lang.verdict(w);
    let strata = |m: Option<BTreeMap<String, Stratum>>| {
        m.unwrap_or_default()
            .into_iter()
            .map(|(name, s)| StratumView {
                name,
                weight: format!("{:.1}%", s.weight_pct),
                fav: pct(s.favorability),
            })
            .collect()
    };
    let sent = p.weighted_sentiment_pct;
    let raw = serde_json::from_str::<serde_json::Value>(json)
        .and_then(|v| serde_json::to_string_pretty(&v))
        .unwrap_or_else(|_| json.to_string());
    Some(PredView {
        verdict: verdict.to_string(),
        verdict_class: verdict_class.to_string(),
        weighted: w,
        weighted_pct: pct(w),
        unweighted_pct: pct(p.unweighted_favorability.unwrap_or(0.0)),
        delta: format!("{:+.0}%", p.weighting_delta.unwrap_or(0.0) * 100.0),
        pos: sent.as_ref().map(|s| format!("{:.0}%", s.positive)).unwrap_or_default(),
        neu: sent.as_ref().map(|s| format!("{:.0}%", s.neutral)).unwrap_or_default(),
        neg: sent.as_ref().map(|s| format!("{:.0}%", s.negative)).unwrap_or_default(),
        responders: p.responders.map(|n| n.to_string()).unwrap_or_default(),
        confidence: p.confidence.unwrap_or_default(),
        strata_arch: strata(p.by_archetype),
        strata_loc: strata(p.by_locality),
        raw,
    })
}

/// success is the outcome metric comparable to weighted favorability.
fn outcome_success(json: &str) -> Option<f64> {
    let o: Outcome = serde_json::from_str(json).ok()?;
    o.metrics?.get("success").copied()
}

fn outcome_view(json: &str, predicted: Option<f64>) -> Option<OutcomeView> {
    let o: Outcome = serde_json::from_str(json).ok()?;
    let metrics = o.metrics.unwrap_or_default();
    let success = metrics.get("success").copied();
    let (predicted_pct, actual_pct, error) = match (predicted, success) {
        (Some(p), Some(a)) => (pct(p), pct(a), format!("{:+.0}%", (p - a) * 100.0)),
        (_, Some(a)) => (String::new(), pct(a), String::new()),
        _ => (String::new(), String::new(), String::new()),
    };
    Some(OutcomeView {
        predicted_pct,
        actual_pct,
        error,
        metrics: metrics.into_iter().map(|(k, v)| (k, format!("{v}"))).collect(),
    })
}

// ── handlers ────────────────────────────────────────────────────────────────

async fn index(
    State(st): State<AppState>,
    Query(q): Query<LangQuery>,
    headers: HeaderMap,
) -> Response {
    let lang = resolve_lang(&q, &headers);
    let page = IndexTemplate {
        t: t(lang),
        queue: st.cfg.queue.clone(),
        runner: match st.cfg.runner {
            Runner::PanelRun => "panel-run.sh".to_string(),
            Runner::AppendOnly => {
                if st.cfg.listener_enabled {
                    "append + resident listener".to_string()
                } else {
                    "append-only (no listener!)".to_string()
                }
            }
        },
        engine_default: st.cfg.reply_engine.clone(),
    };
    with_lang_cookie(lang, page.into_response())
}

async fn run_page(
    Path(item): Path<u64>,
    Query(q): Query<LangQuery>,
    headers: HeaderMap,
) -> Response {
    let lang = resolve_lang(&q, &headers);
    with_lang_cookie(lang, RunPageTemplate { t: t(lang), item }.into_response())
}

async fn runs_fragment(
    State(st): State<AppState>,
    Query(q): Query<LangQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let lang = resolve_lang(&q, &headers);
    let runs = {
        let store = st.store.lock().expect("store mutex poisoned");
        store.list_runs()?
    };
    let runs = runs
        .into_iter()
        .map(|r| {
            let mut title: String = r.draft.chars().take(90).collect();
            if title.chars().count() < r.draft.chars().count() {
                title.push('…');
            }
            let (fav, verdict, verdict_class) = r
                .prediction_json
                .as_deref()
                .and_then(|j| pred_view(j, lang))
                .map(|p| (p.weighted_pct, p.verdict, p.verdict_class))
                .unwrap_or_default();
            RunCard {
                item: r.item_ordinal,
                title,
                status: r.status,
                reply_count: r.reply_count,
                fav,
                verdict,
                verdict_class,
                has_outcome: r.outcome_json.is_some(),
            }
        })
        .collect();
    Ok(RunsListTemplate { t: t(lang), runs })
}

async fn run_fragment(
    State(st): State<AppState>,
    Path(item): Path<u64>,
    Query(q): Query<LangQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let lang = resolve_lang(&q, &headers);
    detail(&st, item, lang, String::new())
}

fn detail(
    st: &AppState,
    item: u64,
    lang: Lang,
    outcome_error: String,
) -> Result<RunDetailTemplate, AppError> {
    let store = st.store.lock().expect("store mutex poisoned");
    let run = store.get_run(item)?.ok_or(AppError::NotFound)?;
    let replies = store
        .replies_for(item)?
        .into_iter()
        .map(|r| ReplyView {
            persona_id: r.persona_id,
            display_name: r.display_name,
            role: match lang {
                Lang::Et => r.role_et.or(r.role.clone()).unwrap_or_default(),
                Lang::En => r.role.clone().unwrap_or_default(),
            },
            archetype: r.archetype.unwrap_or_default(),
            locked: r.locked,
            body: r.body,
        })
        .collect();
    let prediction = run.prediction_json.as_deref().and_then(|j| pred_view(j, lang));
    let predicted = prediction.as_ref().map(|p| p.weighted);
    let outcome = run.outcome_json.as_deref().and_then(|j| outcome_view(j, predicted));
    Ok(RunDetailTemplate {
        t: t(lang),
        item,
        pending: run.status == "pending",
        draft: run.draft,
        reply_count: run.reply_count,
        replies,
        prediction,
        engine_used: run.engine_used.unwrap_or_default(),
        outcome,
        outcome_error,
        notes: run.notes,
        saved: false,
    })
}

#[derive(Deserialize)]
struct SubmitForm {
    draft: String,
    #[serde(default)]
    engine: String,
}

async fn submit(
    State(st): State<AppState>,
    Query(q): Query<LangQuery>,
    headers: HeaderMap,
    Form(form): Form<SubmitForm>,
) -> Result<impl IntoResponse, AppError> {
    let _lang = resolve_lang(&q, &headers);
    let draft = form.draft.trim().to_string();
    if draft.is_empty() {
        return Ok(FlashTemplate { ok: false, msg: "Draft is empty — nothing submitted.".into() });
    }
    let engine = match form.engine.as_str() {
        "mock" => "mock",
        "model" => "model",
        _ => st.cfg.reply_engine.as_str(),
    }
    .to_string();
    match st.cfg.runner {
        Runner::PanelRun => {
            let script = st.cfg.ovk_root.join("scripts/panel-run.sh");
            if !script.exists() {
                return Ok(FlashTemplate {
                    ok: false,
                    msg: format!(
                        "panel-run.sh not found at {} — set OVK_ROOT, or use the default append runner",
                        script.display()
                    ),
                });
            }
            let child = Command::new("bash")
                .arg(script)
                .arg(&draft)
                .current_dir(&st.cfg.ovk_root)
                .env("PANEL_QUEUE", &st.cfg.queue)
                .env("REPLY_ENGINE", &engine)
                .env("_ZO_DOCTOR", "0")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            match child {
                Ok(_) => Ok(FlashTemplate {
                    ok: true,
                    msg: format!(
                        "Handed to OVK's panel-run (engine: {engine}). The run appears below as the rapids fill."
                    ),
                }),
                Err(e) => Ok(FlashTemplate { ok: false, msg: format!("could not start panel-run: {e}") }),
            }
        }
        Runner::AppendOnly => {
            let flow = st.flow.clone();
            let body = draft.clone();
            let ordinal = tokio::task::spawn_blocking(move || flow.append(&body, "content:query"))
                .await
                .map_err(|e| AppError::Internal(e.into()))??;
            {
                let store = st.store.lock().expect("store mutex poisoned");
                store.set_engine_requested(ordinal, &engine)?;
            }
            Ok(FlashTemplate {
                ok: true,
                msg: format!(
                    "content:query appended at ordinal {ordinal} (engine: {engine}). The listener picks it up within seconds."
                ),
            })
        }
    }
}

#[derive(Deserialize)]
struct NotesForm {
    notes: String,
}

async fn save_notes(
    State(st): State<AppState>,
    Path(item): Path<u64>,
    Query(q): Query<LangQuery>,
    headers: HeaderMap,
    Form(form): Form<NotesForm>,
) -> Result<impl IntoResponse, AppError> {
    let lang = resolve_lang(&q, &headers);
    let store = st.store.lock().expect("store mutex poisoned");
    store.set_notes(item, form.notes.trim())?;
    Ok(NotesFormTemplate { t: t(lang), item, notes: form.notes.trim().to_string(), saved: true })
}

#[derive(Deserialize)]
struct OutcomeForm {
    #[serde(default)]
    success: String,
    #[serde(default)]
    ctr: String,
    #[serde(default)]
    conversion: String,
    #[serde(default)]
    engagement: String,
}

/// Mirrors OVK's OutcomeRecord validation: success/ctr/conversion in 0..1,
/// engagement >= 0, at least one metric. Appended in OVK's exact body format
/// so its own calibration tooling can consume what we record.
fn build_outcome(item: u64, f: &OutcomeForm) -> Result<String, String> {
    let mut metrics = serde_json::Map::new();
    let parse = |name: &str, raw: &str, max_one: bool| -> Result<Option<f64>, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        let v: f64 = raw.parse().map_err(|_| format!("{name} must be numeric"))?;
        if v < 0.0 || (max_one && v > 1.0) {
            return Err(if max_one {
                format!("{name} must be within 0..1")
            } else {
                format!("{name} must be >= 0")
            });
        }
        Ok(Some(v))
    };
    for (name, raw, max_one) in [
        ("success", &f.success, true),
        ("ctr", &f.ctr, true),
        ("conversion", &f.conversion, true),
        ("engagement", &f.engagement, false),
    ] {
        if let Some(v) = parse(name, raw, max_one)? {
            metrics.insert(name.to_string(), serde_json::json!(v));
        }
    }
    if metrics.is_empty() {
        return Err("at least one metric required".into());
    }
    Ok(serde_json::json!({"item": item, "metrics": metrics}).to_string())
}

async fn record_outcome(
    State(st): State<AppState>,
    Path(item): Path<u64>,
    Query(q): Query<LangQuery>,
    headers: HeaderMap,
    Form(form): Form<OutcomeForm>,
) -> Result<impl IntoResponse, AppError> {
    let lang = resolve_lang(&q, &headers);
    let body = match build_outcome(item, &form) {
        Ok(b) => b,
        Err(msg) => return detail(&st, item, lang, msg),
    };
    let flow = st.flow.clone();
    let b = body.clone();
    let ct = format!("outcome:{item}");
    let ordinal = tokio::task::spawn_blocking(move || flow.append(&b, &ct))
        .await
        .map_err(|e| AppError::Internal(e.into()))??;
    {
        // Written directly too, so the UI shows it before the consumer's next tick.
        let store = st.store.lock().expect("store mutex poisoned");
        store.set_outcome(item, ordinal, &body)?;
    }
    detail(&st, item, lang, String::new())
}

async fn toggle_lock(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<LockQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let lang = resolve_lang(&LangQuery { lang: q.lang.clone() }, &headers);
    {
        let store = st.store.lock().expect("store mutex poisoned");
        store.toggle_locked(&id)?;
    }
    detail(&st, q.item, lang, String::new())
}

#[derive(Deserialize)]
struct LockQuery {
    item: u64,
    lang: Option<String>,
}

async fn calibration_fragment(
    State(st): State<AppState>,
    Query(q): Query<LangQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let lang = resolve_lang(&q, &headers);
    let runs = {
        let store = st.store.lock().expect("store mutex poisoned");
        store.calibrated_runs()?
    };
    let mut errors: Vec<f64> = Vec::new();
    let rows: Vec<CalRow> = runs
        .iter()
        .filter_map(|r: &Run| {
            let predicted = r
                .prediction_json
                .as_deref()
                .and_then(|j| pred_view(j, lang))
                .map(|p| p.weighted)?;
            let actual = r.outcome_json.as_deref().and_then(outcome_success)?;
            errors.push(predicted - actual);
            let mut title: String = r.draft.chars().take(50).collect();
            if title.chars().count() < r.draft.chars().count() {
                title.push('…');
            }
            Some(CalRow {
                item: r.item_ordinal,
                title,
                predicted: pct(predicted),
                actual: pct(actual),
                error: format!("{:+.0}%", (predicted - actual) * 100.0),
            })
        })
        .collect();
    let mean_error = if errors.is_empty() {
        String::new()
    } else {
        format!("{:+.1}%", errors.iter().sum::<f64>() / errors.len() as f64 * 100.0)
    };
    Ok(CalibrationTemplate { t: t(lang), rows, mean_error })
}

async fn htmx_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        include_str!("../static/htmx.min.js"),
    )
}

/// The repo-root static sales mockup (Tallinn scenarios, illustrative data),
/// baked in at compile time so it ships with this binary rather than needing a
/// second deploy path. Unrelated to the live panel at `/`, which runs real
/// submissions through OVK's queue.
async fn static_mockup() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../../index.html"),
    )
}

// ── error plumbing ──────────────────────────────────────────────────────────

pub enum AppError {
    NotFound,
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Internal(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            AppError::Internal(e) => {
                eprintln!("internal error: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {e:#}")).into_response()
            }
        }
    }
}
