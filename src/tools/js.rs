//! A skill that is one Node-style ES module, run under Deno, which holds it to the hosts and environment variables it
//! declares. It exports `skill` (`{ name, description, input, net?, env? }`) and a default `async function (input)`.
//!
//! The module is kept at `<home>/skills/<name>/<sha256>.js`, by its content's hash, and fetched only when that file is
//! not there; with `sha256` pinned, a module with any other hash is refused. Its `skill` is read once a hash, by
//! `describe.js` with no permissions but reading the module itself, and kept beside it as `<sha256>.json`. A call runs
//! `run.js` with only what `skill` declares (`--allow-net`, `--allow-env`), the input on stdin and the result on stdout,
//! in an empty environment but for the declared names, within TIMEOUT and MAX_BYTES.

use super::{
    capped, described::Described, json_text, Call, Resolved, Setup, Tool, MAX_BYTES, TIMEOUT,
};
use crate::{
    config::{home, tools::is_url},
    provider::chat::ToolSpec,
};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

/// Prints the module's `skill` as JSON.
const DESCRIBE: &str = r#"// dm: prints a skill module's `skill` as JSON. What the module itself prints goes to stderr.
const out = (value) => new Response(JSON.stringify(value ?? null)).body.pipeTo(Deno.stdout.writable);
for (const name of ["log", "info", "debug"]) console[name] = console.error;
try {
  await out((await import(Deno.args[0])).skill);
} catch (error) {
  console.error(`error: ${error?.name ?? "Error"}: ${error?.message ?? error}`);
  Deno.exit(1);
}
"#;

/// Calls the module's default export with the input JSON on stdin, and prints what it returns as JSON.
const RUN: &str = r#"// dm: calls a skill module's default export with the JSON on stdin and prints its result as JSON. What the module
// itself prints goes to stderr.
const out = (value) => new Response(JSON.stringify(value ?? null)).body.pipeTo(Deno.stdout.writable);
for (const name of ["log", "info", "debug"]) console[name] = console.error;
try {
  const skill = await import(Deno.args[0]);
  const input = JSON.parse((await new Response(Deno.stdin.readable).text()) || "{}");
  await out(await skill.default(input));
} catch (error) {
  console.error(`error: ${error?.name ?? "Error"}: ${error?.message ?? error}`);
  Deno.exit(1);
}
"#;

pub struct Js {
    spec: ToolSpec,
    deno: Deno,
    /// `--allow-net` and `--allow-env`, with what the module declares.
    allow: Vec<String>,
    env: Resolved,
}

impl Js {
    pub async fn open(
        name: &str,
        source: &str,
        pinned: Option<&str>,
        env: &BTreeMap<String, String>,
        setup: &Setup<'_>,
    ) -> Result<Box<dyn Tool>> {
        let skills = setup.store.skills_dir();
        let deno = Deno {
            exe: deno()?,
            module: fetch(&skills.join(name), source, pinned, setup.http).await?,
            skills,
        };
        for (wrapper, text) in [("describe.js", DESCRIBE), ("run.js", RUN)] {
            write_if_changed(&deno.skills.join(wrapper), text.as_bytes())?;
        }
        let described = deno.describe().await?;
        let allow = [("net", &described.net), ("env", &described.env)]
            .into_iter()
            .filter(|(_, items)| !items.is_empty())
            .map(|(what, items)| format!("--allow-{what}={}", items.join(",")))
            .collect();
        // Only what the module declares is resolved and given to it.
        let declared = env
            .iter()
            .filter(|(key, _)| described.env.contains(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Ok(Box::new(Self {
            spec: described.spec(name),
            deno,
            allow,
            env: Resolved::of(&declared)?,
        }))
    }
}

impl Tool for Js {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call<'a>(&'a self, input: &'a Value) -> Call<'a> {
        Box::pin(async move {
            let input = input.to_string();
            let printed = self
                .deno
                .run("run.js", &self.allow, &self.env, Some(input.as_bytes()))
                .await?;
            Ok(self.env.redact(&json_text(&printed, &self.spec.name)?))
        })
    }
}

/// Deno, on one module in the cache.
struct Deno {
    exe: PathBuf,
    skills: PathBuf,
    module: PathBuf,
}

impl Deno {
    /// The module's `skill`, read with no environment and no permissions, once for each hash.
    async fn describe(&self) -> Result<Described> {
        let kept = self.module.with_extension("json");
        if let Ok(described) = fs::read(&kept)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Described::parse(&bytes, "its kept skill"))
        {
            return Ok(described);
        }
        let none = Resolved { values: vec![] };
        let printed = self
            .run("describe.js", &[], &none, None)
            .await
            .context("its skill could not be read")?;
        let described = Described::parse(&printed, "its skill")?;
        write_if_changed(&kept, serde_json::to_string(&described)?.as_bytes())?;
        Ok(described)
    }

    /// A wrapper on the module, allowed to read the module, what `allow` adds and nothing else, with `env` alone.
    async fn run(
        &self,
        wrapper: &str,
        allow: &[String],
        env: &Resolved,
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let module = reqwest::Url::from_file_path(&self.module)
            .map_err(|()| anyhow!("{} is not an absolute path", self.module.display()))?;
        let mut command = Command::new(&self.exe);
        command
            .args(["run", "--quiet", "--no-prompt", "--no-config", "--no-lock"])
            .arg(format!("--allow-read={}", self.module.display()))
            .args(allow)
            .arg(self.skills.join(wrapper))
            .arg(module.as_str())
            .current_dir(&self.skills)
            .env_clear()
            .env("DENO_DIR", self.skills.join(".deno"))
            .env("DENO_NO_UPDATE_CHECK", "1")
            .env("NO_COLOR", "1")
            .envs(
                env.values
                    .iter()
                    .map(|(name, value)| (name, value.expose())),
            )
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Past the timeout the child is dropped, and so killed.
        let (status, out, err) = tokio::time::timeout(TIMEOUT, async {
            let mut child = command.spawn().context("Deno could not be started")?;
            if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
                stdin.write_all(input).await?;
            }
            let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
            let (out, err) = tokio::join!(
                async {
                    let out = read_capped(stdout).await;
                    if out.len() > MAX_BYTES {
                        let _ = child.start_kill();
                    }
                    out
                },
                read_capped(stderr)
            );
            anyhow::Ok((child.wait().await?, out, err))
        })
        .await
        .map_err(|_| {
            anyhow!(
                "it took longer than {} seconds and was stopped",
                TIMEOUT.as_secs()
            )
        })??;
        if out.len() > MAX_BYTES {
            bail!("it printed more than {} KiB", MAX_BYTES / 1024);
        }
        if !status.success() {
            bail!("{}", env.redact(&reason(&err, status)));
        }
        Ok(out)
    }
}

/// Deno: `DM_DENO`, else the first on PATH.
fn deno() -> Result<PathBuf> {
    const MISSING: &str =
        "JavaScript skills run under Deno, and none was found: install it (https://deno.com) or set DM_DENO to it";
    if let Some(given) = std::env::var_os("DM_DENO") {
        let path = home::expand(Path::new(&given));
        return path
            .is_file()
            .then_some(path)
            .ok_or_else(|| anyhow!("{MISSING}; DM_DENO names no file"));
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("deno"))
                .find(|p| p.is_file())
        })
        .ok_or_else(|| anyhow!(MISSING))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The module's file in the cache, fetched from `source` (an address or a path) when it is not there yet.
async fn fetch(
    dir: &Path,
    source: &str,
    pinned: Option<&str>,
    http: &reqwest::Client,
) -> Result<PathBuf> {
    let file = |hash: &str| dir.join(format!("{hash}.js"));
    if let Some(hash) = pinned {
        if fs::read(file(hash)).is_ok_and(|kept| sha256_hex(&kept) == hash) {
            return Ok(file(hash));
        }
    }
    let bytes = if is_url(source) {
        let response = http
            .get(source)
            .timeout(TIMEOUT)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .with_context(|| format!("{source} could not be fetched"))?;
        capped(response).await?
    } else {
        let path = home::expand(Path::new(source));
        fs::read(&path).with_context(|| format!("could not read {}", path.display()))?
    };
    let hash = sha256_hex(&bytes);
    if let Some(pinned) = pinned.filter(|pinned| *pinned != hash) {
        bail!("the module at {source} is not the one pinned (sha256 {hash}, not {pinned}), so it is not run");
    }
    write_if_changed(&file(&hash), &bytes)?;
    Ok(file(&hash))
}

/// Writes through a part file renamed over the old, so a reader never sees half a file.
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if fs::read(path).is_ok_and(|there| there == bytes) {
        return Ok(());
    }
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder", path.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("could not make {}", dir.display()))?;
    let part = path.with_extension(format!("part-{}", std::process::id()));
    fs::write(&part, bytes)
        .and_then(|()| fs::rename(&part, path))
        .with_context(|| format!("could not write {}", path.display()))
}

/// Up to one byte past MAX_BYTES, so too much can be told from just enough.
async fn read_capped(stream: Option<impl AsyncRead + Unpin>) -> Vec<u8> {
    let mut bytes = vec![];
    if let Some(stream) = stream {
        let _ = stream
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await;
    }
    bytes
}

/// Why Deno stopped: the wrappers' `error:` line, else its last line, else its exit status.
fn reason(stderr: &[u8], status: std::process::ExitStatus) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let line = lines
        .iter()
        .rev()
        .find(|l| l.starts_with("error:"))
        .or(lines.last());
    match line {
        Some(line) => line
            .trim_start_matches("error:")
            .trim()
            .chars()
            .take(500)
            .collect(),
        None => format!("Deno exited with {status}"),
    }
}
