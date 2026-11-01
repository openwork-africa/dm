use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use dm::{
    config::Providers,
    day, jobs,
    output::{sink_for, Format},
    request::{read_providers, Request, DEFAULT_AGENT},
    who::Who,
    Store,
};
use std::{
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
    process::ExitCode,
};

/// Answers a direct message straight away, keeps transcripts, summarises them, and lets the model recall them.
#[derive(Parser)]
#[command(name = "dm", version, args_conflicts_with_subcommands = true)]
struct Cli {
    /// Where dm keeps its data [default: $DM_HOME, else ~/.dm]
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    ask: Ask,
}

#[derive(Args)]
struct Ask {
    /// The message to answer. Given alone (`dm -p`), the request is read and the reply printed as plain text
    #[arg(short = 'p', long = "print", num_args = 0..=1, value_name = "MESSAGE")]
    print: Option<Option<String>>,
    /// The request JSON [default: stdin, unless -p gives the message]
    #[arg(long)]
    request: Option<PathBuf>,
    /// stream-json: JSON lines; text: the reply alone [default: text with -p, else stream-json]
    #[arg(long, value_enum)]
    output_format: Option<Format>,
    /// The same as --output-format stream-json
    #[arg(long)]
    json: bool,
    /// <provider>/<model-id>
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    agent: Option<String>,
    #[arg(long)]
    person: Option<String>,
    /// An IANA time zone [default: this machine's]
    #[arg(long)]
    time_zone: Option<String>,
    /// The agent's instructions
    #[arg(long, conflicts_with = "system_file")]
    system: Option<String>,
    /// A file holding the agent's instructions
    #[arg(long)]
    system_file: Option<PathBuf>,
    #[command(flatten)]
    providers: ProvidersArg,
    /// Leave compaction and summaries for `dm jobs` instead of running them before exit
    #[arg(long)]
    no_jobs: bool,
}

#[derive(Args)]
struct ProvidersArg {
    /// A providers file in msg's models.json shape [default: <home>/providers.json]
    #[arg(long = "providers")]
    path: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Run pending compaction and summaries
    Jobs {
        /// A request whose providers the summaries use
        #[arg(long)]
        request: Option<PathBuf>,
        #[command(flatten)]
        providers: ProvidersArg,
    },
    /// Print a day's messages as JSON lines
    History {
        #[arg(long, default_value = DEFAULT_AGENT)]
        agent: String,
        #[arg(long)]
        person: Option<String>,
        /// YYYY-MM-DD [default: the day they last spoke on]
        #[arg(long)]
        day: Option<String>,
    },
}

fn home(given: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(home) = given.or_else(|| std::env::var_os("DM_HOME").map(PathBuf::from)) {
        return Ok(home);
    }
    let user = std::env::var_os("HOME").ok_or_else(|| anyhow!("set --home, DM_HOME or HOME"))?;
    Ok(PathBuf::from(user).join(".dm"))
}

fn read_request(path: &Path) -> Result<Request> {
    Request::parse(
        &std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?,
    )
}

fn read_stdin() -> Result<Request> {
    if std::io::stdin().is_terminal() {
        bail!("give the message with -p '<message>', or a request JSON on stdin or with --request");
    }
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    Request::parse(&text)
}

impl ProvidersArg {
    /// The file named, else the home's providers.json when there is one.
    fn load(&self, home: &Path) -> Result<Option<Providers>> {
        match &self.path {
            Some(path) => read_providers(path).map(Some),
            None => {
                let default = home.join("providers.json");
                if default.exists() {
                    read_providers(&default).map(Some)
                } else {
                    Ok(None)
                }
            }
        }
    }
}

impl Ask {
    fn format(&self) -> Format {
        match self.output_format {
            Some(format) => format,
            None if self.json => Format::StreamJson,
            None if self.print.is_some() => Format::Text,
            None => Format::StreamJson,
        }
    }

    /// The request, with what the flags say over it.
    fn request(&self, home: &Path) -> Result<Request> {
        let message = self.print.clone().flatten();
        let mut request = match (&self.request, &message) {
            (Some(path), _) => read_request(path)?,
            (None, Some(_)) => Request::default(),
            (None, None) => read_stdin()?,
        };
        let set = |field: &mut Option<String>, value: &Option<String>| {
            if value.is_some() {
                field.clone_from(value);
            }
        };
        set(&mut request.message, &message);
        set(&mut request.model, &self.model);
        set(&mut request.agent, &self.agent);
        set(&mut request.person, &self.person);
        set(&mut request.time_zone, &self.time_zone);
        set(&mut request.system, &self.system);
        if let Some(path) = &self.system_file {
            request.system = Some(
                std::fs::read_to_string(path)
                    .with_context(|| format!("could not read {}", path.display()))?,
            );
        }
        if request.providers.0.is_empty() {
            request.providers = self.providers.load(home)?.ok_or_else(|| {
                anyhow!(
                    "no providers: put them in the request, pass --providers <file>, or write {}",
                    home.join("providers.json").display()
                )
            })?;
        }
        Ok(request)
    }
}

async fn run(cli: Cli) -> Result<()> {
    let home = home(cli.home)?;
    let http = reqwest::Client::new();
    match cli.command {
        None => {
            let ask = cli.ask;
            let request = ask.request(&home)?;
            let mut store = Store::open(&home)?;
            dm::respond(&store, &http, &request, sink_for(ask.format()).as_mut()).await?;
            if !ask.no_jobs {
                jobs::run_due(&mut store, Some(&request.providers), &http).await?;
            }
        }
        Some(Command::Jobs { request, providers }) => {
            let providers = match request {
                Some(path) => Some(read_request(&path)?.providers),
                None => providers.load(&home)?,
            };
            let mut store = Store::open(&home)?;
            let ran = jobs::run_due(&mut store, providers.as_ref(), &http).await?;
            println!("{}", serde_json::to_string(&ran)?);
        }
        Some(Command::History { agent, person, day }) => {
            let who = Who::new(&agent, person.as_deref())?;
            let store = Store::open(&home)?;
            let day = match day {
                Some(day) if day::is_day(&day) => day,
                Some(day) => bail!("`{day}` is not a day: give YYYY-MM-DD"),
                None => match store.last_day(&who)? {
                    Some(day) => day,
                    None => return Ok(()),
                },
            };
            for message in store.messages(&who, &day, true)? {
                println!("{}", serde_json::to_string(&message)?);
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("dm: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dm: {err:#}");
            ExitCode::FAILURE
        }
    }
}
