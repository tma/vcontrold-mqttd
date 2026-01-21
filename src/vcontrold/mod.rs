//! vcontrold module - TCP client for vcontrold daemon

mod client;
mod protocol;

pub use client::VcontroldClient;
pub use protocol::{build_json_response, CommandResult, Value};
