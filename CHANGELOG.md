# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

## [0.1.0]

First release. Reads Microsoft Loop `.loop` and Fluid `.fluid` containers and
reconstructs the document inside them, entirely locally.

### Added

- `extract` — reconstructs a document as Markdown (default), plain text or JSON.
  `--output` writes to a file; the document goes to stdout and diagnostics to
  stderr.
- `inspect` — reports what a container holds: gzip members and their offsets,
  embedded payloads, merge trees with their roles and content hashes, the
  operation log, every property key encountered, and warnings.
  `--dump-payloads` writes each inflated member and discovered JSON payload to a
  directory under deterministic names, for reverse-engineering work.
- Gzip member discovery with bounded decompression. A coincidental gzip
  signature is recorded and skipped rather than treated as a failure.
- An anchor-driven JSON fragment scanner. A decompressed member is binary-framed
  rather than being JSON, so payloads are found by field name and quote-aware
  brace matching, taking the smallest enclosing object.
- A narrow reader for the container framing, recovering the Fluid channel path
  of each payload. This is what distinguishes a page title from a table cell,
  which are otherwise structurally identical.
- Merge-tree assembly: split trees are reassembled in segment order, and
  byte-identical duplicates are collapsed by SHA-256. Two chunks claiming one
  position are reported rather than merged.
- Operation-log replay for pages whose snapshot holds no body, seeded from the
  snapshot where the sequence numbers prove the log begins after it. Insert and
  remove are applied; annotate is parsed, counted and reported.
- A format-independent document model — paragraphs, headings, list items with
  depth, embedded-component markers, and inline text, bold, italic and links —
  with unrecognised properties preserved rather than discarded.
- Structural warnings carrying stable codes, so callers and tests can assert on
  them.

### Security

- Attribution is never rendered unless asked for. A Loop container records an
  author on nearly every paragraph, with display name, email address, directory
  object id and edit timestamp. Markdown and text output never carry it; JSON
  omits it unless `--include-attribution` is passed. The parser retains it, so
  filtering is a choice made at output time rather than information lost at
  parse time.
- Decompression is bounded per member and in total, so a malformed or hostile
  container cannot exhaust memory.
- The framing reader stops on a tag it does not recognise rather than guessing a
  width, because guessing would silently misread everything after it.

### Known limitations

- Embedded components are not rendered. Tables, images, `@`-mentions and tables
  of contents appear in the body as placeholder markers; their content reaches
  JSON output but not Markdown.
- `Annotate` operations are parsed but not applied, so formatting applied after
  the last snapshot may be missing.
- Ordered lists are rendered as bullets. No test file contains a numbered list,
  so the mapping has not been established and is not guessed.
