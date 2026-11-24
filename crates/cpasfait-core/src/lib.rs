//! Core of cpasfait: task model, op-log, local storage and sync.
//!
//! This crate has no UI dependencies; it is shared by the TUI, the companion
//! tools and (later) the mobile app through UniFFI bindings.

mod apply;
pub mod backend;
pub mod config;
pub mod crypto;
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
pub mod score;
pub mod sync;
pub mod taskfile;
pub mod tree;

pub use error::{Error, Result};
pub use ids::{DeviceId, SessionId, TabId, TaskId};
pub use model::{
    Author, DepInfo, DepState, Dependencies, Epoch, Member, RemoteConfig, RemoteMove, Session, SettingInfo,
    TAB_SETTINGS, TabInfo, TabSettings, Task,
};
pub use op::{Change, Op, Status};
pub use repo::{Moved, Place, Repo};
pub use tree::{Row, Tree};
