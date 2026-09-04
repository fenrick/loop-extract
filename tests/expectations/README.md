# Recorded expectations

One JSON file per corpus file, named `<corpus filename>.json`. The corpus tests
check a file when one exists and skip it otherwise, so a fixture can be added
without changing any test code.

## Where they live

| Directory | Committed | Holds |
| --- | --- | --- |
| `tests/expectations/` | yes | this README and `EXAMPLE.loop.json`, a synthetic file documenting the schema |
| `tests/expectations/private/` | no, gitignored | expectations that name or quote real documents |

Expectation files are checked in `private/` first, then here. Real expectations
name documents and quote their text, so they stay beside the documents they
describe rather than in source control. `EXAMPLE.loop.json` matches no real file
and is never exercised; it exists so the schema is readable without the private
directory.

These values are hand-reviewed readings of a document, established by inspecting
the extracted trees. They are **not** canonical Microsoft Loop export output.
When a genuine Loop-exported Markdown file becomes available for a fixture, it
should be compared separately and labelled as such.

## Fields

All optional; only the fields present are checked.

| Field | Meaning |
| --- | --- |
| `merge_trees` | number of trees after exact deduplication |
| `body_total_chars` | `totalLengthChars` of the body tree |
| `body_segment_count` | assembled segment count of the body tree |
| `body_contains` | strings the body text must contain |
| `title` | text of a title candidate, trimmed |
| `structural_title` | text of the title the container framing identifies |
| `structurally_classified` | whether roles came from the framing rather than heuristics |
| `embedded_component_trees` | trees the framing attributes to embedded components |
| `content_source` | `snapshot`, `operations` or `snapshot-plus-operations` |
| `has_bold` | whether the body carries `format!bold` |
| `has_lists` | whether the body carries `list!reference` |
| `markdown_first_line` | exact first line of rendered Markdown |
| `markdown_lines` | lines the rendered Markdown must contain exactly |
| `markdown_contains` | substrings the rendered Markdown must contain |
| `markdown_min_list_depth` | deepest bullet nesting the Markdown must reach |
