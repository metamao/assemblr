//! The `README.md` and `AGENTS.md` that `init` and `unit init` write when
//! they are missing (owner decision of 2026-10-23, #115).
//!
//! `AGENTS.md` points a coding agent to `assemblr guide`, the measured
//! pointer, so that it holds no instructions that a later Assemblr could
//! make wrong. A unit's files add sections for the author to
//! fill in with what only they know.

use crate::domain::{Description, UnitIdentity};

/// The project's own `AGENTS.md`.
pub(crate) const PROJECT_AGENTS: &str = "# AGENTS.md\n\
\n\
This project is composed with Assemblr (the `assemblr` CLI) from reusable, versioned units \
declared in `assemblr.jsonc`. Before adding a capability (auth, lint, infrastructure, \
configuration), updating the units, or making something reusable across projects, run \
`assemblr guide` and follow it.\n";

/// A new unit's `README.md`.
pub(crate) fn unit_readme(identity: &UnitIdentity, description: &Description) -> String {
    format!(
        "# {identity}\n\
         \n\
         {description}\n\
         \n\
         ## What it provides\n\
         \n\
         <!-- The files it delivers under src/, and the files of its dependencies it amends. -->\n\
         \n\
         ## Placeholders\n\
         \n\
         <!-- Each placeholder its files hold: the value the project writes in its place. -->\n\
         \n\
         ## Use\n\
         \n\
         ```sh\n\
         assemblr add {identity}\n\
         ```\n",
        description = description.as_str()
    )
}

/// A new unit's `AGENTS.md`.
pub(crate) fn unit_agents(identity: &UnitIdentity) -> String {
    format!(
        "# AGENTS.md\n\
         \n\
         This repository is the Assemblr unit `{identity}`. Its files under `src/`, and its \
         changes to its dependencies' files under `amendments/`, are composed into the projects \
         that select it. Before changing it, run `assemblr guide` and follow it.\n\
         \n\
         ## This unit\n\
         \n\
         <!-- What it is for, the conventions its files follow, and what to check before \
         publishing. -->\n"
    )
}

/// Whether `bytes` are an `AGENTS.md` that some version of `assemblr init`
/// wrote, which a composition may replace. Only one text exists so far;
/// when `PROJECT_AGENTS` changes, the earlier text must still be accepted
/// here, or projects initialized before the change lose that guarantee.
pub(crate) fn is_project_agents(bytes: &[u8]) -> bool {
    bytes == PROJECT_AGENTS.as_bytes()
}

#[cfg(test)]
mod tests {
    use super::{PROJECT_AGENTS, unit_agents};
    use crate::domain::UnitIdentity;

    /// The commands each `AGENTS.md` names, which must stay true across
    /// versions of Assemblr.
    fn commands(text: &str) -> Vec<&str> {
        text.split('`')
            .skip(1)
            .step_by(2)
            .filter(|quoted| quoted.starts_with("assemblr "))
            .collect()
    }

    #[test]
    fn every_agents_file_names_only_the_guide() {
        let unit = UnitIdentity::parse("@acme/web").unwrap();
        for text in [PROJECT_AGENTS.to_owned(), unit_agents(&unit)] {
            assert_eq!(commands(&text), ["assemblr guide"], "{text}");
        }
    }
}
