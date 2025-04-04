# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.1] - unreleased

### Fixed/Improved

 - Improve typings
 - Migrate to uv


## [0.3.0] - 2024-12-04

### Fixed/Improved

 - Use `Mapping` instead of `Dict` for json type (`Mapping` is co-variant)
 - python type annotations

### Changed

- Rust: Improve `Server::listen_ws()` and `Server::listen_http()` API.

## [0.2.1] - 2024-04-23

### Fixed

- Type annotation

## [0.2.0] - 2024-04-23

### Added

- Rigorous type annotation with mypy
- Transition tooling to mypy

## [0.1.0] - 2022-08-29

Initial release.
