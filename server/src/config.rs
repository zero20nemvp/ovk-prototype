use std::env;
use std::path::PathBuf;

/// All knobs come from the environment; every one has a working default for
/// the dev box. Nothing here mutates OVK — OVK_ROOT is only used to (a) find
/// the flow daemon's socket and (b) optionally invoke OVK's own panel-run.sh.
#[derive(Debug, Clone)]
pub struct Config {
    /// Checkout of the OVK repo (read-only from our side).
    pub ovk_root: PathBuf,
    /// Flow queue name the panel rides on (OVK default: "panel").
    pub queue: String,
    /// Explicit socket path override; when unset we discover it via
    /// `ground flow status <queue>` run inside ovk_root.
    pub socket_override: Option<PathBuf>,
    /// TCP flow hub "host:port" (e.g. OVK prod on the Air over the tailnet).
    /// When set, wins over local socket discovery entirely.
    pub flow_hub: Option<String>,
    /// Durable cursor name this backend consumes the rapids with.
    pub cursor: String,
    /// "append" (default): submitting a draft appends the content:query and
    /// the resident listener answers it — the fully-live bus path.
    /// "panel-run": submitting invokes OVK's one-shot scripts/panel-run.sh
    /// instead (disable the listener too, or they race to answer).
    pub runner: Runner,
    /// Resident panel listener answering content:query entries (default on).
    pub listener_enabled: bool,
    /// Default reply engine (mock | model). "model" routes personas through
    /// OVK's llm.sh seam; the listener probes reachability per run and
    /// degrades to mock with an honest label if the model is down.
    pub reply_engine: String,
    /// Address the HTTP server binds.
    pub bind: String,
    /// Where our OWN data lives (SQLite enrichment store). This is the
    /// "data held in this project that is not in OVK".
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Runner {
    PanelRun,
    AppendOnly,
}

impl Config {
    pub fn from_env() -> Self {
        let ovk_root =
            PathBuf::from(env::var("OVK_ROOT").unwrap_or_else(|_| "../../ovk".into()));
        // Absolutize: script paths built from this root are passed to commands
        // that also set current_dir to it, so a relative root resolves wrong.
        let ovk_root = ovk_root.canonicalize().unwrap_or(ovk_root);
        let runner = match env::var("OVK_RUNNER").as_deref() {
            Ok("panel-run") => Runner::PanelRun,
            _ => Runner::AppendOnly,
        };
        Config {
            ovk_root,
            queue: env::var("OVK_QUEUE").unwrap_or_else(|_| "panel".into()),
            socket_override: env::var("OVK_FLOW_SOCKET").ok().map(PathBuf::from),
            flow_hub: env::var("OVK_FLOW_HUB").ok().filter(|s| !s.is_empty()),
            cursor: env::var("OVK_CURSOR").unwrap_or_else(|_| "ovk-prototype".into()),
            runner,
            listener_enabled: env::var("OVK_LISTENER").as_deref() != Ok("0"),
            reply_engine: env::var("REPLY_ENGINE").unwrap_or_else(|_| "model".into()),
            bind: env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8787".into()),
            data_dir: PathBuf::from(env::var("DATA_DIR").unwrap_or_else(|_| "data".into())),
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("ovk-prototype.db")
    }
}
