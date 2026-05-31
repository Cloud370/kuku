# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Release pipeline: CI builds for Linux x86_64, macOS aarch64, Windows x86_64
- Self-update mechanism (`kuku update`) with multi-source support
- Channel system (stable/alpha) for pre-release tracking
- Install scripts for Linux/macOS (`install.sh`) and Windows (`install.ps1`)
- Config migration for `[update]` section via `config_patch_defaults`
- Version injection at build time (`kuku --version`)
- Makefile with build-web, build, build-all targets
