# Work with a coding agent

Assemblr is designed so that coding agents can use it as well as people can.

- `assemblr guide` prints a short guide for agents that matches the installed
  version.
- For Claude Code, run `assemblr skill install claude` once per machine. It
  installs a skill that tells Claude Code when to use Assemblr and to read
  the guide.
- For other agents, or a cloud session without the skill, add this line to
  the project's `AGENTS.md`: "Run `assemblr guide` before changing this
  project's setup, and follow it."
