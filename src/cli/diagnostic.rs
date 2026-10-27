//! How every command reports a failure on stderr, as
//! `docs/user-facing-copy.md` describes: `error:` says what failed, `note:`
//! what was or was not changed, and `hint:` what to do next.

use std::process::ExitCode;

/// What follows an error: the state it left, and the way forward.
#[derive(Debug, Default)]
pub(super) struct Diagnosis {
    note: Option<String>,
    hint: Option<String>,
}

impl Diagnosis {
    /// A diagnosis that states what was or was not changed.
    pub(super) fn note(note: impl Into<String>) -> Self {
        Self {
            note: Some(note.into()),
            hint: None,
        }
    }

    /// A diagnosis that only says what to do.
    pub(super) fn hint(hint: impl Into<String>) -> Self {
        Self {
            note: None,
            hint: Some(hint.into()),
        }
    }

    /// Adds what to do; several pieces of advice are joined in order.
    pub(super) fn advise(mut self, advice: impl Into<String>) -> Self {
        let advice = advice.into();
        self.hint = Some(match self.hint.take() {
            Some(hint) => format!("{hint}; {advice}"),
            None => advice,
        });
        self
    }

    /// Puts `before` ahead of what the diagnosis says was or was not
    /// changed.
    pub(super) fn noting_first(mut self, before: &str) -> Self {
        self.note = Some(match self.note.take() {
            Some(note) => format!("{before}; {note}"),
            None => before.to_owned(),
        });
        self
    }

    /// Adds what to do, when there is something.
    pub(super) fn advise_if(self, advice: Option<impl Into<String>>) -> Self {
        match advice {
            Some(advice) => self.advise(advice),
            None => self,
        }
    }
}

/// Reports `message` with `diagnosis` and returns the failure exit code.
pub(super) fn fail_with(message: &str, diagnosis: &Diagnosis) -> ExitCode {
    eprintln!("error: {}", message.replace('\n', "\nerror: "));
    if let Some(note) = &diagnosis.note {
        eprintln!("note: {note}");
    }
    if let Some(hint) = &diagnosis.hint {
        eprintln!("hint: {hint}");
    }
    ExitCode::FAILURE
}

/// Reports `message` with an optional hint and returns the failure exit
/// code.
pub(super) fn fail(message: &str, hint: Option<&str>) -> ExitCode {
    fail_with(
        message,
        &Diagnosis {
            note: None,
            hint: hint.map(str::to_owned),
        },
    )
}

/// `count` followed by `singular` or `plural`.
pub(super) fn count(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}
