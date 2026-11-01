//! dm answers a direct message straight away, keeps transcripts, summarises them, gives the model a `remember` tool
//! to recall them, and offers it the tools it is given (builtin, JavaScript under Deno, HTTP). See README.md.

pub mod answer;
pub mod config;
pub mod day;
pub mod memory;
pub mod provider;
pub mod store;
pub mod tools;
pub mod who;

pub use answer as respond;
pub use answer::output;
pub use config::{home, request};
pub use memory::{compaction, jobs};

pub use answer::{respond, Answered};
pub use provider::chat::Usage;
pub use request::Request;
pub use store::{messages::StoredMessage, Store};
