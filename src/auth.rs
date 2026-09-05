//! Auth (OAuth 2.1 + PKCE, FR-11) removed 2026-08-13.
//!
//! WebFind is a local-only tool (no cloud, no multi-tenant), so the entire
//! auth layer was a deployment assumption rather than a requirement. It has
//! been removed:
//!
//! - `pub mod auth` dropped from `lib.rs` (this file is no longer compiled).
//! - `oauth2` / `jsonwebtoken` / `base64` / `urlencoding` removed from
//!   `Cargo.toml`.
//! - `/oauth` routes removed from `api.rs`.
//! - `OAuthConfig` / `resolve_oauth_config` / `WEBFIND_AUTH_MODE` /
//!   `WEBFIND_OAUTH_*` removed from `config.rs` and `.env.example`.
//!
//! Delete this file.
