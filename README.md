# OVK — Red-Team PR Panel (Prototype)

A static, bilingual (EN/ET) UX prototype of a red-team panel that stress-tests draft public communications against how different audience segments, journalists, and hostile actors would likely react before publication.

**This is a sales/investor demo prototype, not the production system.** All draft content, persona reactions, and calibration data are fictional/illustrative — no real client or prospect content is used anywhere in this repo.

## Files

- `server/` — Rust + HTMX backend that runs the panel for real against the OVK
  engine, integrating solely over OVK's flow queue (see `server/README.md`).
  The two HTML files below remain the static, mocked sales demo.
- `index.html` — the standalone, self-contained page (open directly in a browser, or serve as a static site). Single file: inline CSS + JS, no build step, no dependencies.
- `fragment-source.html` — the same content as a body-only fragment (no `<!doctype>`/`<html>`/`<head>`), kept for republishing into tools that wrap fragments themselves (e.g. Claude Artifacts). Edit this file, then regenerate `index.html` by wrapping it with a `<!doctype html><html><head><meta charset="utf-8">...</head><body>` shell — the wrapper is what fixes Estonian character rendering (õ/ä/ö/ü/š/ž), so don't serve `fragment-source.html` directly without it.

## What it covers

- Two demo scenarios (Education-transition, Diagnostics-turnaround/healthcare), each with an "Original submission" and "Revised" draft, plus a "Blank/custom" mode with a live keyword-trigger re-scoring engine.
- 8 personas per scenario, including a hostile-amplification voice whose output is always shown locked (score + flag reason only — never copyable reaction text).
- A mocked prototype account (localStorage only, no real backend/auth/password) that saves run results locally in the browser.
- A calibration-loop tab, clearly labeled as illustrative/simulated — no real outcome data exists yet.
- A "why this isn't just one AI prompt" section explaining the panel's actual differentiators (weighted persona modeling, a self-correcting calibration loop, honesty-first uncertainty labeling) in plain, non-technical language.

## Explicitly out of scope (see in-page "Engineering handoff notes")

Real stratified population modeling, real media/social-listening integration, real multi-tenant auth/billing, and the production message-bus infrastructure. This prototype exists to align on UX and data shape before that gets built.
