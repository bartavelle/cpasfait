//! Core of cpasfait: task model, op-log, local storage and sync.
//!
//! This crate has no UI dependencies; it is shared by the TUI, the companion
//! tools and (later) the mobile app through UniFFI bindings.

mod apply;
pub mod backend;
pub mod dates;
pub mod error;
pub mod hlc;
pub mod ids;
pub mod model;
pub mod op;
pub mod paths;
pub mod position;
pub mod repo;
pub mod report;
mod schema;
pub mod sync;
pub mod tree;

pub use error::{Error, Result};
pub use ids::{DeviceId, SessionId, TabId, TaskId};
pub use model::{DepInfo, DepState, Dependencies, RemoteConfig, Session, TabInfo, Task};
pub use op::{Change, Op, Status};
pub use repo::{Place, Repo};
pub use tree::{Row, Tree};
