mod config;
mod consumer;
mod flow;
mod i18n;
mod listener;
mod routes;
mod seeds;
mod store;

use std::sync::{Arc, Mutex};

use anyhow::Context;

use config::Config;
use flow::FlowClient;
use routes::AppState;
use store::Store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Arc::new(Config::from_env());
    anyhow::ensure!(
        cfg.ovk_root.exists(),
        "OVK checkout not found at {} — set OVK_ROOT",
        cfg.ovk_root.display()
    );

    let store = Arc::new(Mutex::new(
        Store::open(&cfg.db_path()).context("opening enrichment store")?,
    ));
    {
        let seeds = seeds::load(&cfg.ovk_root);
        if seeds.is_empty() {
            eprintln!("warning: no personas seeded (panel/weights.tsv missing in OVK_ROOT?)");
        } else {
            let n = seeds.len();
            store
                .lock()
                .expect("store mutex poisoned")
                .seed_personas(&seeds)
                .context("seeding personas")?;
            eprintln!("seeded {n} personas with red-team roles (Outlaw locked by default)");
        }
    }

    let flow = Arc::new(match &cfg.socket_override {
        Some(p) => FlowClient::new(p.clone()),
        None => FlowClient::discover(&cfg.ovk_root, &cfg.queue)
            .context("discovering flow daemon socket")?,
    });
    eprintln!(
        "rapids: queue '{}', {} entries, cursor '{}'",
        cfg.queue,
        flow.len().map(|n| n.to_string()).unwrap_or_else(|_| "?".into()),
        cfg.cursor
    );

    {
        let f = (*flow).clone();
        let s = store.clone();
        let cursor = cfg.cursor.clone();
        std::thread::spawn(move || consumer::run(f, s, cursor));
    }
    if cfg.listener_enabled {
        let f = (*flow).clone();
        let s = store.clone();
        let c = cfg.clone();
        std::thread::spawn(move || listener::run(f, s, c));
        eprintln!("resident panel listener enabled (default engine: {})", cfg.reply_engine);
    }

    let app = routes::router(AppState { store, flow, cfg: cfg.clone() });
    let listener = tokio::net::TcpListener::bind(&cfg.bind)
        .await
        .with_context(|| format!("binding {}", cfg.bind))?;
    eprintln!("ovk-prototype server listening on http://{}", cfg.bind);
    axum::serve(listener, app).await?;
    Ok(())
}
