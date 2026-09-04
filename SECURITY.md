# Security policy

## Reporting a vulnerability

Report privately through GitHub's **Report a vulnerability** button on the
Security tab of this repository. That opens a private advisory visible only to
the maintainers.

Please do not open a public issue for a vulnerability.

A useful report includes what you did, what happened, and the smallest input
that reproduces it. If the input is a real Loop document, describe its shape
rather than attaching it — a `.loop` file carries its authors' names and email
addresses (see [Handling Loop files](#handling-loop-files) below).

Expect an acknowledgement within a week. This is a small tool maintained
alongside other work, so please allow reasonable time before disclosing.

## Scope

This tool parses untrusted binary input, which is the main thing worth attacking.
In scope:

- memory exhaustion or unbounded allocation from a crafted container;
- a panic, hang or unbounded loop reached from a crafted container;
- reading outside a buffer, or any unsoundness;
- attribution or other personal data appearing in output that is documented as
  not carrying it.

Out of scope:

- inaccurate reconstruction of a document. The format is undocumented and parsed
  from observation; wrong output is a bug, but not a vulnerability. Open an
  issue.

## What is already done about it

- **Bounded decompression.** Each gzip member is capped, and so is the total
  across a file (`--max-member-bytes`, `--max-total-bytes`). A member that hits
  the cap is truncated and reported.
- **Bounded scanning.** The fragment scanner limits how far it looks back for an
  enclosing object, how many candidates it tries, how large an object it will
  match, and how many payloads it accepts from one member.
- **Bounded nesting.** The framing reader refuses input nested deeper than 256
  levels rather than exhausting the stack.
- **No guessing on unknown input.** An unrecognised framing tag stops the decode
  and falls back to content heuristics, rather than assuming a field width and
  misreading everything after it.
- **No network access.** The tool makes no outbound connections. It needs no
  Microsoft 365, OneDrive, SharePoint or Graph access.

The code has not been fuzzed. That is the obvious next step for anyone wanting
to harden it, and `parse` in `src/discovery/envelope.rs` plus `match_object` in
`src/discovery/fragment_scanner.rs` are the two entry points worth targeting.

## Handling Loop files

A `.loop` container is a personnel record as well as a document. Nearly every
paragraph carries its author's display name, email address, directory object id
and an edit timestamp; a single page of meeting notes can hold well over a
hundred such records.

Consequences worth knowing:

- `inspect --dump-payloads` writes raw payloads and **does** include all of it.
  Treat the output directory as you would the source file.
- Never attach a real `.loop` file to an issue or a pull request.
- Never commit one. The repository ignores the fixture directories used for
  local testing.
