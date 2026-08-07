use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::config::Config;
use crate::flow::FlowClient;
use crate::store::Store;

/// Resident panel listener (roadmap item 2). Answers ANY `content:query` on
/// the rapids — not just ones this UI appended — by fanning the draft out to
/// OVK's persona panel and appending `reply:<id>` entries plus a
/// `prediction:<n>`. OVK's repo is used strictly read-only: persona files and
/// weights are read, `llm.sh` (the one model seam) and `panel-aggregate.sh`
/// (a pure function: replies dir in, prediction JSON out) are invoked.
///
/// Rules of engagement:
/// - own durable cursor ("panel-listener"), separate from the UI ingester;
/// - a fresh cursor fast-forwards past the backlog rather than re-answering
///   history;
/// - a query that already has a prediction later in the queue is skipped
///   (e.g. one produced by a manual panel-run.sh);
/// - engine per run: "model" routes personas through llm.sh; if the model is
///   unreachable the run degrades to mock and is labeled so — never silently.
pub fn run(flow: FlowClient, store: Arc<Mutex<Store>>, cfg: Arc<Config>) {
    let cursor = "panel-listener";
    if let Err(e) = flow.cursor_create(cursor) {
        eprintln!("listener: cannot create cursor: {e:#}");
    }
    // Backlog boundary for a FRESH cursor: everything that already existed
    // when this process started is history and gets skipped; everything after
    // — including a query submitted milliseconds after boot — gets answered.
    // (Jumping to `head` at first tick instead would eat early submissions.)
    let baseline = loop {
        match flow.len() {
            Ok(n) => break n,
            Err(e) => {
                eprintln!("listener: waiting for flow endpoint: {e:#}");
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    };
    loop {
        match step(&flow, &store, &cfg, cursor, baseline) {
            Ok(true) => {}
            Ok(false) => std::thread::sleep(Duration::from_millis(1500)),
            Err(e) => {
                eprintln!("listener: {e:#}");
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

fn step(
    flow: &FlowClient,
    store: &Arc<Mutex<Store>>,
    cfg: &Arc<Config>,
    cursor: &str,
    baseline: u64,
) -> Result<bool> {
    flow.cursor_create(cursor)?; // idempotent; may have failed at startup
    let pos = flow.cursor_position(cursor)?;
    // Fresh cursor: skip exactly the pre-boot backlog (ordinals < baseline),
    // then consume normally from there.
    let next = match pos {
        Some(p) => p + 1,
        None if baseline > 0 => {
            flow.cursor_commit(cursor, baseline - 1)?;
            eprintln!("listener: fast-forwarded past {baseline} backlog entries");
            baseline
        }
        None => 0,
    };
    let Some(entry) = flow.get(next)? else {
        return Ok(false);
    };
    if entry.content_type == "content:query" {
        if let Err(e) = answer(flow, store, cfg, entry.ordinal, &entry.body) {
            // Log and move on — one bad query must not wedge the listener.
            eprintln!("listener: query {} failed: {e:#}", entry.ordinal);
        }
    }
    flow.cursor_commit(cursor, next)?;
    Ok(true)
}

fn answer(
    flow: &FlowClient,
    store: &Arc<Mutex<Store>>,
    cfg: &Arc<Config>,
    item: u64,
    draft: &str,
) -> Result<()> {
    if prediction_exists(flow, item)? {
        eprintln!("listener: query {item} already has a prediction — skipping");
        return Ok(());
    }
    let requested = {
        let st = store.lock().expect("store mutex poisoned");
        st.engine_requested(item)?.unwrap_or_else(|| cfg.reply_engine.clone())
    };
    let engine_used = if requested == "model" {
        if probe_model(&cfg.ovk_root) { "model".to_string() } else {
            eprintln!("listener: model engine unreachable — degrading query {item} to mock");
            "mock (model unreachable)".to_string()
        }
    } else {
        "mock".to_string()
    };
    let use_model = engine_used == "model";

    let panel = load_panel(&cfg.ovk_root)?;
    eprintln!(
        "listener: query {item} → fanning out to {} personas (engine: {engine_used})",
        panel.len()
    );

    // Fan out in chunks of 4 (a local SLM serves few concurrent requests).
    let mut replies: Vec<(String, String)> = Vec::new(); // (persona id, text)
    for chunk in panel.chunks(4) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|p| {
                let p = p.clone();
                let ovk = cfg.ovk_root.clone();
                let draft = draft.to_string();
                std::thread::spawn(move || {
                    let text = if use_model {
                        model_reply(&ovk, &p, &draft).unwrap_or_default()
                    } else {
                        mock_reply(&p.archetype).to_string()
                    };
                    (p.id, text)
                })
            })
            .collect();
        for h in handles {
            if let Ok((id, text)) = h.join() {
                // A failed/empty model call drops the persona — no fake
                // "(no reply)" polluting sentiment (OVK's own rule).
                if !text.trim().is_empty() {
                    replies.push((id, text.trim().to_string()));
                }
            }
        }
    }
    let dropped = panel.len() - replies.len();
    if replies.is_empty() {
        anyhow::bail!("no persona replied — nothing to batch, no prediction");
    }
    if dropped > 0 {
        eprintln!("listener: {dropped} personas dropped (failed/empty)");
    }

    // Atomic batch: reply entries land together (AX-OVK-00019).
    let items: Vec<(String, String)> = replies
        .iter()
        .map(|(id, text)| (text.clone(), format!("reply:{id}")))
        .collect();
    flow.append_batch(&items)?;

    // Aggregate via OVK's own script — pure function, no queue writes.
    let pred_json = aggregate(&cfg.ovk_root, &cfg.data_dir, item, &replies)?;
    flow.append(&pred_json, &format!("prediction:{item}"))?;
    eprintln!("listener: prediction appended for query {item}");

    let st = store.lock().expect("store mutex poisoned");
    st.set_engine_used(item, &engine_used)?;
    Ok(())
}

/// Has anything after `item` already predicted it? Guards against
/// double-answering queries served by a manual panel-run.sh.
fn prediction_exists(flow: &FlowClient, item: u64) -> Result<bool> {
    let tag = format!("prediction:{item}");
    let len = flow.len()?;
    for ord in (item + 1)..len {
        if let Some(e) = flow.get(ord)? {
            if e.content_type == tag {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

// ── panel loading (read-only from OVK) ──────────────────────────────────────

#[derive(Debug, Clone)]
struct Persona {
    id: String,
    archetype: String,
    prompt: String,
}

fn load_panel(ovk_root: &Path) -> Result<Vec<Persona>> {
    let weights = std::fs::read_to_string(ovk_root.join("panel/weights.tsv"))
        .context("reading panel/weights.tsv (run OVK's panel-generate.sh first?)")?;
    let mut panel = Vec::new();
    for line in weights.lines() {
        let mut f = line.split('\t');
        let (Some(id), Some(_w), Some(arch)) = (f.next(), f.next(), f.next()) else { continue };
        if id.is_empty() {
            continue;
        }
        let persona_md = std::fs::read_to_string(ovk_root.join(format!("panel/personas/{id}.md")))
            .unwrap_or_default();
        panel.push(Persona {
            id: id.to_string(),
            archetype: arch.to_string(),
            prompt: system_prompt(&persona_md),
        });
    }
    Ok(panel)
}

/// Mirrors panel-run.sh's persona_system_prompt().
fn system_prompt(persona_md: &str) -> String {
    format!(
        "You are a marketing panel persona. Stay fully in character; never break the fourth wall.\n\n\
         {persona_md}\n\n\
         Respond ONLY as this persona to the message shown, in 1-2 honest sentences filtered\n\
         through all three layers (your demographics, your archetype's instincts, and your\n\
         local context today). No markdown, no signature."
    )
}

// ── reply engines ───────────────────────────────────────────────────────────

/// Ported from panel-run.sh's mock_reply(): deterministic, archetype-flavored,
/// exercises the sentiment scorer.
fn mock_reply(archetype: &str) -> &'static str {
    match archetype {
        "Everyman" => "I'd want to know what this actually changes for my street and my wallet before making my mind up — the announcement alone doesn't tell me.",
        "Caregiver" => "If the children's daily routine is protected I could support this — and promising parent meetings before anything changes is reassuring.",
        "Explorer" => "Change doesn't scare me — this reads like a step forward and I'd welcome trying the new arrangement.",
        "Creator" => "The wording is polished, but I'm looking for the substance behind it — show me what's actually being decided.",
        "Sage" => "Where are the figures? Dates, budget lines, sources — until those are on the table this text tells me nothing.",
        "Ruler" => "A city that governs properly publishes dates and budgets; this feels rushed and I doubt the follow-through.",
        "Lover" => "The tone is respectful and it feels fair — if they follow through with the same warmth, I'm on board.",
        "Innocent" => "It sounds honest and clear to me — I believe the city means well with this.",
        "Jester" => "Ah, another majestic press release from city hall — I'll believe it when the diggers actually show up.",
        "Outlaw" => "I don't trust a word of this — it's spin, the decision was made long ago and residents are the last to hear.",
        "Hero" => "Commitments are only real when tracked — I'll be watching whether each promise here actually gets delivered.",
        "Magician" => "There's a real chance here to change how the city feels — done properly, this is a welcome shift.",
        _ => "I'd need to see more detail before I can say.",
    }
}

fn model_reply(ovk_root: &Path, p: &Persona, draft: &str) -> Result<String> {
    run_with_timeout(
        Command::new("bash")
            .arg(ovk_root.join("scripts/llm.sh"))
            .args(["--role", "reply", "--system", &p.prompt])
            .current_dir(ovk_root)
            .env("_ZO_DOCTOR", "0"),
        draft,
        Duration::from_secs(120),
    )
}

/// One cheap model call to decide whether "model" is honest for this run.
fn probe_model(ovk_root: &Path) -> bool {
    run_with_timeout(
        Command::new("bash")
            .arg(ovk_root.join("scripts/llm.sh"))
            .args(["--role", "reply", "--system", "You are a health check. Reply with the single word OK."])
            .current_dir(ovk_root)
            .env("_ZO_DOCTOR", "0"),
        "Are you there?",
        Duration::from_secs(20),
    )
    .map(|out| !out.trim().is_empty())
    .unwrap_or(false)
}

// ── aggregation via OVK's pure script ───────────────────────────────────────

fn aggregate(
    ovk_root: &Path,
    data_dir: &Path,
    item: u64,
    replies: &[(String, String)],
) -> Result<String> {
    let dir = data_dir.join(format!("tmp-aggregate-{item}"));
    std::fs::create_dir_all(&dir)?;
    // Absolutize: the aggregate child runs with current_dir(ovk_root), which
    // would re-resolve a relative data_dir against the wrong root.
    let dir = dir.canonicalize()?;
    for (id, text) in replies {
        std::fs::write(dir.join(id), text)?;
    }
    let out = Command::new("bash")
        .arg(ovk_root.join("scripts/panel-aggregate.sh"))
        .arg("--replies")
        .arg(&dir)
        .arg("--json-only")
        .current_dir(ovk_root)
        .env("_ZO_DOCTOR", "0")
        .output()
        .context("running panel-aggregate.sh")?;
    let _ = std::fs::remove_dir_all(&dir);
    anyhow::ensure!(
        out.status.success(),
        "panel-aggregate.sh failed: {}",
        String::from_utf8_lossy(&out.stderr).chars().take(300).collect::<String>()
    );
    let json = String::from_utf8_lossy(&out.stdout).trim().to_string();
    anyhow::ensure!(!json.is_empty(), "panel-aggregate.sh produced no JSON");
    Ok(json)
}

// ── subprocess with timeout ─────────────────────────────────────────────────

fn run_with_timeout(cmd: &mut Command, stdin_data: &str, timeout: Duration) -> Result<String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning subprocess")?;
    if let Some(mut sin) = child.stdin.take() {
        let _ = sin.write_all(stdin_data.as_bytes());
        // drop closes the pipe → child sees EOF
    }
    let started = Instant::now();
    loop {
        if let Some(_status) = child.try_wait()? {
            let mut out = String::new();
            if let Some(mut sout) = child.stdout.take() {
                use std::io::Read as _;
                let _ = sout.read_to_string(&mut out);
            }
            return Ok(out);
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("subprocess timed out after {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
