# Crumple bench harness

Runs `crumple optimize` over the Kodak corpus (`bench/corpus/kodak`) at one or more
quality targets, then summarises the JSONL reports next to the Squoosh-defaults
baseline in `bench/baseline/results.json`.

Needs Node 24 and no npm packages. Fetch the corpus first with `node bench/corpus/fetch.mjs`.

```sh
cargo build --release -p crumple-cli
node bench/crumple/run.mjs                        # targets 70,80,90, one run each
node bench/crumple/run.mjs --targets 80 --runs 3  # wall time is the median of the runs
node bench/crumple/run.mjs --determinism          # also compares --jobs 1 vs --jobs <all CPUs> at target 80
```

Options:

- `--crumple <path or command>`: the binary to run. Defaults to `target/release/crumple(.exe)`.
  A command string is split on whitespace, and double quotes group words.
- `--targets 70,80,90`, `--runs 1`, `--determinism` (`--jobs 1` against `--jobs J`; it fails
  unless both runs exit 0 and every output file hashes the same).
- `--no-pin`: skip the Windows P-core pinning.

For each target `T` it deletes `bench/out/crumple/tT` and `tT.jsonl`, runs
`<crumple> optimize bench/corpus/kodak --out bench/out/crumple/tT --target T --jobs J --report bench/out/crumple/tT.jsonl --overwrite`
and writes `bench/crumple/results.json` and `bench/crumple/RESULTS.md`. `J` is the number of
CPUs the harness may use after pinning (Node's `os.availableParallelism()`); it is passed
explicitly because Rust's `available_parallelism()` ignores the Windows affinity mask.

Hit rate is hits divided by every image the run should have handled. A hit is an
`optimized` output whose score is at least the target, an `optimized` lossless PNG, or a
kept original. Errors and `too-large` skips count as misses; only non-image files (skipped
as `unsupported`) are left out. Mean evals per codec averages over the images where that
codec was actually searched (evals > 0).

On Windows machines with more than 16 logical CPUs, the harness sets its own affinity
mask to `0xFFFF` (P-cores 0-15 on the i7-14700KF, as described in `bench/README.md`);
the crumple child processes inherit it, and `results.json` records the mask under
`env.scheduling`. On Linux, pin with `taskset -c 0-15 node bench/crumple/run.mjs`.

## Independent re-score

`rescore.mjs` checks the outputs of the last run without any Crumple code: it decodes each file
in `bench/out/crumple/t<T>/` with jSquash (Squoosh's decoders) and scores it with
`ssimulacra2_rs`, the path `bench/baseline/run.mjs` used, so its scores compare directly with the
baseline. It needs `npm ci` in `bench/` and `ssimulacra2_rs` on `PATH`.

```sh
node bench/crumple/rescore.mjs --targets 70,80,90
```

## Testing the harness without the real binary

`fake-crumple.mjs` accepts the `optimize` flags, copies every input unchanged and
writes a report in the frozen T4 schema:

```sh
node bench/crumple/run.mjs --crumple "node bench/crumple/fake-crumple.mjs" --targets 80
```

Expect 25 images, all `kept-original`, 100% hit rate.
