//! `assemblr skill install`: the coding agent's pointer to `assemblr guide`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a fixture that cannot be built fails the test"
)]

mod common;

use std::fs;
use std::path::PathBuf;

use common::{Fixture, fails, stderr, stdout, succeeds};

fn skill(fixture: &Fixture) -> PathBuf {
    // Joined by part, as the binary prints it on every platform.
    fixture
        .home
        .join(".claude")
        .join("skills")
        .join("assemblr")
        .join("SKILL.md")
}

#[test]
fn install_writes_the_skill_once_and_reports_it_up_to_date_after() {
    let fixture = Fixture::new("install_once");

    let output = succeeds(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude"])
            .output()
            .unwrap(),
    );
    let path = skill(&fixture);
    assert!(
        stdout(&output).contains(&format!(
            "Installed the Claude Code skill in {}.",
            path.display()
        )),
        "{}",
        stdout(&output)
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("---\nname: assemblr\ndescription: Use when"),
        "{text}"
    );
    assert!(
        text.contains("run `assemblr guide` and follow it"),
        "{text}"
    );

    let output = succeeds(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude"])
            .output()
            .unwrap(),
    );
    assert!(
        stdout(&output).contains("is up to date"),
        "{}",
        stdout(&output)
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn install_updates_a_skill_that_an_earlier_version_wrote() {
    let fixture = Fixture::new("install_update");
    let path = skill(&fixture);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "---\nname: assemblr\n---\n\nAn older pointer.\n\n\
         <!-- assemblr-skill -->\n",
    )
    .unwrap();

    let output = succeeds(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude"])
            .output()
            .unwrap(),
    );

    assert!(
        stdout(&output).contains("Updated the Claude Code skill"),
        "{}",
        stdout(&output)
    );
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("`assemblr guide`"), "{text}");
    let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "only SKILL.md remains: {leftovers:?}");
}

#[test]
fn install_keeps_a_skill_assemblr_did_not_write_unless_forced() {
    let fixture = Fixture::new("install_foreign");
    let path = skill(&fixture);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "My own notes about Assemblr.\n").unwrap();

    let output = fails(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude"])
            .output()
            .unwrap(),
    );
    let message = stderr(&output);
    assert!(message.contains("Assemblr did not write it"), "{message}");
    assert!(message.contains("note: nothing was changed"), "{message}");
    assert!(
        message.contains("`assemblr skill install claude --force`"),
        "{message}"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "My own notes about Assemblr.\n"
    );

    let output = succeeds(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude", "--force"])
            .output()
            .unwrap(),
    );
    assert!(
        stdout(&output).contains("Updated the Claude Code skill"),
        "{}",
        stdout(&output)
    );
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("`assemblr guide`")
    );
}

#[test]
fn install_keeps_a_directory_at_the_skill_s_path() {
    let fixture = Fixture::new("install_directory");
    fs::create_dir_all(skill(&fixture)).unwrap();

    let output = fails(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude", "--force"])
            .output()
            .unwrap(),
    );

    let message = stderr(&output);
    assert!(message.contains("SKILL.md is a directory"), "{message}");
    assert!(message.contains("move the directory away"), "{message}");
    assert!(skill(&fixture).is_dir());
}

#[cfg(unix)]
#[test]
fn install_does_not_write_through_a_link_unless_forced() {
    let fixture = Fixture::new("install_link");
    let path = skill(&fixture);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let target = fixture.dir.join("dotfiles-skill.md");
    fs::write(&target, "kept\n").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();

    let output = fails(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude"])
            .output()
            .unwrap(),
    );
    assert!(
        stderr(&output).contains("SKILL.md is a symbolic link"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "kept\n");

    succeeds(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude", "--force"])
            .output()
            .unwrap(),
    );
    assert!(
        !fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "kept\n");
}

#[test]
fn install_refuses_a_relative_claude_config_dir() {
    let fixture = Fixture::new("install_relative");

    let output = fails(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude"])
            .env("CLAUDE_CONFIG_DIR", "claude-config")
            .output()
            .unwrap(),
    );

    let message = stderr(&output);
    assert!(
        message.contains("CLAUDE_CONFIG_DIR is the relative path"),
        "{message}"
    );
    assert!(!fixture.dir.join("claude-config").exists());
}

/// Without `CLAUDE_CONFIG_DIR`, the skill goes in `~/.claude`. Only on Unix,
/// where the home directory is `HOME`: elsewhere it is the real user's.
#[cfg(unix)]
#[test]
fn install_defaults_to_the_home_directory() {
    let fixture = Fixture::new("install_home");

    succeeds(
        fixture
            .command(&fixture.dir, &["skill", "install", "claude"])
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .unwrap(),
    );

    assert!(skill(&fixture).is_file());
}

#[test]
fn install_accepts_only_known_agents() {
    let fixture = Fixture::new("install_unknown_agent");

    let output = fixture
        .command(&fixture.dir, &["skill", "install", "codex"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2), "a usage error");

    assert!(
        stderr(&output).contains("[possible values: claude]"),
        "{}",
        stderr(&output)
    );
    assert!(!fixture.home.join(".claude").exists());
}
