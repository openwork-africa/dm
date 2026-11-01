//! Work that runs after the answer: compaction and summaries, kept in the `jobs` table so it survives the process. dm
//! runs what is due once the answer is out, before it exits; a job that fails waits, doubling from RETRY_MS, and is
//! tried again by the next dm or `dm jobs`, up to MAX_ATTEMPTS times.

use crate::{
    config::Providers,
    provider::Model,
    store::{now_ms, Store},
    summary,
    who::Who,
};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Job {
    /// Copy messages into a transcript and mark them compacted; closing a day compacts all of it. `model` is the one
    /// the day's summary will use.
    Compact {
        agent: String,
        person: String,
        day: String,
        close: bool,
        model: String,
    },
    /// Update the notes from a closed day's transcripts, with `model` (`<provider>/<model-id>`).
    Summary {
        agent: String,
        person: String,
        day: String,
        model: String,
    },
}

impl Job {
    pub fn compact(who: &Who, day: &str, close: bool, model: &str) -> Self {
        Job::Compact {
            agent: who.agent.clone(),
            person: who.person.clone(),
            day: day.into(),
            close,
            model: model.into(),
        }
    }
}

const MAX_ATTEMPTS: i64 = 5;
const RETRY_MS: i64 = 30_000;

/// What a run of the queue did.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Ran {
    pub done: usize,
    pub failed: usize,
    /// Summaries left for a run that has the providers.
    pub waiting: usize,
}

impl Store {
    pub fn enqueue(&self, job: &Job) -> Result<()> {
        self.conn.execute(
            "INSERT INTO jobs (job, run_after) VALUES (?1, ?2)",
            params![serde_json::to_string(job)?, now_ms()],
        )?;
        Ok(())
    }

    fn next_due(&self, after: i64) -> Result<Option<(i64, String, i64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, job, attempts FROM jobs WHERE run_after <= ?1 AND id > ?2 ORDER BY id LIMIT 1",
                params![now_ms(), after],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }

    fn finish(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM jobs WHERE id = ?1", [id])?;
        Ok(())
    }

    fn retry_later(&self, id: i64, attempts: i64) -> Result<()> {
        if attempts + 1 >= MAX_ATTEMPTS {
            return self.finish(id);
        }
        self.conn.execute(
            "UPDATE jobs SET attempts = attempts + 1, run_after = ?2 WHERE id = ?1",
            params![id, now_ms() + RETRY_MS * 2_i64.pow(attempts as u32)],
        )?;
        Ok(())
    }

    pub fn pending_jobs(&self) -> Result<Vec<Job>> {
        let mut query = self.conn.prepare("SELECT job FROM jobs ORDER BY id")?;
        let jobs = query
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(jobs
            .iter()
            .filter_map(|j| serde_json::from_str(j).ok())
            .collect())
    }
}

/// Runs every due job once, oldest first, with those it adds. Summaries need `providers`; without them they wait.
pub async fn run_due(
    store: &Store,
    providers: Option<&Providers>,
    http: &reqwest::Client,
) -> Result<Ran> {
    let mut ran = Ran::default();
    let mut last = 0;
    while let Some((id, text, attempts)) = store.next_due(last)? {
        last = id;
        let job: Job = match serde_json::from_str(&text) {
            Ok(job) => job,
            Err(_) => {
                store.finish(id)?;
                continue;
            }
        };
        let result = match &job {
            Job::Compact {
                agent,
                person,
                day,
                close,
                model,
            } => Who::new(agent, Some(person))
                .and_then(|who| store.compact(&who, day, *close, model)),
            Job::Summary {
                agent,
                person,
                day,
                model,
            } => {
                let Some(providers) = providers else {
                    ran.waiting += 1;
                    continue;
                };
                match (Who::new(agent, Some(person)), providers.target(Some(model))) {
                    (Ok(who), Ok(target)) => {
                        summary::update(store, &Model::new(http.clone(), target), &who, day).await
                    }
                    (Err(err), _) | (_, Err(err)) => Err(err),
                }
            }
        };
        match result {
            Ok(()) => {
                store.finish(id)?;
                ran.done += 1;
            }
            Err(err) => {
                eprintln!(
                    "dm: a {} job failed (attempt {}): {err:#}",
                    kind(&job),
                    attempts + 1
                );
                store.retry_later(id, attempts)?;
                ran.failed += 1;
            }
        }
    }
    Ok(ran)
}

fn kind(job: &Job) -> &'static str {
    match job {
        Job::Compact { .. } => "compact",
        Job::Summary { .. } => "summary",
    }
}
