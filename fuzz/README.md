# Fuzzing

Fuzz targets for the parsers of network data. They need nightly Rust and
[`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz). Run from the
repository root:

```sh
cargo install cargo-fuzz --locked
cargo +nightly fuzz list
# Arguments: the corpus directory, then the seeds.
cargo +nightly fuzz run soap fuzz/corpus/soap fuzz/seeds/soap
```

Crashes are saved in `fuzz/artifacts/<target>/`. To run one again:
`cargo +nightly fuzz run <target> <file>`.

Each target also checks rules that must be true for any input. The checks
are in the `fuzz` modules next to the parsers in `src/`. `proc_route` runs
only on Linux.
