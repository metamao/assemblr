//! The tasks given to the agent: how each starts, what it asks, and the
//! checks of the state it leaves.
//!
//! Prompts are in Spanish, as in the measurements this bench reproduces
//! (ADR 0034). Checks look only at the end state, never at how the agent got
//! there; the transcript metrics cover that.

use std::fs;
use std::path::{Path, PathBuf};

use crate::fixture;
use crate::machine::{Machine, Root, arg};

pub(crate) struct Scenario {
    pub(crate) name: &'static str,
    pub(crate) summary: &'static str,
    /// What the agent is asked. `{root}` is replaced by the run's directory.
    pub(crate) prompt: &'static str,
    /// Where the agent starts, relative to the run's directory.
    pub(crate) start: &'static str,
    /// Directories outside the start that the agent may also use.
    pub(crate) extra_dirs: &'static [&'static str],
    pub(crate) setup: fn(&Root) -> Result<(), String>,
    pub(crate) verify: fn(&Root, &mut Report) -> Result<(), String>,
}

/// The outcome of a scenario's checks, in the order they ran.
#[derive(Default)]
pub(crate) struct Report {
    pub(crate) checks: Vec<(&'static str, bool)>,
    pub(crate) notes: Vec<String>,
    /// What went wrong with the run itself rather than with the agent's
    /// work, such as a timeout or Claude Code failing to start.
    pub(crate) problems: Vec<String>,
}

impl Report {
    fn check(&mut self, name: &'static str, passed: bool) {
        self.checks.push((name, passed));
    }

    pub(crate) fn problem(&mut self, problem: String) {
        self.problems.push(problem);
    }

    pub(crate) fn note(&mut self, note: String) {
        self.notes.push(note);
    }
}

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "s1b",
        summary: "new Cloudflare Workers API in an empty directory",
        prompt: "Quiero una API en Cloudflare Workers con TypeScript, con GET /health que devuelva \
                 {\"status\":\"ok\"} y POST /echo que devuelva el JSON recibido.",
        start: "home/work/api",
        extra_dirs: &[],
        setup: |root| {
            let agent = root.agent()?;
            fixture::units(&agent)?;
            create_dir(&agent.work().join("api"))
        },
        verify: |root, report| {
            let agent = root.agent()?;
            let project = agent.work().join("api");
            report.check(
                "used_assemblr",
                contains(&project.join("assemblr.jsonc"), "@acme/cloudflare-worker"),
            );
            report.check(
                "health_route",
                tree_contains(&project.join("src"), "/health"),
            );
            report.check("echo_route", tree_contains(&project.join("src"), "/echo"));
            report.check("no_placeholders_left", no_placeholders(&agent, &project));
            report.check("compose_noop", nothing_to_compose(&agent, &project));
            report.check(
                "code_survives_compose",
                tree_contains(&project.join("src"), "/health"),
            );
            Ok(())
        },
    },
    Scenario {
        name: "s2",
        summary: "make an existing ESLint and Prettier setup reusable",
        prompt: "Quiero reutilizar la configuración de ESLint y Prettier de este repo en mis otros \
                 proyectos. Prepárala para eso.",
        start: "home/work/lint-config",
        extra_dirs: &[],
        setup: |root| {
            let agent = root.agent()?;
            fixture::units(&agent)?;
            fixture::repository(&agent, "lint-config", &agent.work().join("lint-config"))
        },
        verify: |root, report| {
            let agent = root.agent()?;
            let unit = find(&agent.work(), &|path| {
                path.ends_with("manifest.jsonc") && contains(path, "\"name\"")
            })
            .and_then(|manifest| manifest.parent().map(Path::to_owned));
            report.note(format!("unit: {}", display(unit.as_deref())));
            report.check("made_a_unit", unit.is_some());
            let Some(unit) = unit else { return Ok(()) };
            report.check(
                "unit_check_passes",
                succeeds(&agent, &unit, &["unit", "check"]),
            );
            let src = unit.join("src");
            report.check("eslint_in_src", has_file(&src, "eslint.config.js"));
            report.check("prettier_in_src", has_file(&src, ".prettierrc"));
            report.note(format!(
                "tags: {}",
                output(&agent, &unit, "git", &["tag"]).trim()
            ));
            Ok(())
        },
    },
    Scenario {
        name: "s5",
        summary: "add JWT authentication to an existing project",
        prompt: "Añade autenticación JWT a la API. Los tokens los emite https://auth.orders.example/.",
        start: "home/work/orders-api",
        extra_dirs: &[],
        setup: |root| {
            let agent = root.agent()?;
            fixture::units(&agent)?;
            orders_api(&agent).map(|_| ())
        },
        verify: |root, report| {
            let agent = root.agent()?;
            let project = agent.work().join("orders-api");
            report.check(
                "used_assemblr",
                contains(&project.join("assemblr.jsonc"), "@acme/auth-jwt"),
            );
            let issuer = "https://auth.orders.example/";
            report.check("issuer_set", tree_contains(&project.join("src"), issuer));
            report.check(
                "no_placeholders_left",
                !tree_contains(&project, "__JWT_ISSUER__"),
            );
            report.check(
                "worker_name_kept",
                contains(&project.join("wrangler.toml"), "name = \"orders-api\""),
            );
            report.check("auth_file", project.join("src/auth.ts").is_file());
            report.check("compose_noop", nothing_to_compose(&agent, &project));
            Ok(())
        },
    },
    Scenario {
        name: "s6",
        summary: "update a project whose edited file the new unit version also changed",
        prompt: "Actualiza las dependencias del proyecto.",
        start: "home/work/orders-api",
        extra_dirs: &[],
        setup: |root| {
            let agent = root.agent()?;
            fixture::units(&agent)?;
            let project = orders_api(&agent)?;
            write(
                &project.join("src/index.ts"),
                "export default {\n  async fetch(request: Request): Promise<Response> {\n    \
                 const url = new URL(request.url);\n    \
                 if (url.pathname === \"/orders\") return Response.json([]);\n    \
                 return Response.json({ ok: true });\n  },\n};\n",
            )?;
            fixture::commit(&agent, &project, "Add /orders route")?;
            fixture::release(&agent, "cloudflare-worker", "1.1.0")
        },
        verify: |root, report| {
            let agent = root.agent()?;
            let project = agent.work().join("orders-api");
            report.check(
                "locked_1_1_0",
                locked_version(&project, "@acme/cloudflare-worker").as_deref() == Some("1.1.0"),
            );
            report.check("no_markers", !has_markers(&project));
            let index = project.join("src/index.ts");
            report.check("user_route_kept", contains(&index, "/orders"));
            report.check("incoming_header", contains(&index, "cache-control"));
            report.check(
                "new_compat_date",
                contains(&project.join("wrangler.toml"), "2026-10-15"),
            );
            report.check("compose_noop", nothing_to_compose(&agent, &project));
            Ok(())
        },
    },
    Scenario {
        name: "s7",
        summary: "finish an update that an earlier session left waiting on a conflict",
        prompt: "Estaba actualizando el proyecto y se quedó a medias; termínalo.",
        start: "home/work/orders-api",
        extra_dirs: &[],
        setup: |root| {
            let agent = root.agent()?;
            fixture::units(&agent)?;
            let project = orders_api(&agent)?;
            let wrangler = project.join("wrangler.toml");
            write(
                &wrangler,
                &format!(
                    "{}compatibility_flags = [\"streams_enable_constructors\"]\n",
                    read(&wrangler)
                ),
            )?;
            write(
                &project.join("src/routes.ts"),
                "export const routes = [\"/orders\"];\n",
            )?;
            fixture::commit(&agent, &project, "Enable streams constructors; add routes")?;
            fixture::release(&agent, "cloudflare-worker", "1.2.0")?;
            // The earlier session: the update stops, waiting on the conflict
            // in wrangler.toml.
            if agent.assemblr(&project, &["update"]).is_ok() {
                return Err("the update did not stop on a conflict".to_owned());
            }
            Ok(())
        },
        verify: |root, report| {
            let agent = root.agent()?;
            let project = agent.work().join("orders-api");
            report.check(
                "locked_1_2_0",
                locked_version(&project, "@acme/cloudflare-worker").as_deref() == Some("1.2.0"),
            );
            report.check("no_markers", !has_markers(&project));
            report.check("compose_noop", nothing_to_compose(&agent, &project));
            let wrangler = read(&project.join("wrangler.toml"));
            report.check(
                "user_flag_kept",
                wrangler.contains("streams_enable_constructors"),
            );
            report.check("incoming_flag_kept", wrangler.contains("nodejs_compat"));
            report.check(
                "one_flags_key",
                wrangler
                    .lines()
                    .filter(|line| line.trim_start().starts_with("compatibility_flags"))
                    .count()
                    == 1,
            );
            report.check("user_file_kept", project.join("src/routes.ts").is_file());
            Ok(())
        },
    },
    Scenario {
        name: "s11",
        summary: "start a new project from a template project",
        prompt: "Crea un proyecto nuevo llamado tienda a partir de mi template web-starter, que \
                 está en {root}/templates/web-starter.",
        start: "home/work",
        extra_dirs: &["templates"],
        setup: |root| {
            let agent = root.agent()?;
            fixture::units(&agent)?;
            let template = web_starter(&agent)?;
            write(
                &template.join("README.md"),
                "# web-starter\n\nPlantilla: Angular SSR, Docker con Bun y Cloudflare. \
                 Compuesta con Assemblr.\n",
            )?;
            fixture::commit(&agent, &template, "Describe the template")
        },
        verify: |root, report| {
            let agent = root.agent()?;
            let project = agent.work().join("tienda");
            let declaration = project.join("assemblr.jsonc");
            report.check("project_exists", declaration.is_file());
            if declaration.is_file() {
                let text = read(&declaration);
                report.check(
                    "same_units",
                    ["angular-ssr", "docker-bun", "cloudflare-pages"]
                        .iter()
                        .all(|unit| text.contains(unit)),
                );
                report.check(
                    "package_name",
                    contains(&project.join("package.json"), "\"name\": \"tienda\""),
                );
                report.check(
                    "wrangler_name",
                    contains(&project.join("wrangler.toml"), "name = \"tienda\""),
                );
                report.check("compose_noop", nothing_to_compose(&agent, &project));
            }
            let template = root.path.join("templates/web-starter");
            report.check("template_untouched", is_clean(&agent, &template));
            Ok(())
        },
    },
    Scenario {
        name: "s12",
        summary: "add a page to a project made from a template, then update its units",
        prompt: "Añade una página de catálogo de productos en la ruta /productos, con algunos \
                 productos de ejemplo. Después actualiza el proyecto a las últimas versiones de \
                 sus units.",
        start: "home/work/tienda",
        extra_dirs: &[],
        setup: |root| {
            let agent = root.agent()?;
            fixture::units(&agent)?;
            let template = web_starter(&agent)?;
            let work = agent.work();
            agent.git(&work, &["clone", "-q", arg(&template)?, "tienda"])?;
            let project = work.join("tienda");
            fixture::replace(&project, "web-starter", "tienda")?;
            fixture::commit(&agent, &project, "Start tienda from web-starter")?;
            fixture::release(&agent, "angular-ssr", "1.1.0")
        },
        verify: |root, report| {
            let agent = root.agent()?;
            let project = agent.work().join("tienda");
            report.check(
                "locked_1_1_0",
                locked_version(&project, "@acme/angular-ssr").as_deref() == Some("1.1.0"),
            );
            report.check("no_markers", !has_markers(&project));
            report.check(
                "productos_code",
                tree_contains(&project.join("src"), "productos"),
            );
            report.check(
                "router_outlet_kept",
                tree_contains(&project.join("src"), "router-outlet"),
            );
            report.check("compose_noop", nothing_to_compose(&agent, &project));
            Ok(())
        },
    },
    Scenario {
        name: "s13",
        summary: "create a unit that extends another",
        prompt: "Crea una unit @acme/worker-health que extienda @acme/cloudflare-worker para que \
                 el Worker responda GET /health con {\"status\":\"ok\"}. La quiero usar en todos \
                 mis Workers.",
        start: "home/work",
        extra_dirs: &[],
        setup: |root| fixture::units(&root.agent()?),
        verify: |root, report| {
            let agent = root.agent()?;
            let unit = find(&agent.work(), &|path| {
                path.ends_with("manifest.jsonc") && contains(path, "@acme/worker-health")
            })
            .and_then(|manifest| manifest.parent().map(Path::to_owned));
            report.note(format!("unit: {}", display(unit.as_deref())));
            report.check("unit_exists", unit.is_some());
            let Some(unit) = unit else { return Ok(()) };
            report.check(
                "depends_on_worker",
                contains(&unit.join("manifest.jsonc"), "@acme/cloudflare-worker"),
            );
            report.check(
                "amends_index",
                find(&unit.join("amendments"), &|path| {
                    file_name(path).starts_with("index.ts")
                })
                .is_some(),
            );
            report.check(
                "unit_check_passes",
                succeeds(&agent, &unit, &["unit", "check"]),
            );
            report.check(
                "build_has_health",
                tree_contains(&unit.join("build/src"), "/health"),
            );
            report.note(format!(
                "tags: {}",
                output(&agent, &unit, "git", &["tag"]).trim()
            ));
            Ok(())
        },
    },
];

/// The project `orders-api`, started from `@acme/cloudflare-worker`.
fn orders_api(machine: &Machine<'_>) -> Result<PathBuf, String> {
    let project = machine.work().join("orders-api");
    fixture::project(
        machine,
        &project,
        &["@acme/cloudflare-worker"],
        &[("__PROJECT_NAME__", "orders-api")],
        "Start orders-api from @acme/cloudflare-worker",
    )?;
    Ok(project)
}

/// The template project `templates/web-starter`.
fn web_starter(machine: &Machine<'_>) -> Result<PathBuf, String> {
    let template = machine.root.path.join("templates/web-starter");
    fixture::project(
        machine,
        &template,
        &[
            "@acme/angular-ssr",
            "@acme/docker-bun",
            "@acme/cloudflare-pages",
        ],
        &[("__PROJECT_NAME__", "web-starter")],
        "web-starter template",
    )?;
    Ok(template)
}

fn create_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// The file's text, or nothing when it cannot be read.
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn contains(path: &Path, text: &str) -> bool {
    read(path).contains(text)
}

fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

fn display(path: Option<&Path>) -> String {
    path.map_or_else(|| "none".to_owned(), |path| path.display().to_string())
}

/// The files under `dir`, in a stable order, outside Git's and Assemblr's
/// own state and installed packages.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if ![".git", ".assemblr", "node_modules"].contains(&file_name(&path)) {
                found.extend(files(&path));
            }
        } else {
            found.push(path);
        }
    }
    found
}

fn find(dir: &Path, matches: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    files(dir).into_iter().find(|path| matches(path))
}

fn tree_contains(dir: &Path, text: &str) -> bool {
    find(dir, &|path| contains(path, text)).is_some()
}

/// Whether `dir` holds a file whose name starts with `name`.
fn has_file(dir: &Path, name: &str) -> bool {
    find(dir, &|path| file_name(path).starts_with(name)).is_some()
}

fn has_markers(dir: &Path) -> bool {
    find(dir, &|path| {
        read(path).lines().any(|line| line.starts_with("<<<<<<<"))
    })
    .is_some()
}

fn succeeds(machine: &Machine<'_>, dir: &Path, args: &[&str]) -> bool {
    succeeds_program(machine, dir, "assemblr", args)
}

fn succeeds_program(machine: &Machine<'_>, dir: &Path, program: &str, args: &[&str]) -> bool {
    machine.run(dir, program, args).is_ok()
}

/// The command's output, or nothing when it fails.
fn output(machine: &Machine<'_>, dir: &Path, program: &str, args: &[&str]) -> String {
    machine.run(dir, program, args).unwrap_or_default()
}

/// Whether `assemblr status` reports no placeholder left to replace.
fn no_placeholders(machine: &Machine<'_>, project: &Path) -> bool {
    let status = output(machine, project, "assemblr", &["status", "--json"]);
    serde_json::from_str::<serde_json::Value>(&status)
        .is_ok_and(|status| status["placeholders"] == serde_json::json!([]))
}

fn nothing_to_compose(machine: &Machine<'_>, project: &Path) -> bool {
    output(machine, project, "assemblr", &["compose"]).contains("Nothing to compose")
}

fn is_clean(machine: &Machine<'_>, repository: &Path) -> bool {
    machine
        .git(repository, &["status", "--porcelain"])
        .is_ok_and(|status| status.trim().is_empty())
}

fn locked_version(project: &Path, unit: &str) -> Option<String> {
    let lock: serde_json::Value =
        serde_json::from_str(&read(&project.join("assemblr-lock.json"))).ok()?;
    lock["units"][unit]["version"].as_str().map(str::to_owned)
}
