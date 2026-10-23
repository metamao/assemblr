//! The agent bench's fixtures and checks, without an agent (ADR 0034).
//!
//! The bench itself runs Claude Code and never runs in CI. This test builds
//! every scenario's starting state with the binary under test, so that a
//! change to the CLI that breaks a scenario fails here rather than before a
//! release, and checks that each scenario's checks fail on that starting
//! state, so that a pass means the agent did the work.

// The bench, like the agent it runs, uses a Unix shell environment.
#![cfg(unix)]
#![allow(
    dead_code,
    reason = "the bench's modules are shared with the example, which uses all of them"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a fixture that cannot be built fails the test"
)]

mod common;
#[path = "../examples/agent_bench/fixture.rs"]
mod fixture;
#[path = "../examples/agent_bench/machine.rs"]
mod machine;
#[path = "../examples/agent_bench/scenarios.rs"]
mod scenarios;
#[path = "../examples/agent_bench/transcript.rs"]
mod transcript;

use std::path::Path;

use machine::Root;
use scenarios::{Report, SCENARIOS};

#[test]
fn every_scenario_sets_up_and_fails_its_checks_before_the_agent_acts() {
    let dir = common::scratch("scenarios");
    let bin =
        machine::install(Path::new(env!("CARGO_BIN_EXE_assemblr")), &dir.join("bin")).unwrap();
    std::thread::scope(|scope| {
        for scenario in SCENARIOS {
            let (dir, bin) = (&dir, &bin);
            scope.spawn(move || {
                let root = Root::create(dir.join(scenario.name), bin).unwrap();
                (scenario.setup)(&root)
                    .unwrap_or_else(|error| panic!("{}: {error}", scenario.name));
                assert!(
                    root.path.join(scenario.start).is_dir(),
                    "{}: the agent's start directory is missing",
                    scenario.name
                );
                let mut report = Report::default();
                (scenario.verify)(&root, &mut report)
                    .unwrap_or_else(|error| panic!("{}: {error}", scenario.name));
                assert!(
                    report.checks.iter().any(|(_, passed)| !passed),
                    "{}: every check passes before the agent acts: {:?}",
                    scenario.name,
                    report.checks
                );
            });
        }
    });
}

#[test]
fn scenario_names_are_unique() {
    let mut names: Vec<_> = SCENARIOS.iter().map(|scenario| scenario.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), SCENARIOS.len());
}
