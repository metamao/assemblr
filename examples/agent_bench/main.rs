//! Gives coding agents (Claude Code) tasks that call for Assemblr, against a
//! development build, and reports how they did. See ADR 0034.
//!
//! ```text
//! cargo agent-bench [--condition <condition>]... [--runs <n>] [--jobs <n>]
//!                   [--model <model>] [--budget <usd>] [--keep] [<scenario>...]
//! cargo agent-bench --list
//! ```
//!
//! Each run builds a fresh fixture (sample units, and the project, template,
//! or catalog its scenario starts from) in a directory of its own under the
//! system's temporary directory, gives the agent the scenario's prompt with
//! a home of its own, and then checks the state the agent left. Transcripts,
//! checks, and a summary go to `target/agent-bench/<run id>/`.
//!
//! A condition is what the agent knows about Assemblr beforehand: `none`,
//! `hint` (the prompt says that Assemblr is installed), `pointer` (the
//! user-level `CLAUDE.md` says to run `assemblr guide`), or `skill` (the
//! default: `assemblr skill install claude` installed the skill).
//!
//! It costs money and needs Claude Code on `PATH`, so it never runs in CI.
//! Claude Code gets no access to the caller's home, so it authenticates with
//! `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` (from `claude
//! setup-token`).

mod fixture;
mod machine;
mod scenarios;
mod transcript;

use std::fmt::Write as _;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use machine::{AUTHENTICATION, Root, enclosing_repository};
use scenarios::{Report, SCENARIOS, Scenario};
use transcript::Metrics;

const USAGE: &str = "usage: cargo agent-bench [--condition <none|hint|pointer|skill>]... [--runs <n>] \
                     [--jobs <n>] [--model <model>] [--budget <usd>] [--keep] [<scenario>...]\n       \
                     cargo agent-bench --list";

/// What the user-level `CLAUDE.md` says in the `pointer` condition. It names
/// no command but `assemblr guide`, so it stays true across versions.
const POINTER: &str = "Assemblr (the `assemblr` CLI) is installed. It composes projects from \
                       reusable, versioned units. Before creating a project, adding a capability \
                       (auth, lint, infrastructure, configuration), updating a project that has \
                       `assemblr.jsonc`, or making something reusable across projects, run \
                       `assemblr guide` and follow it.\n";

const HINT: &str = " Assemblr está instalado.";

/// The tools the agent may use without asking, since nobody is there to
/// answer: file tools, and the commands the scenarios need.
const ALLOWED_TOOLS: &[&str] = &[
    "Read",
    "Edit",
    "Write",
    "Glob",
    "Grep",
    "Bash(assemblr:*)",
    "Bash(cd:*)",
    "Bash(git:*)",
    "Bash(ls:*)",
    "Bash(cat:*)",
    "Bash(find:*)",
    "Bash(mkdir:*)",
    "Bash(cp:*)",
    "Bash(mv:*)",
    "Bash(head:*)",
    "Bash(tail:*)",
    "Bash(pwd)",
    "Bash(which:*)",
];

/// How long one run may take before it is stopped.
const TIMEOUT: Duration = Duration::from_secs(25 * 60);

#[derive(Clone, Copy, PartialEq)]
enum Condition {
    None,
    Hint,
    Pointer,
    Skill,
}

impl Condition {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "none" => Ok(Self::None),
            "hint" => Ok(Self::Hint),
            "pointer" => Ok(Self::Pointer),
            "skill" => Ok(Self::Skill),
            _ => Err(format!(
                "unknown condition {name:?}: use none, hint, pointer, or skill"
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Hint => "hint",
            Self::Pointer => "pointer",
            Self::Skill => "skill",
        }
    }
}

struct Options {
    list: bool,
    conditions: Vec<Condition>,
    runs: usize,
    jobs: usize,
    model: Option<String>,
    budget: String,
    keep: bool,
    scenarios: Vec<&'static Scenario>,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        list: false,
        conditions: Vec::new(),
        runs: 2,
        jobs: 4,
        model: None,
        budget: "1".to_owned(),
        keep: false,
        scenarios: Vec::new(),
    };
    let number = |value: Option<String>| -> Result<usize, String> {
        value
            .and_then(|value| value.parse().ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| USAGE.to_owned())
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => options.list = true,
            "--condition" => options
                .conditions
                .push(Condition::parse(&args.next().ok_or(USAGE)?)?),
            "--runs" => options.runs = number(args.next())?,
            "--jobs" => options.jobs = number(args.next())?,
            "--model" => options.model = Some(args.next().ok_or(USAGE)?),
            "--budget" => {
                let budget = args.next().ok_or(USAGE)?;
                if budget.parse::<f64>().map_or(true, |usd| usd <= 0.0) {
                    return Err(format!(
                        "the budget {budget:?} must be a positive amount of USD"
                    ));
                }
                options.budget = budget;
            }
            "--keep" => options.keep = true,
            "-h" | "--help" => return Err(USAGE.to_owned()),
            name if !name.starts_with('-') => {
                let scenario = SCENARIOS
                    .iter()
                    .find(|scenario| scenario.name == name)
                    .ok_or_else(|| format!("unknown scenario {name:?}; `--list` lists them"))?;
                options.scenarios.push(scenario);
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    // A repeated scenario or condition would give two runs the same
    // directories; `--runs` repeats them instead.
    let mut seen = Vec::new();
    options.conditions.retain(|condition| {
        let new = !seen.contains(condition);
        seen.push(*condition);
        new
    });
    let mut seen = Vec::new();
    options.scenarios.retain(|scenario| {
        let new = !seen.contains(&scenario.name);
        seen.push(scenario.name);
        new
    });
    if options.conditions.is_empty() {
        options.conditions.push(Condition::Skill);
    }
    if options.scenarios.is_empty() {
        options.scenarios = SCENARIOS.iter().collect();
    }
    Ok(options)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("agent-bench: {message}");
            ExitCode::FAILURE
        }
    }
}

struct Job {
    scenario: &'static Scenario,
    condition: Condition,
    number: usize,
}

impl Job {
    fn id(&self) -> String {
        format!(
            "{}-{}-{}",
            self.scenario.name,
            self.condition.name(),
            self.number
        )
    }
}

struct Outcome {
    job: Job,
    report: Report,
    metrics: Metrics,
    error: Option<String>,
}

fn run() -> Result<(), String> {
    let options = parse(std::env::args().skip(1))?;
    if options.list {
        for scenario in SCENARIOS {
            println!("{:<5} {}", scenario.name, scenario.summary);
        }
        return Ok(());
    }
    let authenticated = AUTHENTICATION
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    if !authenticated {
        return Err(
            "Claude Code runs with a home of its own, so set ANTHROPIC_API_KEY or \
                    CLAUDE_CODE_OAUTH_TOKEN (create one with `claude setup-token`)"
                .to_owned(),
        );
    }
    let claude = Command::new("claude")
        .arg("--version")
        .output()
        .map_err(|error| format!("cannot run Claude Code (`claude`): {error}"))?;
    // It prints, for example, `2.1.309 (Claude Code)`.
    let claude = String::from_utf8_lossy(&claude.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("unknown")
        .to_owned();

    let binary = build()?;
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs()
        .to_string();
    let fixtures = std::env::temp_dir().join("assemblr-agent-bench").join(&id);
    if let Some(repository) = enclosing_repository(&fixtures) {
        return Err(format!(
            "{} is inside the Git repository {}; set TMPDIR to a directory outside any repository",
            fixtures.display(),
            repository.display()
        ));
    }
    let results = binary
        .parent()
        .and_then(Path::parent)
        .ok_or("cannot locate the target directory")?
        .join("agent-bench")
        .join(&id);
    fs::create_dir_all(&results)
        .map_err(|error| format!("cannot create {}: {error}", results.display()))?;
    let bin = machine::install(&binary, &fixtures.join("bin"))?;

    let mut jobs = Vec::new();
    for scenario in &options.scenarios {
        for condition in &options.conditions {
            for number in 1..=options.runs {
                jobs.push(Job {
                    scenario,
                    condition: *condition,
                    number,
                });
            }
        }
    }
    eprintln!(
        "agent-bench: {} runs with Claude Code {claude}; results in {}",
        jobs.len(),
        results.display()
    );
    jobs.reverse();
    let queue = Mutex::new(jobs);
    let outcomes = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..options.jobs {
            scope.spawn(|| {
                while let Some(job) = queue.lock().ok().and_then(|mut queue| queue.pop()) {
                    let outcome = run_job(job, &options, &bin, &fixtures, &results);
                    eprintln!("agent-bench: {}", line(&outcome));
                    if let Ok(mut outcomes) = outcomes.lock() {
                        outcomes.push(outcome);
                    }
                }
            });
        }
    });
    let mut outcomes = outcomes.into_inner().map_err(|error| error.to_string())?;
    outcomes.sort_by_key(|outcome| {
        let position = SCENARIOS
            .iter()
            .position(|scenario| scenario.name == outcome.job.scenario.name);
        (position, outcome.job.condition.name(), outcome.job.number)
    });
    let summary = summary(&outcomes, &claude, options.model.as_deref());
    let file = results.join("summary.md");
    fs::write(&file, &summary)
        .map_err(|error| format!("cannot write {}: {error}", file.display()))?;
    print!("{summary}");
    if !options.keep {
        // Best effort: what is left behind is in the temporary directory.
        let _ = fs::remove_dir_all(&fixtures);
    }
    Ok(())
}

fn run_job(job: Job, options: &Options, bin: &Path, fixtures: &Path, results: &Path) -> Outcome {
    let mut report = Report::default();
    let out = results.join(job.id());
    let result = attempt(
        &job,
        options,
        bin,
        &fixtures.join(job.id()),
        &out,
        &mut report,
    );
    let metrics = fs::read_to_string(out.join("transcript.jsonl"))
        .map(|text| transcript::metrics(&text))
        .unwrap_or_default();
    let mut checks = String::new();
    for (name, passed) in &report.checks {
        let _ = writeln!(checks, "{} {name}", if *passed { "PASS" } else { "FAIL" });
    }
    for note in &report.notes {
        let _ = writeln!(checks, "NOTE {note}");
    }
    for problem in &report.problems {
        let _ = writeln!(checks, "PROBLEM {problem}");
    }
    if let Err(error) = &result {
        let _ = writeln!(checks, "ERROR {error}");
    }
    let _ = fs::write(out.join("checks.txt"), checks);
    Outcome {
        job,
        report,
        metrics,
        error: result.err(),
    }
}

fn attempt(
    job: &Job,
    options: &Options,
    bin: &Path,
    fixture: &Path,
    out: &Path,
    report: &mut Report,
) -> Result<(), String> {
    fs::create_dir_all(out).map_err(|error| format!("cannot create {}: {error}", out.display()))?;
    let root = Root::create(fixture.to_owned(), bin)?;
    (job.scenario.setup)(&root).map_err(|error| format!("setup failed: {error}"))?;
    let agent = root.agent()?;
    if job.condition == Condition::Pointer {
        let dir = agent.home.join(".claude");
        fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        fs::write(dir.join("CLAUDE.md"), POINTER).map_err(|error| error.to_string())?;
    }
    if job.condition == Condition::Skill {
        agent
            .assemblr(&agent.work(), &["skill", "install", "claude"])
            .map_err(|error| format!("setup failed: {error}"))?;
    }
    let mut prompt = job
        .scenario
        .prompt
        .replace("{root}", machine::arg(&root.path)?);
    if job.condition == Condition::Hint {
        prompt.push_str(HINT);
    }
    fs::write(out.join("prompt.txt"), &prompt).map_err(|error| error.to_string())?;

    let create = |name: &str| {
        File::create(out.join(name)).map_err(|error| format!("cannot create {name}: {error}"))
    };
    let start = root.path.join(job.scenario.start);
    let mut command = agent.command("claude", &start)?;
    command.args([
        "-p",
        &prompt,
        "--no-session-persistence",
        "--strict-mcp-config",
    ]);
    command.args(["--max-budget-usd", &options.budget]);
    if let Some(model) = &options.model {
        command.args(["--model", model]);
    }
    for dir in job.scenario.extra_dirs {
        command.arg("--add-dir").arg(root.path.join(dir));
    }
    command.arg("--allowedTools").args(ALLOWED_TOOLS);
    command.args(["--output-format", "stream-json", "--verbose"]);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(create("transcript.jsonl")?)
        .stderr(create("stderr.txt")?)
        .spawn()
        .map_err(|error| format!("cannot run claude: {error}"))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            if !status.success() {
                report.problem(format!("claude exited with {status}"));
            }
            break;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            report.problem(format!(
                "stopped after {} minutes, at an unknown cost",
                TIMEOUT.as_secs() / 60
            ));
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    (job.scenario.verify)(&root, report)
}

/// One line for the progress of a finished run.
fn line(outcome: &Outcome) -> String {
    let passed = outcome
        .report
        .checks
        .iter()
        .filter(|(_, passed)| *passed)
        .count();
    let mut line = format!(
        "{} {passed}/{} checks, ${:.2}",
        outcome.job.id(),
        outcome.report.checks.len(),
        outcome.metrics.cost
    );
    if let Some(error) = &outcome.error {
        let _ = write!(line, ", {error}");
    }
    line
}

fn summary(outcomes: &[Outcome], claude: &str, model: Option<&str>) -> String {
    let mut text = String::from("# Agent bench\n\n");
    let models: std::collections::BTreeSet<_> = outcomes
        .iter()
        .map(|outcome| outcome.metrics.model.as_str())
        .filter(|model| !model.is_empty())
        .collect();
    let models = models.into_iter().collect::<Vec<_>>().join(", ");
    let _ = writeln!(
        text,
        "Claude Code {claude}, model {}.\n",
        if models.is_empty() {
            model.unwrap_or("unknown")
        } else {
            &models
        }
    );
    text.push_str(
        "| Scenario | Condition | All checks | Checks | Ran guide | Assemblr commands (failed) | Cost |\n\
         |---|---|---|---|---|---|---|\n",
    );
    let mut groups: Vec<(&str, &str)> = outcomes
        .iter()
        .map(|outcome| (outcome.job.scenario.name, outcome.job.condition.name()))
        .collect();
    groups.dedup();
    for (scenario, condition) in groups {
        let group: Vec<_> = outcomes
            .iter()
            .filter(|outcome| {
                outcome.job.scenario.name == scenario && outcome.job.condition.name() == condition
            })
            .collect();
        let all = group.iter().filter(|outcome| all_passed(outcome)).count();
        let passed: usize = group
            .iter()
            .map(|outcome| {
                outcome
                    .report
                    .checks
                    .iter()
                    .filter(|(_, passed)| *passed)
                    .count()
            })
            .sum();
        let checks: usize = group
            .iter()
            .map(|outcome| outcome.report.checks.len())
            .sum();
        let guide = group
            .iter()
            .filter(|outcome| outcome.metrics.ran_guide)
            .count();
        let commands: usize = group.iter().map(|outcome| outcome.metrics.assemblr).sum();
        let failed: usize = group.iter().map(|outcome| outcome.metrics.failed).sum();
        let cost: f64 = group.iter().map(|outcome| outcome.metrics.cost).sum();
        let runs = group.len();
        let _ = writeln!(
            text,
            "| {scenario} | {condition} | {all}/{runs} | {passed}/{checks} | {guide}/{runs} | \
             {commands} ({failed}) | ${cost:.2} |"
        );
    }
    let total: f64 = outcomes.iter().map(|outcome| outcome.metrics.cost).sum();
    let _ = writeln!(text, "\nTotal cost: ${total:.2}.\n");

    let notable: Vec<_> = outcomes
        .iter()
        .filter(|outcome| {
            !all_passed(outcome)
                || !outcome.metrics.unknown.is_empty()
                || outcome.metrics.internal_reads > 0
                || outcome.metrics.denials > 0
                || !outcome.metrics.finished
                || outcome.metrics.ended.is_some()
                || !outcome.report.problems.is_empty()
        })
        .collect();
    if !notable.is_empty() {
        text.push_str("## Runs to look at\n\n");
        for outcome in notable {
            let mut findings = Vec::new();
            let failed: Vec<_> = outcome
                .report
                .checks
                .iter()
                .filter(|(_, passed)| !passed)
                .map(|(name, _)| *name)
                .collect();
            if !failed.is_empty() {
                findings.push(format!("failed {}", failed.join(", ")));
            }
            if let Some(error) = &outcome.error {
                findings.push(error.clone());
            }
            findings.extend(outcome.report.problems.iter().cloned());
            if !outcome.metrics.finished {
                findings.push("no result event".to_owned());
            }
            if let Some(ended) = &outcome.metrics.ended {
                findings.push(format!("Claude Code ended with {ended}"));
            }
            if !outcome.metrics.unknown.is_empty() {
                let unknown: Vec<_> = outcome.metrics.unknown.iter().map(String::as_str).collect();
                findings.push(format!("tried `{}`", unknown.join("`, `")));
            }
            if outcome.metrics.internal_reads > 0 {
                findings.push(format!(
                    "{} reads of Assemblr's state",
                    outcome.metrics.internal_reads
                ));
            }
            if outcome.metrics.denials > 0 {
                findings.push(format!("{} permission denials", outcome.metrics.denials));
            }
            let _ = writeln!(text, "- {}: {}.", outcome.job.id(), findings.join("; "));
        }
    }
    text
}

fn all_passed(outcome: &Outcome) -> bool {
    outcome.error.is_none()
        && !outcome.report.checks.is_empty()
        && outcome.report.checks.iter().all(|(_, passed)| *passed)
}

/// Builds the `assemblr` binary of this checkout and returns its path. This
/// example is built in `examples/` next to it, in the same profile.
fn build() -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--quiet", "--bin", "assemblr"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if !status.success() {
        return Err("building assemblr failed".to_owned());
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let binary = exe
        .parent()
        .and_then(Path::parent)
        .ok_or("cannot locate the build directory")?
        .join(format!("assemblr{}", std::env::consts::EXE_SUFFIX));
    if !binary.is_file() {
        return Err(format!("no binary at {}", binary.display()));
    }
    Ok(binary)
}
