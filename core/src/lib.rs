#![allow(clippy::too_many_arguments)]

//! Chimera Core — domain-pure foundation crate.
//!
//! Architectural law (see docs/architecture/rust-workspace.md):
//! this crate must NEVER import database drivers (sqlx, diesel) or web
//! frameworks (axum, actix). It hosts domain constants and framework-agnostic
//! utilities. The `operator` crate re-exports these modules so the legacy
//! `chimera_operator::*` paths keep working during incremental extraction.

pub mod config;
pub mod constants;
pub mod engine;
pub mod error;
pub mod experiment;
pub mod jupiter;
pub mod models;
pub mod price_cache;
pub mod retry;
pub mod roster;
pub mod utils;

/// Test-only helpers shared across module test suites.
#[cfg(test)]
pub mod test_util {
    use tracing::subscriber::Interest;
    use tracing::level_filters::LevelFilter;
    use tracing::{span, Event, Metadata, Subscriber};

    /// A no-op subscriber that enables every level and evaluates all event
    /// fields. `tracing` macros skip evaluating field expressions when no
    /// subscriber is installed, which would leave those expressions uncovered
    /// by tarpaulin. Installed once per process (globally) by the first test
    /// module that calls [`init_tracing`].
    #[derive(Debug, Default)]
    struct EagerNopSubscriber;

    impl Subscriber for EagerNopSubscriber {
        fn register_callsite(&self, _m: &'static Metadata<'static>) -> Interest {
            Interest::always()
        }
        fn enabled(&self, _m: &Metadata<'_>) -> bool {
            true
        }
        fn max_level_hint(&self) -> Option<LevelFilter> {
            Some(LevelFilter::TRACE)
        }
        fn new_span(&self, _s: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }
        fn record(&self, _s: &span::Id, _v: &span::Record<'_>) {}
        fn record_follows_from(&self, _s: &span::Id, _f: &span::Id) {}
        fn event(&self, _e: &Event<'_>) {}
        fn enter(&self, _s: &span::Id) {}
        fn exit(&self, _s: &span::Id) {}
        fn clone_span(&self, _s: &span::Id) -> span::Id {
            span::Id::from_u64(1)
        }
        fn try_close(&self, _s: tracing::Id) -> bool {
            true
        }
    }

    /// Install the eager no-op subscriber (idempotent).
    pub fn init_tracing() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = tracing::subscriber::set_global_default(EagerNopSubscriber);
        });
    }
}
