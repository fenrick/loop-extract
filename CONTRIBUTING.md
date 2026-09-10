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

All three must be clean. CI runs the same three, so a failure here is a failure
there.

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

[`FORMAT-NOTES.md`](FORMAT-NOTES.md) is the same material written as a whole,
for a reader who wants the format rather than the code. Keep the two in step: a
finding that changes the notes usually changes a module comment too.

Read the relevant one before changing how something is parsed.

## Commit messages

Releases are generated from commit messages, so they have to follow
[Conventional Commits](https://www.conventionalcommits.org/):

```text
feat: recover embedded table content
fix: count operation positions in UTF-16 units
docs: correct the tag table's u32 widths
```

| Prefix | Effect on the next release |
| --- | --- |
| `feat:` | minor version bump, listed under **Added** |
| `fix:` | patch bump, listed under **Fixed** |
| `perf:`, `refactor:`, `docs:` | patch bump, listed under Performance / Changed / Documentation |
| `test:`, `build:`, `ci:`, `chore:` | patch bump, not listed |
| `feat!:` or a `BREAKING CHANGE:` footer | major bump |

Anything not matching a known prefix is ignored for versioning, which quietly
leaves a change out of the changelog. Prefix every commit.

## How a release happens

Releases are automated with
[release-please](https://github.com/googleapis/release-please). Nobody tags by
hand.

1. Merge work into `main` with conventional commit messages.
2. release-please opens or updates a **release pull request** that bumps the
   version in `Cargo.toml` and `Cargo.lock`, writes the `CHANGELOG.md` entry and
   updates `.release-please-manifest.json`.
3. Review that pull request. It is the last chance to correct the version or the
   changelog before either becomes permanent.
4. Merge it. release-please tags `vX.Y.Z` and publishes a GitHub Release.
5. The release workflow then builds four targets, and attaches an archive plus a
   SHA-256 checksum for each, along with `.deb` and `.rpm` packages.

When a pull request is merged with a merge commit, the merge commit carries the
pull request *title*. Prefix **either** the title **or** the commits inside it,
never both, or the change is counted twice and appears twice in the changelog.
Splitting work into typed commits means giving the pull request a plain title.

To release a specific version regardless of what the commits imply, put a footer
on a commit:

```text
Release-As: 2.0.0
```

That is how the first release was set to `1.0.0` rather than the `0.2.0` the
commit history implied.

CI runs formatting, lints and tests, checks that all four release targets still
compile, and fails if a `.loop` file or an email address is ever committed. All
of it must pass before a release pull request is merged.

## Building a binary by hand

If you build one to hand to someone else, remap the build paths first, or the
binary embeds your home directory in its panic messages:

```bash
RUSTFLAGS="--remap-path-prefix=$HOME=." cargo build --release
```

On macOS, a binary copied between machines picks up a quarantine flag and is
blocked on first run. The recipient clears it with
`xattr -d com.apple.quarantine loop-extract`, or builds from source instead.

## Packaging

`.deb` and `.rpm` metadata lives in `Cargo.toml` under
`[package.metadata.deb]` and `[package.metadata.generate-rpm]`. To build them
locally:

```bash
cargo install cargo-deb cargo-generate-rpm
cargo build --release
cargo deb --no-build --output dist/
cargo generate-rpm --output dist/
```

Homebrew, Scoop and WinGet manifests are not published yet. Each needs a
per-release checksum, so they want a workflow step rather than a file in this
repository.
