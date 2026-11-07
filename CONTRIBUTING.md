# Contributing

## Issues

Bug reports and ideas are welcome. Search the existing issues first.

For a bug, include the output of `assemblr --version`, your operating system,
the commands you ran, what you expected, and what happened. Report security
problems as [SECURITY.md](SECURITY.md) describes, never in an issue.

## Pull requests

Open an issue first, and wait until the change is agreed there. Pull requests
without that agreement are closed. This keeps every change in line with the
product contracts and saves you work that might not be merged.

Once a change is agreed:

1. Read [`AGENTS.md`](AGENTS.md) for the engineering rules and
   [`docs/domain/`](docs/domain/README.md) for the product contracts. Behavior
   follows the approved contracts. A change to approved behavior updates
   `docs/domain/` in the same pull request and needs the maintainer's approval.
2. Add a regression test to every fix.
3. Run the checks before you push:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-targets --all-features
   cargo build --all-targets
   ```

Workflows on pull requests from forks run after the maintainer approves them.

## License

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in Assemblr by you, as defined in the Apache-2.0
license, shall be dual licensed as in the [README](README.md#license),
without any additional terms or conditions.
