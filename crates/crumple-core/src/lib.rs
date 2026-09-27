//! Crumple core library: quality-target planner, candidate choice, orientation and resize.

pub mod choose;
pub mod orient;
pub mod planner;
pub mod prior;
pub mod resize;

pub use choose::{choose, Candidate};
pub use orient::apply_orientation;
pub use planner::{search, Outcome, Planner, SearchConfig, Step};
pub use prior::{prior_q, CodecKind};
pub use resize::fit_within;

/// Returns the version of the `crumple-core` crate.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_cargo_pkg_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(!version().is_empty());
    }
}
