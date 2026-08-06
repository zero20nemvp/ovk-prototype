use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

/// Native client for the `ground flow` daemon's Unix-socket protocol:
/// one connection per request, one JSON line out, one JSON line back.
/// Command set (verified against the daemon): append, append_batch, len, get,
/// cursor_create, cursor_delete, cursor_state, cursor_move, cursor_read.
/// Bodies travel base64-encoded. Entries are immutable; readers only ever
/// advance cursors — this client cannot modify OVK's queue history even by
/// accident.
#[derive(Debug, Clone)]
pub struct FlowClient {
    socket_path: PathBuf,
}

/// A decoded queue entry.
#[derive(Debug, Clone)]
pub struct Entry {
    pub ordinal: u64,
    pub content_type: String,
    pub body: String,
    pub enqueued_at: i64,
}

/// macOS caps sun_path at ~104 bytes; a socket in a deeply nested directory
/// is unreachable by absolute path. OVK's own clients dodge this with relative
/// paths; we dodge it with a short symlink in the OS temp dir, which the
/// kernel resolves at connect time.
const SUN_PATH_MAX: usize = 100;

impl FlowClient {
    pub fn new(socket_path: PathBuf) -> Self {
        let socket_path = Self::shorten_if_needed(socket_path);
        FlowClient { socket_path }
    }

    fn shorten_if_needed(path: PathBuf) -> PathBuf {
        if path.as_os_str().len() <= SUN_PATH_MAX {
            return path;
        }
        let link = std::env::temp_dir().join(format!("ovk-proto-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&link);
        match std::os::unix::fs::symlink(&path, &link) {
            Ok(()) if link.as_os_str().len() <= SUN_PATH_MAX => {
                eprintln!(
                    "flow socket path exceeds sun_path limit; connecting via symlink {}",
                    link.display()
                );
                link
            }
            _ => path, // connect will fail with a clear error either way
        }
    }

    /// Locate the daemon socket for `queue` by asking the `ground` CLI from
    /// inside the OVK checkout (the same way OVK's own scripts do). Starts the
    /// daemon if it is not running — an operational act, not a change to OVK.
    pub fn discover(ovk_root: &Path, queue: &str) -> Result<Self> {
        for attempt in 0..10 {
            let out = Command::new("ground")
                .args(["flow", "status", queue])
                .current_dir(ovk_root)
                .env("_ZO_DOCTOR", "0")
                .output()
                .context("running `ground flow status` (is `ground` on PATH?)")?;
            let text = String::from_utf8_lossy(&out.stdout).to_string()
                + &String::from_utf8_lossy(&out.stderr);
            if let Some(path) = text
                .lines()
                .find_map(|l| l.trim().strip_prefix("socket").map(|r| r.trim().to_string()))
            {
                let p = PathBuf::from(&path);
                let abs = if p.is_absolute() { p } else { ovk_root.join(p) };
                return Ok(FlowClient::new(abs));
            }
            if attempt == 0 {
                eprintln!("flow daemon for queue '{queue}' not running — starting it");
                let _ = Command::new("ground")
                    .args(["flow", "serve", queue])
                    .current_dir(ovk_root)
                    .env("_ZO_DOCTOR", "0")
                    .status();
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        bail!("could not discover flow socket for queue '{queue}' in {}", ovk_root.display())
    }

    /// Append a text body; returns the assigned ordinal.
    pub fn append(&self, body: &str, content_type: &str) -> Result<u64> {
        let res = self.request(json!({
            "cmd": "append",
            "body": base64::engine::general_purpose::STANDARD.encode(body),
            "content_type": content_type,
        }))?;
        res["ordinal"]
            .as_u64()
            .ok_or_else(|| anyhow!("append: no ordinal in response: {res}"))
    }

    /// Atomic multi-append — all entries land contiguously or none do
    /// (AX-OVK-00019: a prediction must never land without its replies).
    /// Returns the assigned ordinals.
    pub fn append_batch(&self, items: &[(String, String)]) -> Result<Vec<u64>> {
        let encoded: Vec<Value> = items
            .iter()
            .map(|(body, ct)| {
                json!({
                    "body": base64::engine::general_purpose::STANDARD.encode(body),
                    "content_type": ct,
                })
            })
            .collect();
        let res = self.request(json!({"cmd": "append_batch", "items": encoded}))?;
        if !res["ok"].as_bool().unwrap_or(false) {
            bail!("append_batch failed: {res}");
        }
        Ok(res["ordinals"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
            .unwrap_or_default())
    }

    pub fn len(&self) -> Result<u64> {
        let res = self.request(json!({"cmd": "len"}))?;
        res["len"].as_u64().ok_or_else(|| anyhow!("len: bad response: {res}"))
    }

    /// Cursor-free fetch by absolute ordinal; None when the ordinal is absent
    /// (i.e. we are caught up to the head).
    pub fn get(&self, ordinal: u64) -> Result<Option<Entry>> {
        let res = self.request(json!({"cmd": "get", "ordinal": ordinal}))?;
        if !res["ok"].as_bool().unwrap_or(false) {
            return Ok(None);
        }
        Ok(Some(Self::decode_entry(&res)?))
    }

    /// Idempotent.
    pub fn cursor_create(&self, name: &str) -> Result<()> {
        self.request(json!({"cmd": "cursor_create", "name": name}))?;
        Ok(())
    }

    pub fn cursor_delete(&self, name: &str) -> Result<()> {
        self.request(json!({"cmd": "cursor_delete", "name": name}))?;
        Ok(())
    }

    /// Last fully-processed ordinal, or None for a fresh cursor.
    pub fn cursor_position(&self, name: &str) -> Result<Option<u64>> {
        let res = self.request(json!({"cmd": "cursor_state", "name": name}))?;
        Ok(res["position"].as_u64())
    }

    /// Commit the cursor to `ordinal` (position == last fully-processed entry;
    /// at-least-once semantics). The daemon only moves by keyword
    /// (origin | head | next | prev), so this walks: origin to position a fresh
    /// cursor, then next until the target. In normal consumption the walk is a
    /// single step.
    pub fn cursor_commit(&self, name: &str, ordinal: u64) -> Result<()> {
        let mut pos = match self.cursor_position(name)? {
            Some(p) => p,
            None => {
                self.cursor_move(name, "origin")?;
                0
            }
        };
        if pos > ordinal {
            bail!("cursor '{name}' at {pos}, beyond commit target {ordinal}");
        }
        while pos < ordinal {
            self.cursor_move(name, "next")?;
            pos += 1;
        }
        Ok(())
    }

    /// Jump the cursor to the newest entry; returns its position. Used to
    /// fast-forward past a backlog without walking it.
    pub fn cursor_move_head(&self, name: &str) -> Result<u64> {
        // An unpositioned cursor may refuse relative moves — position it first.
        if self.cursor_position(name)?.is_none() {
            self.cursor_move(name, "origin")?;
        }
        self.cursor_move(name, "head")
    }

    fn cursor_move(&self, name: &str, to: &str) -> Result<u64> {
        let res = self.request(json!({"cmd": "cursor_move", "name": name, "to": to}))?;
        if !res["ok"].as_bool().unwrap_or(false) {
            bail!("cursor_move {to} failed: {res}");
        }
        Ok(res["position"].as_u64().unwrap_or(0))
    }

    fn decode_entry(res: &Value) -> Result<Entry> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(res["body"].as_str().unwrap_or_default())
            .context("entry body is not valid base64")?;
        Ok(Entry {
            ordinal: res["ordinal"].as_u64().ok_or_else(|| anyhow!("entry without ordinal"))?,
            content_type: res["content_type"].as_str().unwrap_or_default().to_string(),
            body: String::from_utf8_lossy(&raw).into_owned(),
            enqueued_at: res["enqueued_at"].as_i64().unwrap_or(0),
        })
    }

    fn request(&self, payload: Value) -> Result<Value> {
        let mut stream = UnixStream::connect(&self.socket_path).with_context(|| {
            format!("cannot reach flow daemon at {}", self.socket_path.display())
        })?;
        let mut line = serde_json::to_string(&payload)?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        let mut reader = BufReader::new(stream);
        let mut resp = String::new();
        reader.read_line(&mut resp)?;
        if resp.is_empty() {
            bail!("no response from flow daemon");
        }
        Ok(serde_json::from_str(&resp)?)
    }
}
