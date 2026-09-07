# Notes on the Microsoft Loop container format

Microsoft Loop stores a page as a `.loop` file, and older Fluid-based documents
as `.fluid`. Neither is a document format in the usual sense: both are binary
containers built on the [Fluid Framework](https://fluidframework.com/), holding
a snapshot of collaborative editing state rather than a rendered document.

This file records what the format looks like from the outside. **No published
specification exists.** Everything here was worked out by reading real files,
and each claim is marked as one of two things:

- **Observed** — seen consistently in real files, and something you can verify.
- **Assumed** — a reading that fits, but which no test demonstrates. Nothing in
  this tool's output is allowed to depend on an assumption.

That distinction is the whole discipline. A parser that forgets which of its
beliefs were guesses becomes quietly wrong two format versions later.

## The shape of a file

Four layers, each independent of the next:

```
.loop file
  └── gzip members, laid end to end          ← found by signature scan
        └── a tagged binary value stream      ← the "envelope"
              ├── a summary tree of named blobs
              └── the blobs themselves, most holding JSON
                    └── Fluid merge trees     ← the document content
```

A `.loop` file is not text and does not decode as UTF-8. `file` reports `data`.

## Layer 1: gzip members

**Observed.** A container holds many independent gzip members laid end to end.
There is no index; members are found by scanning for the gzip magic `1F 8B 08`
and attempting to inflate each hit. Counts in the tens to low hundreds are
normal, and a large page can inflate to several megabytes in total.

**Assumed.** A `1F 8B 08` sequence that fails to inflate is a coincidental match
inside compressed data rather than a corrupt member. Either way the right
behaviour is to record it and move on: a failed candidate must never stop the
scan.

Practical consequences:

- inflate under an explicit size cap, per member and in total. A container is
  untrusted input and a decompression bomb is cheap to build;
- expect the same content to appear more than once. See
  [Duplication](#duplication).

## Layer 2: the envelope

A decompressed member is **not** JSON. Parsing one with a JSON parser fails
within the first few dozen bytes. It is a tagged value stream with an interned
string table, which decodes to a Fluid summary:

```text
{
  mrv, cv, lsn,
  snapshot: { id, sequenceNumber, treeNodes: [...] },
  blobs: [ { id, data }, ... ],
  deltas: { firstSequenceNumber, deltas: [...] }
}
```

`treeNodes` is a tree of `{name, children}` and `{name, nodeType, value}` nodes
where `value` is a content id. `blobs` is a flat list holding the content those
ids address. Walking the tree gives every blob a path:

```text
/.app/.channels/E/.channels/text/content/header
/.app/.channels/<guid>/.channels/text/content/body
/.protocol/quorumMembers
```

### Tag table

**Observed**, by decoding real files to completion — the evidence being that a
correct table consumes a member with no bytes left over.

| Tag | Meaning |
| --- | --- |
| `0x01` | integer zero |
| `0x03` / `0x05` / `0x07` | integer, `u8` / `u16` LE / `u32` LE |
| `0x0b` / `0x0c` / `0x0d` | `true` / `false` / `null` |
| `0x0e` / `0x0f` / `0x10` | string literal, length `u8` / `u16` LE / `u32` LE |
| `0x11` / `0x12` / `0x13` | interned string reference, id `u8` / `u16` LE / `u32` LE |
| `0x14` | intern a string: `<u8 id> <u8 len> <bytes>` |
| `0x15` | intern a string, wide: `<u32 id> <u32 len> <bytes>` |
| `0x21` / `0x22` / `0x23` | blob, length `u8` / `u16` LE / `u32` LE |
| `0x31` / `0x32` | open / close array |
| `0x33` / `0x34` | open / close map |

**Assumed.** The `0x07`, `0x10`, `0x13` and `0x23` widths are extrapolated from
the pattern of their narrower siblings. They have not been seen in a real file,
so they are untested.

Notes that cost real time when working this out:

- **A string definition yields no value.** `0x14` and `0x15` populate the table
  and are skipped by the value reader. They appear anywhere, including between a
  map's last entry and its closing tag.
- **The wide forms exist because tables outgrow one byte.** A page with many
  embedded components passes 255 interned strings and switches to `0x15` / `0x12`
  partway through.
- **`0x34 0x33` reads as the ASCII text `43`.** Close-map followed by open-map is
  two printable digits, and `0x34 0x32` reads as `42`. A hex dump of an envelope
  appears to contain stray decimal numbers. It does not.
- **Stop on an unknown tag.** Guessing a width silently misreads every byte
  after it, and the result looks like data rather than an error.

### What the envelope is for

Content alone cannot tell you what a piece of text *is*. A page title and a
table cell are structurally identical — same fields, same shape, same absence of
distinguishing properties. The envelope is what disambiguates them, because it
says which channel each blob belongs to.

**Observed.** Each channel's `.component` blob names the component that owns it:

```json
{"pkg": "[\"Root\",\"LoopCanvasComponentSingleton\"]", "summaryFormatVersion": 2}
```

The last element of `pkg` is the component. Names seen in real files:

| Package | What it owns |
| --- | --- |
| `LoopCanvasComponentSingleton` | the page body |
| `LoopPageTitleSingleton` | the page title |
| `LoopPageHeaderComponentSingleton` | the header component that owns the title |
| `TableroComponentType`, `@ms/tablero/*` | a table, and its data and view models |
| `BlockCalloutComponentType` | a callout card |
| `AtMentionsComponentType` | an `@`-mention |
| `IRichTextData`, `@ms/dias/DiasComponent` | other embedded components |
| `@ms/scriptor` | the text component in older `.fluid` files |

Fluid handles in the page root corroborate this: `CanvasComponentHandle` points
at the body channel and `HeaderComponentHandle` at the header component.

**Observed, and worth knowing:** the header handle points at the *component*,
while the title's merge tree lives in a **sibling** channel whose id differs from
the handle's target by one character. The handle alone therefore does not find
the title. The package name does.

**Assumed.** Two ids recur unchanged across unrelated files written months
apart, which suggests they are stable identifiers for the Loop header component
rather than per-document values. Treat that as a diagnostic hint, never as a
lookup key.

Container identity also appears in the envelope, as a package record:
`@fluidx/loop-page-container` in Loop files from late 2024 onward, and
`@ms/office-fluid-container` in older ones. The version alongside it is a build
identifier, not semantic, and **not** reliably date-based: `0.0.1` and
`20250129024` both occur, and the newest files are not the highest-numbered.

## Layer 3: merge trees

Document content lives in Fluid **merge-tree chunk summaries**, JSON blobs of
this shape:

```json
{
  "chunkStartSegmentIndex": 80,
  "chunkSegmentCount": 54,
  "chunkLengthChars": 4637,
  "totalLengthChars": 14654,
  "totalSegmentCount": 134,
  "chunkSequenceNumber": 304,
  "segmentTexts": [ ... ],
  "headerMetadata": { "orderedChunkMetadata": [{"id": "header"}], ... }
}
```

**Observed.** `headerMetadata.orderedChunkMetadata[].id` is `"header"` on
*every* tree. It names the chunk, not the tree's role, and classifies nothing.
This is an easy and costly trap.

### Chunking

**Observed.** A tree may be split across chunks. A 134-segment body can be
stored as segments 0..79 in one blob and 80..133 in another, both declaring
`totalSegmentCount: 134`. Treating each chunk as a document loses more than a
third of such a file.

The split follows the SharedString summary convention: the first chunk sits under
a blob path ending `header`, the remainder under one ending `body`.

### Segments

`segmentTexts` is an ordered array mixing three shapes:

```json
[
  "a bare string, which is text with no properties",
  {"text": "text with properties", "props": {"format!bold": true}},
  {"marker": {"refType": 1}, "props": {"nodeType": "Paragraph"}}
]
```

**Observed.** Bare strings occur alongside objects in the same array. A parser
that assumes every entry is an object drops text.

### How segments become blocks

**Observed**, consistently, across every file carrying body text:

**A marker terminates the text run before it, and carries that block's
properties** — the way a paragraph mark does in a word processor.

```text
TEXT  "A bold lead-in: "        {format!bold: true}
TEXT  "the sentence that follows"  {}
MARK                            {nodeType: Paragraph,
                                 list!reference: "0;list!list-<guid>0"}
```

That is one list item at depth 0, whose content is bold text followed by plain
text. Two consequences:

- a body often opens with markers that have nothing before them. Those are real
  structure — an empty leading paragraph, or a widget such as a table of
  contents — not noise to be skipped;
- text after the final marker still forms a block.

## Properties

Keys follow a `namespace!name` convention. Keys without `!` are structural.

**Observed and understood well enough to render:**

| Key | Meaning |
| --- | --- |
| `format!bold`, `format!italic` | inline emphasis |
| `style!reference` | block style; `Heading 1`, `Heading 2`, `Body` seen |
| `list!reference` | list membership and depth; see below |
| `hyperlink!url` | link target |
| `nodeType` | block kind; `Paragraph`, `TableOfContents`, `FluidComponent`, `GuestFluidComponent`, `Image` seen |

**Observed but not rendered:** `markerId`, `attribution`, `list!id`,
`list!itemFormat-0`..`-8`, `list!list-<guid><n>`, `paragraph!textAlignment`,
`paragraph!writingMode`, `placeholder!*`, `proofing!spellingError`,
`proofing!grammarError`, `content!locale`, `content!detectedLocale`,
`component!display|height|width|mimeType|routerInput|url|urlTitle`,
`tab!indentCountLeft`, `tab!indentCountRight`, `aria!role`.

### `list!reference`

Values take the form `"<n>;list!list-<guid><m>"`, for example
`"1;list!list-daa4d3e9-…-fd030"`.

**Assumed.** The leading integer is the nesting depth. This fits every observed
document, but is not proven.

**Unknown.** The trailing digit on the list id. Two ids differing only in that
digit occur in one document, and their relationship is unestablished. A change of
list id does start a new list.

### `list!itemFormat-<n>`

Nine of these appear per list, one per nesting level, in the root map rather than
on segments. Values look like
`%cssClass-scriptor-listItem-marker-bullet;%cd0`.

**Observed.** Every value seen so far specifies a bullet. **No numbered list has
been seen**, so the mapping from these values to ordered lists is unknown and
should not be guessed.

### `@`-prefixed keys

**Observed.** A key such as `"@hyperlink!url"` holds an *attribution record*, not
a URL:

```json
{"@hyperlink!url": {"id": "<address>", "name": "<name>", "timestamp": 1234567890}}
```

**Assumed.** A leading `@` marks per-property attribution — who set the property
named after it. Only one such key has been seen, so the generalisation is
unproven. It is safe either way, because the only behaviour depending on it is
treating the value as personal data.

## Attribution

**Observed.** A container records an author on nearly every segment:

```json
{"id": "<address>", "name": "<name>", "email": "<address>",
 "oid": "<directory object id>", "timestamp": 1788326100000, "dataSource": 0}
```

A single page of meeting notes can hold well over a hundred such records. A
`.loop` file is therefore a personnel record as well as a document, and a
Markdown export made by Loop itself contains **none** of it — so an export is
not a faithful copy where authorship or timing matters.

## Duplication

**Observed.** Every merge tree appears at least twice in a container: once inside
a large aggregate member and once as a standalone member, byte for byte
identical. Concatenating everything you find duplicates the whole document.

Deduplicate by content hash. Two refinements matter:

- **Chunk counters do not identify a tree.** In a page built around a table,
  dozens of one-segment cell trees share identical `totalSegmentCount`,
  `totalLengthChars` *and* `chunkSequenceNumber`. Grouping on those alone merges
  unrelated content. A chunk that already holds its whole tree
  (`chunkStartSegmentIndex == 0` and `chunkSegmentCount == totalSegmentCount`) is
  best identified by the hash of its own bytes; only genuinely split trees need
  grouping by counters.
- **Only one copy carries the framing.** The copy inside the aggregate member has
  a summary-tree path; the standalone copy does not, and it is not always the one
  a naive deduplication keeps. Take the placement from whichever copy has it.

Two chunks claiming the same position with different content should be reported,
never merged.

## The operation log

**Observed.** `deltas.deltas` holds sequenced messages, each a JSON document
**as a string**, whose `contents` is itself a JSON-encoded string nested several
levels deep and addressed as it descends:

```json
{"contents": {"type": "component",
  "contents": {"address": "<channel id>",
    "contents": {"content": {"address": "text",
      "contents": {"pos1": 1, "seg": {"text": "typed text", "props": {…}}, "type": 0}},
    "type": "op"}}}}
```

The outer `address` is the channel; the inner one names the data structure within
it, seen as `text`. Delta type numbers follow Fluid: `0` insert, `1` remove,
`2` annotate, `3` group. Group operations nest ordinary ones inside themselves.

**There is no LZ4 anywhere.** Operations are plain JSON. An insert's `seg` value
has exactly the same shape as a snapshot segment, so one segment parser handles
both.

**Observed.** Positions count **UTF-16 code units**, not bytes and not Unicode
scalar values, because the runtime that wrote them is JavaScript. A marker
occupies one unit. Getting this wrong shifts text whenever a document contains
an emoji or any other astral-plane character.

### Why the log matters

**Observed.** Some pages have an effectively empty snapshot and hold their entire
visible content in operations — a snapshot of a few empty markers, and every
paragraph arriving as an insert. A snapshot-only reader returns nothing for
those files while reporting success.

### Replaying safely

**Observed.** In every file carrying both,
`deltas.firstSequenceNumber == snapshot.sequenceNumber + 1`. The log begins
exactly where the snapshot ends, so replaying it on top of the snapshot applies
nothing twice.

That equality is worth checking rather than assuming, because it is what makes
seeding sound. Seeding also matters in practice: an operation-backed page's first
insert lands at position 1, *after* the snapshot's first marker. Replay from
empty and every position is out of range, everything is appended, and content can
duplicate.

## What is still unknown

- **Ordered lists.** No numbered list has been seen, so the `list!itemFormat`
  encoding for one is unknown.
- **The trailing digit on a list id.**
- **How embedded components attach to their placeholder.** A body marker such as
  `FluidComponent` says a component sits at that position; the component's content
  is in another channel. The join between marker and channel has not been
  established, so a table's cells can be extracted but not placed back into the
  page.
- **Whether `annotate` operations are needed.** They occur, but no observed file
  requires them to recover its text.
- **The oldest containers.** Some 2023-era `.fluid` files have no member carrying
  a `treeNodes` summary at all, and their layout has not been examined.

## Exploring a file yourself

`inspect` reports the layers without reconstructing a document:

```bash
loop-extract inspect page.loop --verbose
loop-extract inspect page.loop --dump-payloads ./debug/
```

The dump writes each inflated member verbatim and each JSON payload found inside
it, under deterministic names, so a diff between two files — or two Loop
versions — is meaningful. It contains the document's full text and its authors'
personal data; treat the directory as you would the source file.

The `properties` section of an `inspect` report is the early-warning system: a
new Loop version showing up as `unknown` property keys is how you notice a format
change before it becomes wrong output.
