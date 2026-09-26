//! `peek-server`: the backend of [Peek](https://peek.teamofsilicons.com).
//!
//! It holds the IAM app secret and nothing a Silicon owns: it exchanges SLTs,
//! rotates and revokes app sessions (the tokens go straight back to the
//! caller, BLUEPRINT D5), introspects every bearer live, mints Ting OBO proofs
//! for enrollment and deliveries, mints short-lived Deepgram JWTs, keeps each
//! Silicon's drawing copy, files bug reports, relays client telemetry to Space
//! Station, receives IAM webhooks and acts as a Honeycomb lifecycle
//! participant for testing environments.
//!
//! The binary (`src/main.rs`) only loads [`config::Config`], builds
//! [`state::AppState`] and serves [`app::router`]. Everything else is internal;
//! `src/README.md` documents routes, environment and local development.
#![forbid(unsafe_code)]

pub mod app;
pub mod config;
pub mod state;
pub mod telemetry;

mod auth;
mod crypto;
mod db;
mod deepgram;
mod error;
mod extract;
mod github;
mod honeycomb;
mod iam;
mod idempotency;
mod plane;
mod ratelimit;
mod routes;
mod store;
mod ting;
mod webhook;
