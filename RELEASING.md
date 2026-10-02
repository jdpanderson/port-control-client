# Releasing

1. Set `version` in `Cargo.toml`, and run `cargo check`. Before 1.0, a
   breaking change increases the minor version.
2. In `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`,
   and add a new empty `## [Unreleased]` above it.
3. Commit, push, and wait for CI to pass.
4. Tag and push: `git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z`.

The [release workflow](.github/workflows/release.yml) then publishes to
crates.io and makes a GitHub release from the changelog section. A tag
with a suffix, such as `v0.2.0-rc.1`, makes a prerelease. If the workflow
fails, run it again.
