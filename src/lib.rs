//! `peek-server`: the backend of [Peek](https://peek.teamofsilicons.com).
//!
//! It holds the IAM app secret and encrypted feature credentials: it exchanges SLTs,
//! rotates and revokes app sessions (the tokens go straight back to the
//! caller), introspects every bearer live, and stores independently approved Ting roots
//! for enrollment and deliveries, mints direct TTS credentials and relays `OpenAI` STT, keeps each
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
mod elevenlabs;
mod error;
mod extract;
mod github;
mod honeycomb;
mod iam;
mod idempotency;
mod openai;
mod obo;
mod plane;
mod ratelimit;
mod routes;
mod store;
mod ting;
mod webhook;
