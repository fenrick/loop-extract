# loop-extract

[![CI](https://github.com/fenrick/loop-extract/actions/workflows/ci.yml/badge.svg)](https://github.com/fenrick/loop-extract/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/fenrick/loop-extract?sort=semver)](https://github.com/fenrick/loop-extract/releases/latest)
[![License: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)

Reads Microsoft Loop `.loop` and Fluid `.fluid` files and reconstructs the
document inside them as Markdown, plain text or JSON.

It runs entirely locally. It needs no Microsoft 365, OneDrive, SharePoint or
Microsoft Graph access, and makes no network calls.

## Why

A Loop page is not a document file. It is a binary Fluid container: `file`
reports `data`, and the bytes do not decode as text. Nothing in the usual
toolchain can read one, so the content of a Loop page is effectively trapped
unless you open it in Loop and copy it out by hand.

The format is undocumented. Everything this tool knows about it was established
by reading a corpus of real files. Every such conclusion is marked in the source
as either an observation or an assumption, and the `inspect` command exists so
that a future format change shows up as a diagnostic rather than as quietly
missing text.

## Install

Every release publishes a signed-checksum archive for each supported platform,
plus Debian and RPM packages. See
[the latest release](https://github.com/fenrick/loop-extract/releases/latest).

| Platform | Asset |
| --- | --- |
| Linux x86-64 | `loop-extract-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`, `.deb`, `.rpm` |
| macOS Apple Silicon | `loop-extract-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| Windows x86-64 | `loop-extract-vX.Y.Z-x86_64-pc-windows-msvc.zip` |
| Windows ARM64 | `loop-extract-vX.Y.Z-aarch64-pc-windows-msvc.zip` |

Each asset has a `.sha256` file beside it. Verify before use:

```bash
sha256sum -c loop-extract-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz.sha256
```

### Linux

```bash
sudo dpkg -i loop-extract_X.Y.Z-1_amd64.deb       # Debian, Ubuntu
sudo rpm -i loop-extract-X.Y.Z-1.x86_64.rpm       # Fedora, RHEL, openSUSE
```

Or extract the tarball and put the binary on your `PATH`.

### macOS

```bash
tar xzf loop-extract-vX.Y.Z-aarch64-apple-darwin.tar.gz
xattr -d com.apple.quarantine loop-extract    # downloaded binaries are quarantined
sudo mv loop-extract /usr/local/bin/
```

The binary is not code-signed or notarised, so macOS blocks it on first run until
the quarantine attribute is cleared. Building from source avoids this.

### Windows

Extract the `.zip` and put `loop-extract.exe` somewhere on your `PATH`.

### From source

```bash
cargo install --git https://github.com/fenrick/loop-extract
```

or

```bash
git clone https://github.com/fenrick/loop-extract
cd loop-extract
cargo build --release
# binary at ./target/release/loop-extract
```

Rust 1.85 or later. No system dependencies, no C toolchain, no build scripts.

## Use

```bash
loop-extract extract page.loop                      # Markdown to stdout
loop-extract extract page.loop --format text
loop-extract extract page.loop --format json
loop-extract extract page.loop -o page.md

loop-extract inspect page.loop                      # what the container holds
loop-extract inspect page.loop --verbose
loop-extract inspect page.loop --dump-payloads ./debug/
```

The document goes to stdout; warnings and notes go to stderr, so redirecting
stdout gives you a clean file.

`--help` on any subcommand explains every option, including why
`--include-attribution` is off by default.

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | the document was reconstructed, or the page is legitimately empty (a note on stderr says which) |
| 1 | the input is unreadable or empty, the file is not a Loop/Fluid container, or the output could not be written |

An unreadable file and an empty page are deliberately different outcomes, so a
script can tell them apart.

## Output formats

**Markdown** (default) — title, headings, `**bold**`, `*italic*`, links, and
nested bullets indented four spaces per level.

**Text** — the same structure with formatting stripped. Paragraph and list
boundaries are kept; bullets and indentation are kept, because they are
structure rather than formatting.

**JSON** — the parsed document model plus the diagnostics needed to check it:

- `metadata` — file, format assessment, component identity, which source the
  content came from (`snapshot`, `operations`, `snapshot-plus-operations`), and
  `title_selection`, which records the rule that chose the title, not just the
  answer;
- `title`, omitted when there is none;
- `blocks` — the document in order, each tagged `paragraph`, `heading`,
  `list-item`, `embedded` or `unknown`, with inline content nested as `text`,
  `bold`, `italic` or `link`. Unrecognised Loop properties are preserved on the
  block rather than dropped;
- `source` — gzip members, payloads, one entry per merge tree with its role and
  content hash, and every property key seen with a count and how well it is
  understood (`rendered`, `recognised`, `unknown`);
- `warnings` — each with a stable `code` you can assert on.

The `properties` map is the early-warning system: a new Loop version turning up
as `unknown` keys is how you would notice a format change.

## Personal data

A Loop container records an author on nearly every paragraph — display name,
email address, directory object id and an edit timestamp. One page of meeting
minutes in the reference corpus holds 139 such records.

The parser keeps all of it; the renderers strip it. Markdown and text never
carry attribution. JSON omits it unless you pass `--include-attribution`.

`inspect --dump-payloads` writes raw payloads and **does** contain it. Treat that
directory as you would the source file.

## What it does not do yet

- **Embedded components are not rendered.** Tables, images, `@`-mentions and
  tables of contents appear in the body as placeholder markers; their content
  lives in separate channels and reaches `--format json` but not Markdown. A
  page that is mostly a table will extract as its surrounding prose only.
- **`Annotate` operations are not applied.** Formatting applied after the last
  snapshot may be missing. Insert and remove are applied.
- **Ordered lists are always rendered as bullets.** No corpus file contains a
  numbered list, so the mapping has not been established and is not guessed.

## Verification

Output has been compared against Microsoft's own Markdown export of a Loop page:
46 content lines each, with no difference in title, paragraph text, list text,
list nesting or heading level. The one deliberate difference is that Loop's
export drops bold formatting the container records, and this tool keeps it.

## Tests

```bash
cargo test                                              # unit tests, synthetic data only
LOOP_EXTRACT_TEST_CORPUS=/path/to/files cargo test      # plus corpus regression tests
```

Test documents are not committed: a Loop file contains document text and its
authors' personal data.
Corpus tests report that they were skipped when the variable is absent, so the
suite passes without it. See `tests/expectations/README.md` for how per-file
expectations work.

## How the format works

[`FORMAT-NOTES.md`](FORMAT-NOTES.md) documents the container: the four layers,
the framing tag table, how tree roles are decided, the property vocabulary, the
operation log and its sequencing — and, explicitly, what is still unknown. Each
claim is marked **observed** or **assumed**, because a parser that forgets which
of its beliefs were guesses becomes quietly wrong two format versions later.

The same knowledge also sits in the module documentation, next to the code that
depends on it:

| Module | Covers |
| --- | --- |
| `src/gzip.rs` | member discovery and bounded inflation |
| `src/discovery/fragment_scanner.rs` | finding JSON inside binary framing |
| `src/discovery/envelope.rs` | the framing itself, including the tag table |
| `src/fluid/merge_tree.rs` | chunks, tree assembly, deduplication, tree roles |
| `src/fluid/properties.rs` | the property vocabulary and what each key means |
| `src/document/reconstruct.rs` | how segments become blocks |
| `src/operations/log.rs` | the operation log and how it is addressed |

Read the relevant module before changing how something is parsed, and see
[`CONTRIBUTING.md`](CONTRIBUTING.md) for why the observed/assumed distinction is
enforced.
