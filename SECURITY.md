# Security policy

## Reporting a vulnerability

Report it privately through
[GitHub's vulnerability reporting](https://github.com/metamao/assemblr/security/advisories/new).
Do not open a public issue.

Include the output of `assemblr --version`, your operating system, the steps
that reproduce the problem, and what an attacker could do with it.

Assemblr treats units, registries, repositories, and paths as untrusted
([AGENTS.md §5](AGENTS.md#5-filesystem-and-external-input-security)). A way to
make it write outside the directory it owns, follow a symlink out of it, or
run code that a unit supplies is a vulnerability.

## Supported versions

Assemblr is in alpha. Only the latest release receives fixes.

## What to expect

Assemblr has one maintainer, so there is no guaranteed response time. A fix
ships in a new release, and the advisory is published after it.
