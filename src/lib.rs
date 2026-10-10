//! `peek-server`: the backend of [Peek](https://peek.teamofsilicons.com).
//!
//! It exchanges and rotates Silicon Accounts sessions, encrypts successful token
//! responses for recovery, and introspects every bearer live. Account UUIDs own
//! drawings and settings. Short-lived Accounts proofs authorize Ting operations;
//! the server also brokers speech, bug reports, telemetry, and account webhooks.
//!
//! The binary (`src/main.rs`) only loads [`config::Config`], builds
//! [`state::AppState`] and serves [`app::router`]. Everything else is internal;
//! `src/README.md` documents routes, environment and local development.
#![forbid(unsafe_code)]

pub mod app;
pub mod config;
pub mod state;
pub mod telemetry;

mod accounts;
mod auth;
mod crypto;
mod db;
mod deepgram;
mod elevenlabs;
mod error;
mod extract;
mod github;
mod idempotency;
mod obo;
mod openai;
mod plane;
mod ratelimit;
mod routes;
mod store;
mod ting;
