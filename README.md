# dm

`dm` answers a direct message straight away. It keeps each conversation by agent, person and day, copies older
messages into transcripts, summarises them once a day into notes about the person, and gives the model a tool,
`remember`, to recall them. It offers the model the other tools it is given, too: skills that are JavaScript modules
(run under Deno, held to the hosts and environment they declare) or HTTP endpoints. It is a Rust library and a single
static binary, and it talks to Anthropic, OpenAI (chat completions and Responses), Amazon Bedrock and Azure AI Foundry.

dm itself does no web search and reads no links; `remember` searches only what dm itself has kept. A skill such as
[`web_search`](https://github.com/openwork-africa/skills) is how a model reaches the web.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/openwork-africa/dm/main/install.sh | sh
```

This picks the release archive for your system (Linux x86_64 and aarch64, static musl builds that run on any
distribution; macOS Apple silicon and Intel), checks its SHA-256 checksum, and installs `dm` to `$HOME/.local/bin`
without root. JavaScript skills need [Deno](https://deno.com) on `PATH` (or `DM_DENO`); the script says so when it is
missing, and installs nothing else. Set `DM_VERSION=v0.1.0` for a particular release, or `DM_INSTALL_DIR` for another folder. Or build it:
`cargo build --release` (stable Rust), giving `target/release/dm`.

## Calling it

Like other coding-agent harnesses, with the message as an argument:

```sh
dm -p 'what was my sister starting next month?' --output-format stream-json --model 'anthropic/claude-opus-5-5'
```

Or with one JSON request, on stdin or in a file:

```sh
dm < request.json
dm --request request.json -p        # the reply alone, as plain text
```

| flag | |
| --- | --- |
| `-p, --print [MESSAGE]` | The message to answer. Alone, it reads the request and prints plain text. |
| `--output-format stream-json\|text`, `--json` | JSON lines, or the reply alone. The default is `text` with `-p`, else `stream-json`. |
| `--request <file>` | The request JSON (else stdin, unless `-p` gives the message). |
| `--model <provider>/<model-id>` | Else the request's `model`, else the model marked `default`. |
| `--agent`, `--person`, `--time-zone`, `--system`, `--system-file` | Set the request's fields of the same name. |
| `--providers <file>` | The providers, when the request has none; else `<home>/providers.json`. |
| `--tools <file or URL>` | The tools, when the request has none; else `<home>/tools.json`; else `remember` alone. |
| `--home <dir>` | Where dm keeps its data: `$DM_HOME`, else `~/.dm` (from the OS when `HOME` is not set). |
| `--no-jobs` | Leave compaction and summaries for `dm jobs`. |

A flag wins over the request's own field. The working directory is never written to; everything stays in the home.

### The request

```json
{
  "providers": { "<id>": { "api": "...", "baseUrl": "...", "apiKey": "$ENV", "models": [ ... ] } },
  "model": "<provider>/<model-id>",
  "agent": "default",
  "person": "optional",
  "timeZone": "Africa/Johannesburg",
  "system": "optional instructions for this agent",
  "message": "the direct message",
  "lookup": { "model": "<provider>/<model-id>" },
  "tools": { "remember": { "builtin": true } }
}
```

- `model` is optional: without it, the model marked `"default": true` is used. A model id the provider does not list is
  used as given.
- `agent` defaults to `default`, the reserved name for the agent used when none is named. `person` is optional; a
  conversation with no person is kept as `_`. Both may hold letters, digits and `. _ - @ +`.
- `timeZone` decides what "today" is for the person; it defaults to this machine's (`TZ`, else `/etc/localtime`).
- `lookup` is optional, and applies only when `remember` is one of the tools. Without it, `remember` is offered on
  every message and the model decides. With it, the named model (or the answer's) is first asked whether answering
  needs something about the person or something said before: on a yes the answer is offered `remember` and told to use
  it; on a no it is not offered `remember` (the other tools still are). A failed decision is a no.
- `tools` is optional: see [Tools](#tools).

A providers file (`--providers`, or `<home>/providers.json`) has the same `providers` in msg's `models.json` shape:
`{ "providers": { ... } }`.

### Providers

Each provider names its API style in `api`, and lists its models:

| field | |
| --- | --- |
| `api` | `anthropic-messages`, `openai-completions`, `openai-responses`, `bedrock` or `azure-foundry` |
| `baseUrl` | The API's base address (optional for `bedrock`, which builds it from `region`) |
| `apiKey` | `"$NAME"`: the environment variable; `"!command"`: the command's trimmed output (`~`, `$HOME` and `$DM_HOME` work in it, also when dm was started without `HOME`); anything else as written |
| `headers` | Extra headers; values resolve like `apiKey` |
| `authHeader` | `true` sends the key as `Authorization: Bearer` whatever the style's own header |
| `region` | `bedrock`: the AWS region |
| `apiVersion` | `azure-foundry`: the `api-version` query parameter (default `2024-05-01-preview`) |
| `models[]` | `id`, `default`, `thinking`, `reasoningEffort`, `maxTokens`, `headers`, `cost` |

A `name` on a provider or model and a model's `contextWindow` (which msg's `models.json` carries) are accepted and ignored.

`cost` is US dollars per million tokens, `{ "input", "output", "cacheRead", "cacheWrite", "tiers": [{ "inputTokensAbove",
... }] }`; a call whose input is above a tier's threshold is priced at that tier. Keys are resolved once for each
answer, when its model is set up, and are never printed or logged: errors name the variable or command, never the value.

| style | sends to | key |
| --- | --- | --- |
| `anthropic-messages` | `{baseUrl}/v1/messages` | `x-api-key` |
| `openai-completions` | `{baseUrl}/chat/completions` | `Authorization: Bearer` |
| `openai-responses` | `{baseUrl}/responses` (`store: false`) | `Authorization: Bearer` |
| `bedrock` | `{baseUrl or https://bedrock-runtime.<region>.amazonaws.com}/model/<modelId>/converse-stream` | `Authorization: Bearer`, a Bedrock API key |
| `azure-foundry` | `{baseUrl}/chat/completions?api-version=<apiVersion>` | `api-key` |

Every style streams and supports tool calls. `thinking: true` turns on adaptive thinking for Anthropic and Bedrock and
encrypted reasoning for Responses; thinking blocks and reasoning items are sent back unchanged with their turn.
`reasoningEffort` becomes `output_config.effort` (Anthropic, Bedrock), `reasoning.effort` (Responses) or
`reasoning_effort` (chat completions). dm never forces a tool call, since current models reject a forced
`tool_choice`; the prompt asks for it instead. An empty system prompt is not sent at all.

Reasoning a model writes into its text inside `<think>` … `</think>` is kept out of the reply in every style: it is
not streamed as `text`, and is left out of `done.reply`, the stored message, transcripts and notes. Tags may be split
across streamed pieces. A `</think>` with no opening tag means everything before it in that call was reasoning; that
part is taken out of the reply, but what of it had already been streamed as `text` cannot be taken back. An unclosed
`<think>` is reasoning to the end. None of the styles can send unsigned reasoning back, so it is dropped.

Bedrock is signed with a Bedrock API key (`AWS_BEARER_TOKEN_BEDROCK`) as a bearer token. **AWS SigV4 signing (IAM
access keys) is not supported yet.**

Anthropic:

```json
{ "providers": { "anthropic": { "api": "anthropic-messages", "baseUrl": "https://api.anthropic.com",
    "apiKey": "$ANTHROPIC_API_KEY",
    "models": [{ "id": "claude-opus-5-5", "default": true, "thinking": true, "reasoningEffort": "medium",
                 "cost": { "input": 4, "output": 20, "cacheRead": 0.2, "cacheWrite": 5 } }] } },
  "message": "hello" }
```

OpenAI (chat completions, or `"api": "openai-responses"` for the Responses API):

```json
{ "providers": { "openai": { "api": "openai-completions", "baseUrl": "https://api.openai.com/v1",
    "apiKey": "$OPENAI_API_KEY", "models": [{ "id": "gpt-5", "reasoningEffort": "low" }] } },
  "model": "openai/gpt-5", "message": "hello" }
```

Amazon Bedrock:

```json
{ "providers": { "bedrock": { "api": "bedrock", "region": "us-east-1", "apiKey": "$AWS_BEARER_TOKEN_BEDROCK",
    "models": [{ "id": "us.anthropic.claude-opus-5-5", "maxTokens": 16000 }] } },
  "model": "bedrock/us.anthropic.claude-opus-5-5", "message": "hello" }
```

Azure AI Foundry:

```json
{ "providers": { "azure": { "api": "azure-foundry", "baseUrl": "https://<resource>.services.ai.azure.com/models",
    "apiKey": "!cat ~/.config/azure-foundry-key", "apiVersion": "2024-05-01-preview",
    "models": [{ "id": "<deployment-or-model>" }] } },
  "model": "azure/<deployment-or-model>", "message": "hello" }
```

These are placeholders: dm's repository holds no keys.

### Tools

The tools the model is offered, as JSON keyed by name, one entry per tool. dm offers only what is listed, `remember`
included. It comes from the request's `tools`, else `--tools <file or URL>`, else `<home>/tools.json`; with none of
them, dm offers `remember` alone, as it did before there were tools.

```json
{
  "remember":   { "builtin": true },
  "web_search": { "js": "https://openwork.africa/skill/web_search/index.js",
                  "sha256": "<the module's sha256, hex>",
                  "env": { "TAVILY_URL": "http://127.0.0.1:48402/tools/tavily", "TAVILY_API_KEY": "!cat \"$HOME/.dm/key\"" } },
  "some_api":   { "http": "https://example.com/api/some_api", "headers": { "x-api-key": "$SOME_API_KEY" } }
}
```

The name is the key: it is what the model is given, whatever name the skill gives itself (a different one is noted on
stderr). Values in `env` and `headers` resolve like `apiKey` (`$NAME`, `!command`, or a literal), once an answer, and
are never printed: a value that turns up in a tool's result or error is shown as `***`. An entry may also carry a
`description`, which is ignored, so an entry of a skills `index.json` can be copied in as it is.

| kind | entry | |
| --- | --- | --- |
| builtin | `{ "builtin": true }` | dm's own, run in-process. The one builtin is `remember`. |
| js | `{ "js": "<url or path>", "sha256"?, "env"? }` | A JavaScript module run under Deno. |
| http | `{ "http": "<url>", "headers"? }` | An endpoint: `GET` describes it, `POST` calls it. |

**A JavaScript skill** is one Node.js-style ES module that describes itself (`node:` imports and `npm:` packages work):

```js
export const skill = {
  name: "web_search",
  description: "Search the web ...",
  input: { type: "object", properties: { query: { type: "string" } }, required: ["query"] },  // JSON Schema
  net: ["api.tavily.com", "127.0.0.1:48402"],   // the hosts it may reach
  env: ["TAVILY_URL", "TAVILY_API_KEY"],        // the environment variables it may read
};
export default async function (input) { /* ... */ return result; }  // any JSON value
```

- The module is fetched once into `<home>/skills/<name>/<sha256>.js`, keyed by its content's hash. With `sha256` in
  the entry, only that module is run: a cached file with that hash is used without fetching, and a fetched module
  with another hash is refused (its calls answer with the error), so a changed file at the address does not run
  until the entry moves to its hash. Without `sha256`, the address is fetched for each answer.
- Its `skill` is read once a hash (kept as `<sha256>.json` beside it) by a wrapper dm writes, with no environment and
  no permissions but reading the module itself:
  `deno run --quiet --no-prompt --no-config --no-lock --allow-read=<module> <home>/skills/describe.js file://<module>`.
  `input` must be a JSON schema of `type: object`.
- A call runs `deno run --quiet --no-prompt --no-config --no-lock --allow-read=<module> --allow-net=<skill.net>
  --allow-env=<skill.env> <home>/skills/run.js file://<module>`, the input JSON on stdin and the result JSON on
  stdout (what the module prints goes to stderr). The environment is empty but for `DENO_DIR`
  (`<home>/skills/.deno`) and the declared names, whose values come from the entry's `env`; a name the entry gives
  that the module does not declare is not resolved. A call may take 30 seconds before Deno is killed, and print 1 MiB.
- Deno is `DM_DENO`, else the first `deno` on `PATH`.

**An HTTP skill** is an endpoint that describes itself: `GET <endpoint>` answers `{ "name", "description", "input" }`
(asked once an answer), and `POST <endpoint>` with the input JSON answers with the result JSON. Each call has 30
seconds and 1 MiB.

Whatever goes wrong with a tool (Deno missing, a fetch or description that fails, a hash that does not match, a refused
permission, a non-zero exit, a timeout, an answer that is not JSON) becomes its result, `"<name> failed: <why>"`, for
the model to read, and the answer goes on. A tool that cannot be set up is still offered under its name, and saying
why is all its calls do.

### The output

With `stream-json`, one JSON object per line on stdout, the answer first:

```json
{"type":"text","delta":"Her name "}
{"type":"tool_call","id":"t1","name":"remember","arguments":{"query":"sister"}}
{"type":"tool_result","id":"t1","name":"remember","content":"2026-10-30:\nPerson: my sister Lindi ..."}
{"type":"text","delta":"is Lindi."}
{"type":"done","reply":"Her name is Lindi.","model":"anthropic/claude-opus-5-5","usage":{"input":812,"output":41,"cacheRead":0,"cacheWrite":0},"costUsd":0.004068}
```

`usage` and `costUsd` cover every call made for the message, the lookup decision included. With `text`, only the
reply's text is printed, as it streams. Errors go to stderr as `dm: ...`, with exit code 1.

## How it works

- **The answer.** The system prompt is a neutral base prompt with the person's date and time and the "how to be with
  them" notes, then the request's `system`, then what `remember` is for when it is offered. The model is sent today's
  history (the day's messages not yet compacted) after it. When it calls a tool, dm runs it and asks again, up to
  four calls.
- **Transcripts.** Once 16 messages are in a day's history, all but the newest 4 are copied as they were into
  `transcripts/<agent>/<person>/<day>-<n>.md` and marked compacted, all or none. When the person writes on a new day,
  the day before is closed: everything left is compacted.
- **Summaries.** When a day is closed, a separate call (with the answer's model) reads that day's transcripts with the
  person's current notes and saves the updated notes through a `notes` tool, each note with the person's own words it
  comes from. "How to be with them" (the agent's name, how they talk, how they like you to be) goes into the prompt.
  "What you know" (their name, life, people, likes and dislikes, what matters, what is going on, what they asked you to
  remember) is written to `notes/<agent>/<person>.md` and reachable only through `remember`.
- **remember.** SQLite full-text search (FTS5) over the person's transcripts and their "what you know" notes. The index
  is built from the files when dm starts, so it always matches them.
- **Usage.** Every call's tokens and cost are counted in `dm.db` by day, agent, person, purpose (`chat`, `lookup`,
  `summary`) and model.
- **Jobs.** Compaction and summaries are SQLite jobs. dm prints the answer first, then runs any job that is due before
  it exits (unless `--no-jobs`). A job that fails is retried later, waiting longer each time, up to five attempts, by
  the next `dm` or by `dm jobs`.

## Storage

```
~/.dm/                               --home, or $DM_HOME
  dm.db                              messages, people, notes, usage, jobs (SQLite)
  providers.json                     optional: the providers when a request has none
  tools.json                         optional: the tools when a request has none
  skills/<name>/<sha256>.js, .json   JavaScript skills by content hash, and what each says it is
  skills/describe.js, run.js, .deno/ dm's Deno wrappers, and Deno's cache
  transcripts/<agent>/<person or _>/<day>-<n>.md
  notes/<agent>/<person or _>.md
```

## Commands

| command | |
| --- | --- |
| `dm` | Answer a request (above). |
| `dm jobs [--request <file> \| --providers <file>]` | Run pending compaction and summaries; prints `{"done","failed","waiting"}`. Summaries need providers and wait without them. |
| `dm history [--agent <name>] [--person <id>] [--day YYYY-MM-DD]` | A day's messages as JSON lines (default: the day they last spoke on). |
| `dm --version` | |

## Developing

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

The tests run each API style against a local mock server; no keys are needed. The tools tests need Deno on `PATH`
(and the network, once, for one `npm:` package). A `v*` tag builds the release archives
and their checksums.

| file | |
| --- | --- |
| `src/main.rs` | the command line |
| `src/config/` | the providers and models (`mod.rs`), the request, the tools JSON, key resolution, the home directory |
| `src/answer/` | one answer start to end (`mod.rs`), the tool loop (`rounds.rs`), the lookup decision, the system prompt, the output lines |
| `src/tools/` | the `Tool` trait and setting up a request's tools (`mod.rs`), one file per kind (builtin, js, http), what a skill says of itself |
| `src/memory/` | transcripts, compaction, the job queue, the daily summary, the `remember` tool |
| `src/store/` | the home and its database (`mod.rs`), split by table: messages, the two layers of notes, usage and cost |
| `src/day.rs`, `src/who.rs` | time zones, names |
| `src/provider/` | `Model` and the `Api` trait, the chat shape, the stream framing (SSE and AWS event stream), `<think>` filtering |
| `src/provider/styles/` | one file per API style |
| `src/prompts/` | the system prompt and the summary instructions |

## License

MIT
