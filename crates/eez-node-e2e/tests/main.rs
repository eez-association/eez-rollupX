//! Process tests for the node roles, built as one integration-test binary.
//!
//! Each module used to be its own test target. In the release test profile
//! (thin LTO) every target re-optimized and linked the whole node stack, which
//! dominated CI's test-archive build. Nextest still runs each test in its own
//! process, so tests do not share process state. Select one former target by
//! module path, for example
//! `cargo test -p eez-node-e2e --test node_e2e -- cross_chain:: --test-threads=1`
//! or the nextest filter `test(/^cross_chain::/)`.

mod chained_interstate;
mod cross_chain;
mod derivable_operations;
mod dynamic_admission;
mod e2e;
mod fee_quotes;
mod kurtosis_policy_replay;
mod native_system;
mod partial_consumption;
mod queue_liveness;
mod queue_reorg;
mod safe_fee_quotes;
mod settlement_policy;
mod settlement_policy_cost;
mod settlement_policy_gas;
mod transaction_references;
