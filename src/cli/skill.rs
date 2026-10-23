//! `assemblr skill install`: the coding agent's pointer to `assemblr guide`.

use std::process::ExitCode;

use assemblr::adapters::{SkillFailure, SkillOutcome, claude_skill_path, install_claude_skill};
use clap::{Args, Subcommand, ValueEnum};

use super::diagnostic::{Diagnosis, fail_with};

#[derive(Debug, Subcommand)]
pub(super) enum SkillCommand {
    /// Install the skill that points an agent to the guide
    ///
    /// Writes a skill that tells the agent when Assemblr helps and to run
    /// `assemblr guide`. Run it once per machine; running it again updates
    /// the skill. It never replaces a file that Assemblr did not write,
    /// unless told to.
    #[command(after_long_help = "Example:\n  assemblr skill install claude")]
    Install(InstallArgs),
}

#[derive(Debug, Args)]
pub(super) struct InstallArgs {
    /// The coding agent
    agent: Agent,
    /// Replace the skill's file even if Assemblr did not write it
    #[arg(long)]
    force: bool,
}

/// The coding agents whose skills Assemblr installs.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Agent {
    /// Claude Code, in `~/.claude/skills/` or under `CLAUDE_CONFIG_DIR`
    Claude,
}

pub(super) fn run(command: SkillCommand) -> ExitCode {
    match command {
        SkillCommand::Install(args) => install(&args),
    }
}

fn install(args: &InstallArgs) -> ExitCode {
    let Agent::Claude = args.agent;
    let result = claude_skill_path().and_then(|path| install_claude_skill(&path, args.force));
    match result {
        Ok(installed) => {
            let path = installed.path.display();
            match installed.outcome {
                SkillOutcome::Installed => {
                    println!("Installed the Claude Code skill in {path}.");
                    println!("Claude Code reads it in new sessions.");
                }
                SkillOutcome::Updated => println!("Updated the Claude Code skill in {path}."),
                SkillOutcome::UpToDate => {
                    println!("The Claude Code skill in {path} is up to date.");
                }
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let diagnosis = match &failure {
                SkillFailure::NotOurs(_) => Diagnosis::note("nothing was changed").advise(
                    "move the file away, or run `assemblr skill install claude --force` to \
                     replace it",
                ),
                SkillFailure::NoLocation | SkillFailure::RelativeLocation(_) => {
                    Diagnosis::note("nothing was changed").advise(
                        "set CLAUDE_CONFIG_DIR to the absolute path of Claude Code's \
                         configuration directory",
                    )
                }
                SkillFailure::Io { .. } => Diagnosis::note("the skill was not installed"),
            };
            fail_with(&failure.to_string(), &diagnosis)
        }
    }
}
