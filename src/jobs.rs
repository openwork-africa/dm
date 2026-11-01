//! Work that runs after the answer: compaction and summaries, kept in the `jobs` table so it survives the process. dm
//! runs what is due once the answer is out, before it exits; a job that fails waits, doubling from RETRY_MS, and is
//! tried again by the next dm or `dm jobs`, up to MAX_ATTEMPTS times.

use crate::{config::Providers, provider::Model, store::Store, summary, who::Who};
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

    pub fn summary(who: &Who, day: &str, model: &str) -> Self {
        Job::Summary {
            agent: who.agent.clone(),
            person: who.person.clone(),
            day: day.into(),
            model: model.into(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Job::Compact { .. } => "compact",
            Job::Summary { .. } => "summary",
        }
    }

    async fn run(
        &self,
        store: &Store,
        providers: Option<&Providers>,
        http: &reqwest::Client,
    ) -> Result<Outcome> {
        match self {
            Job::Compact {
                agent,
                person,
                day,
                close,
                model,
            } => store.compact(&Who::new(agent, Some(person))?, day, *close, model)?,
            Job::Summary {
                agent,
                person,
                day,
                model,
            } => {
                let Some(providers) = providers else {
                    return Ok(Outcome::Waiting);
                };
                let who = Who::new(agent, Some(person))?;
                let model = Model::new(http.clone(), providers.target(Some(model))?);
                summary::update(store, &model, &who, day).await?;
            }
        }
        Ok(Outcome::Done)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// What running a job came to.
pub enum Outcome {
    Done,
    /// A summary without providers: left for a run that has them.
    Waiting,
}

const MAX_ATTEMPTS: i64 = 5;
const RETRY_MS: i64 = 30_000;
/// How long a job that is running is hidden from other runs; a crashed run's job is due again after it.
const LEASE_MS: i64 = 600_000;

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

    /// Takes a due job for this run: false when another run took it first.
    fn claim(&self, id: i64) -> Result<bool> {
        let now = now_ms();
        Ok(self.conn.execute(
            "UPDATE jobs SET run_after = ?2 WHERE id = ?1 AND run_after <= ?3",
            params![id, now + LEASE_MS, now],
        )? == 1)
    }

    /// Gives a claimed job back, due now.
    fn release(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE jobs SET run_after = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        Ok(())
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
        let Ok(job) = serde_json::from_str::<Job>(&text) else {
            store.finish(id)?;
            continue;
        };
        if !store.claim(id)? {
            continue;
        }
        match job.run(store, providers, http).await {
            Ok(Outcome::Done) => {
                store.finish(id)?;
                ran.done += 1;
            }
            Ok(Outcome::Waiting) => {
                store.release(id)?;
                ran.waiting += 1;
            }
            Err(err) => {
                eprintln!(
                    "dm: a {} job failed (attempt {}): {err:#}",
                    job.kind(),
                    attempts + 1
                );
                store.retry_later(id, attempts)?;
                ran.failed += 1;
            }
        }
    }
    Ok(ran)
}
