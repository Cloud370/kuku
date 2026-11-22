# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Agent loop: recover once from model output-limit responses with an auditable `model.recovery` event and frozen prompt notice
- Release pipeline: CI builds for Linux x86_64, macOS aarch64, Windows x86_64
- Install scripts for Linux/macOS (`install.sh`) and Windows (`install.ps1`)
- Config: `[update]` section with source, channel, and multi-source support
- Config migration for `[update]` via `config_patch_defaults`
- Version injection at build time (`kuku --version`)
- Makefile with build-web, build, build-all targets

## [0.1.0-alpha.3] - 2026-11-22

Developer preview for the native terminal and Session workflow.

### Highlights

- **Terminal and Sessions.** The TUI can resume a durable Session, switch Agents, manage queued inputs, and inspect background Jobs and their output.
- **History and context.** Reversible context compaction keeps earlier history available. Use Ctrl+G to switch between saved Turns, bring the original message back as a draft, or preview optional file restoration. File restoration uses verified built-in edits and shows the affected files and line counts.
- **Tools and output.** Launch programs with literal arguments, compose deterministic operations in `run_js`, and search retained output by byte offset. Recognized command output gets a bounded diagnostic summary with links to original bytes.
- **Storage and inspection.** Large Sessions pack cold history in seekable gzip frames inside one journal. The CLI can page, query and export the committed events as JSONL.

### Measured on a local fixture

A 509 MiB synthetic Session packed to 65 MiB. In the paired cold-switch test with a different request byte limit, the server took 88–92 ms to commit the switch, down from 2.19–2.31 s. The TUI displayed the restored draft in 0.22 s in the local run.
