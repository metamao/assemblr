//! The `README.md` and `AGENTS.md` that `init`, `unit init`, and `catalog
//! init` write when they are missing (owner decision of 2026-10-23, #115).
//!
//! `AGENTS.md` points a coding agent to `assemblr guide`, the measured
//! pointer, so that it holds no instructions that a later Assemblr could
//! make wrong. A unit's and a catalog's files add sections for the author to
//! fill in with what only they know.

use crate::domain::{CatalogIdentity, Description, UnitIdentity};

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
         ## Options\n\
         \n\
         <!-- Each parameter: what it changes, and its default. -->\n\
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

/// A new catalog's `README.md`.
pub(crate) fn catalog_readme(identity: &CatalogIdentity) -> String {
    format!(
        "# {identity}\n\
         \n\
         An Assemblr catalog: `registry.jsonc` lists unit repositories, and \
         `registry-index.json` their published versions.\n\
         \n\
         ## Units\n\
         \n\
         <!-- What the catalog collects, and who maintains it. -->\n\
         \n\
         ## Use\n\
         \n\
         ```sh\n\
         assemblr registry add <this repository's URL>\n\
         ```\n"
    )
}

/// A new catalog's `AGENTS.md`.
pub(crate) fn catalog_agents(identity: &CatalogIdentity) -> String {
    format!(
        "# AGENTS.md\n\
         \n\
         This repository is the Assemblr catalog `{identity}`. Change it only with `assemblr \
         catalog` commands, never by editing `registry-index.json`. Before changing it, run \
         `assemblr guide` and follow it.\n\
         \n\
         ## This catalog\n\
         \n\
         <!-- Which units belong in it, and who reviews additions. -->\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{PROJECT_AGENTS, catalog_agents, unit_agents};
    use crate::domain::{CatalogIdentity, UnitIdentity};

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
    fn every_agents_file_points_to_the_guide() {
        let unit = UnitIdentity::parse("@acme/web").unwrap();
        let catalog = CatalogIdentity::parse("@acme/catalog").unwrap();
        for text in [
            PROJECT_AGENTS.to_owned(),
            unit_agents(&unit),
            catalog_agents(&catalog),
        ] {
            assert!(commands(&text).contains(&"assemblr guide"), "{text}");
        }
    }
}
