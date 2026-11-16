use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use talocart_platform::{
    database,
    identity::Identity,
};
use talocart_web::{AppState, router};
use tower::ServiceExt;

// Shared fixtures only; all scenarios are declared exactly once in their owning target.
#[path = "../identity/support.rs"]
#[allow(dead_code)] // Each integration target uses only a subset of these shared fixtures.
mod support;


use support::*;
