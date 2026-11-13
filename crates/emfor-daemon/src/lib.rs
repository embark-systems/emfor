//! emfor daemon library: config, checkers, health state, scheduling, and the
//! LMDB state store. The `emfor` binary is a thin wrapper around [`cli`].

pub mod checker;
pub mod cli;
pub mod config;
pub mod dns;
pub mod live;
pub mod metrics;
pub mod peers;
pub mod plugin;
pub mod schedule;
pub mod select;
pub mod state;
pub mod status;
pub mod store;
pub mod supervisor;
pub mod writer;
