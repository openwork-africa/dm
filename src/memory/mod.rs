//! What dm keeps and recalls: transcripts, the job queue, the daily summary and the `remember` tool, which reads links too.

pub mod compaction;
pub mod jobs;
pub mod links;
pub(crate) mod remember;
mod summary;
mod transcript;
