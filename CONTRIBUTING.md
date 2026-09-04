# Contributing

Thanks for looking. This is a small tool with an unusual constraint: the file
format it reads is undocumented, and everything it knows was worked out by
reading real files. That shapes how changes are made.

## Build and test

```bash
cargo build --release
cargo test
```

The test suite passes with no test documents present — corpus tests skip
themselves and say so. To run them, point the variable at a directory of
`.loop` / `.fluid` files:

```bash
LOOP_EXTRACT_TEST_CORPUS=/path/to/files cargo test
```

Before opening a pull request:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

All three must be clean. There is no CI, so these are the gate.

## Never commit a real document

A `.loop` file carries its authors' names, email addresses, directory object ids
and edit timestamps on nearly every paragraph. Do not commit one, do not attach
one to an issue, and do not paste document text into a test.

Unit tests use synthetic data only. Test expectations that quote a real document
belong in `tests/expectations/private/`, which is ignored; the committed
directory holds the schema and one synthetic example. See
`tests/expectations/README.md`.

If you add a fixture, no test code needs changing — the loader picks up an
expectation file if one exists for a corpus file and skips it otherwise.

## Say what you know, and what you only think

This is the important one. Every claim about the format is marked in the source
as one of two things, and reviewers will ask you to do the same:

```rust
// Observed in test fixtures:
// A marker terminates the text run before it and carries that block's
// properties.

// ASSUMPTION, not confirmed:
// The leading integer in a `list!reference` value is the nesting depth.
// Nothing may depend on this until a test demonstrates it.
```

An observation is something you saw in real files and can point at. An
assumption is a reading that has not been demonstrated. Mixing the two is how a
parser ends up quietly wrong two Loop versions later.

Where an assumption cannot be avoided, keep it inert: record it, expose it as a
diagnostic, and do not let output depend on it.

## Design constraints worth knowing before you change things

- **Parsing never depends on rendering.** The layering is bytes → gzip members →
  embedded payloads → merge trees → document model → output. A new output format
  needs no parser change; a format discovery needs no renderer change.
- **Nothing is discarded silently.** An unrecognised property is preserved on the
  block and counted in `inspect`. A recoverable problem becomes a warning with a
  stable code, not a log line.
- **The tool never fails wholesale for one bad payload.** Hard errors are for
  unreadable input, empty input, a file that is not a container at all, and
  output write failures. Everything else is a warning.
- **Guessing is worse than stopping.** The framing reader halts on an unknown tag
  rather than assuming a width. Prefer a reported failure to a plausible wrong
  answer.
- **No new abstraction without an observed problem.** Layers exist here because
  something concrete needed them.

## Where the format knowledge lives

In the module documentation, next to the code that relies on it:

| Module | Covers |
| --- | --- |
| `src/gzip.rs` | member discovery and bounded inflation |
| `src/discovery/fragment_scanner.rs` | finding JSON inside binary framing |
| `src/discovery/envelope.rs` | the framing itself, including the tag table |
| `src/fluid/merge_tree.rs` | chunks, tree assembly, deduplication, roles |
| `src/fluid/properties.rs` | the property vocabulary and what each key means |
| `src/document/reconstruct.rs` | how segments become blocks |
| `src/operations/log.rs` | the operation log and how it is addressed |

Read the relevant one before changing how something is parsed.

## Releasing a binary

There is no CI and no published binary; the repository stores source. If you
build one to hand to someone else, remap the build paths first, or the binary
embeds your home directory in its panic messages:

```bash
RUSTFLAGS="--remap-path-prefix=$HOME=." cargo build --release
```

On macOS, a binary copied between machines picks up a quarantine flag and is
blocked on first run. The recipient clears it with
`xattr -d com.apple.quarantine loop-extract`, or builds from source instead.
