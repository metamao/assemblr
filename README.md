# Assemblr

**Build projects from reusable, versioned units, and keep them up to date.**

Assemblr is a command-line tool that puts a project together from **units**.
A unit is a reusable, opinionated part of a project, built the way its
author thinks it should be done. It can be a whole starting point, such as
a React web app, a Flutter mobile app, or a Rust CLI. It can also be one
piece: a Dockerfile, a CI pipeline, a Terraform cloud setup, or your team's
lint rules. Each unit lives in a Git repository and has versions: in a
repository of its own, or published together with other units in a
catalog.

**Assemblr works with any technology.** A unit is just files, so any
language, framework, cloud, or tool works. Assemblr never needs to know
what is inside.

You pick the units a project needs, and Assemblr writes their files into it.
When a unit publishes a new version, `assemblr update` brings the change in
and keeps the edits you made yourself. A project template is copied once and
then maintained alone. A unit keeps improving every project that uses it.

> **Status: private alpha.** The full workflow works today: create, share,
> compose, and update. File formats may still change between alpha releases.

## Why Assemblr

- **Fix once, update everywhere.** Bump a base image, patch a CI pipeline,
  or tighten a lint rule in one unit. Every project picks it up with
  `assemblr update`.
- **Your edits stay yours.** Assemblr merges a unit's new version with your
  own changes. When it cannot merge safely, it leaves conflict markers for
  you to resolve, as in a Git merge. It never overwrites your work silently.
- **Less AI slop.** A coding agent that adds a reviewed unit does not
  regenerate setup code from scratch, with new small mistakes each time. It
  spends its effort on the code that is unique to your project.
- **Golden paths without forks.** A unit can build on other units and amend
  their files, so a team can extend a shared base without copying it.
- **Start a new project in minutes** with the stack you already trust, and
  mix units from different authors.
- **Git is the only backend.** Units are Git repositories and versions are
  Git tags. There is no server to run.
- **Safe and predictable.** Assemblr never runs code from a unit. It plans
  every change before it writes a file, published versions never change, and
  the same inputs always give the same result.
- **Built for coding agents.** `assemblr guide` teaches an agent the
  workflow, and the commands that report state offer `--json`.

## See it in action

Your team keeps the Dockerfile for its Rust services in one unit,
`@acme/rust-service`. The `orders-api` project uses it, and someone added an
`EXPOSE 8080` line to the project's Dockerfile by hand.

The unit then publishes version 1.1.0, which moves to Rust 1.90 and a smaller
distroless runtime image. In `orders-api`, one command brings that in:

```console
$ assemblr update
Updated: @acme/rust-service 1.0.0 -> 1.1.0.
note: updated assemblr-lock.json: @acme/rust-service 1.0.0 -> 1.1.0
Composed 2 units into 4 files: 1 merged.
  merged Dockerfile
```

Here is what changed in the Dockerfile. Both image updates arrived, and the
`EXPOSE 8080` line is still there:

```diff
-FROM rust:1.89-slim-bookworm AS build
+FROM rust:1.90-slim-bookworm AS build
 WORKDIR /src
 COPY . .
 RUN cargo build --release --locked

-FROM debian:bookworm-slim
+FROM gcr.io/distroless/cc-debian12
 COPY --from=build /src/target/release/orders-api /usr/local/bin/app
 EXPOSE 8080
 USER 65534
 CMD ["app"]
```

Every other service that uses the unit gets the same update the same way.

## How it works

| Concept | What it is |
| --- | --- |
| **Unit** | A Git repository with a `manifest.jsonc` and the files to deliver under `src/`. Its name looks like `@owner/name`. |
| **Version** | A published state of a unit, stored as a Git tag such as `v1.1.0`. It never changes once published. |
| **Project** | Any directory with an `assemblr.jsonc` that lists the units it uses. |
| **Registry** | The list of units this machine knows, kept on this machine. |
| **Amendment** | A unit's change to a file that one of the units it builds on delivers. |
| **Placeholder** | A token such as `__APP_NAME__` that a unit's files hold for a value each project sets. The project replaces it in its own files. |

**There is no hosted registry or package manager.** Assemblr only knows the
units in your local registry, and you decide what goes in it. Assemblr gives
you the tools to fill it and keep it current:

- `assemblr registry add <URL or path>` registers a unit's repository.
- `assemblr registry sync` picks up the versions published since then.
- `assemblr search <words>` and `assemblr info <unit>` show what you can use.

Assemblr records what it delivered to a project in `assemblr-lock.json` and
`.assemblr/`. That record is how it tells your edits apart from a unit's
changes, so commit both with the project.

## Get started

| Guide | What it covers |
| --- | --- |
| [Install Assemblr](docs/user/install.md) | Prebuilt binaries for Linux, WSL2, and Windows, or a build from source |
| [Use units in a project](docs/user/use.md) | Register units, create a project, add units, and stay up to date |
| [Create a unit](docs/user/create-a-unit.md) | Write, build, check, and publish a unit |
| [Keep several units in one repository](docs/user/catalogs.md) | Publish units together as a catalog |
| [Share your units](docs/user/share.md) | Push a unit and hand out its URL |
| [Work with a coding agent](docs/user/agents.md) | The agent guide, the Claude Code skill, and the line for `AGENTS.md` |
| [Command overview](docs/user/commands.md) | Every command in one table |

## Documentation

| Where | What |
| --- | --- |
| [`docs/user/`](docs/user/) | How to install and use Assemblr, one guide per task |
| [`docs/domain/`](docs/domain/README.md) | What Assemblr means and promises: the product contracts |
| [`docs/domain/open-contracts.md`](docs/domain/open-contracts.md) | Product questions that are not decided yet |
| [`docs/adr/`](docs/adr/README.md) | Architectural decision records |
| [`AGENTS.md`](AGENTS.md) | How Assemblr is engineered: rules for contributors and coding agents |
| [`docs/user-facing-copy.md`](docs/user-facing-copy.md) | How Assemblr's help, messages, README, and user guides are written |

## Development

Assemblr is written in Rust. The toolchain is pinned in
`rust-toolchain.toml`, and rustup installs it automatically.

```sh
cargo run -- --help                                        # run the CLI
cargo sandbox -- assemblr --help                           # run it in an isolated registry
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --all-targets
```
