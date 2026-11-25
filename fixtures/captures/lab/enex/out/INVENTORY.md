# Evernote ENEX inventory

Read-only Evernote `.enex` archaeology. Source files are never modified, external DTDs are never fetched, and raw author/email/GPS values are never copied into this inventory.

| Field | Value |
| --- | --- |
| Root | `/CAPTURE_ROOT/src/mini-enex` |
| ENEX files | 2 |
| Notes | 4 |
| Resources | 5 (122 decoded bytes) |

## ENEX files

| File | Notes | Application | Version | Export date | SHA-256 |
| --- | ---: | --- | --- | --- | --- |
| `clips.enex` | 2 | Evernote | 10.85.4 | 20260410T120000Z | `6f3e30416bfc13aa40c50801e97aa2e9891d3cf4afa540434dcc5942a5b1aa5c` |
| `notebook.enex` | 2 | Evernote | 10.85.4 | 20260410T120000Z | `4ba7cd9d20097f01898f201a162d4e4b649258a29ae8b370144bfe60d81411cd` |

## Construct dispositions

| Construct | Class | Count |
| --- | --- | ---: |
| <en-export> metadata (export-date, application, version) | `unsupported` | 2 |
| .enex notebook file (<notebook>.enex) | `transformed` | 2 |
| <note> entry and <title> | `transformed` | 4 |
| <created> (YYYYMMDDTHHMMSSZ) | `transformed` | 4 |
| <updated> (YYYYMMDDTHHMMSSZ) | `unsupported` | 4 |
| <tag> | `exact` | 4 |
| <note-attributes> author / source / source-url | `unsupported` | 9 |
| <note-attributes> latitude / longitude | `unsupported` | 1 |
| <note-attributes> reminder-order | `unsupported` | 1 |
| <note-attributes> content-class | `unsupported` | 2 |
| <content> CDATA + DOCTYPE wrapper | `transformed` | 4 |
| Standard ENML markup | `transformed` | 42 |
| <table> | `transformed` | 1 |
| <en-todo> (checked/unchecked) | `transformed` | 2 |
| <en-media> image matching <resource> | `transformed` | 3 |
| <en-media> non-image matching <resource> | `transformed` | 1 |
| <resource> without <file-name> | `transformed` | 1 |
| <resource> with <recognition> OCR XML | `unsupported` | 1 |
| Dangling <en-media> (no matching <resource>) | `human_review` | 1 |
| Unreferenced <resource> | `human_review` | 1 |
| <en-crypt> | `human_review` | 1 |
| Internal evernote:// note link | `human_review` | 1 |
| Unsafe HTML (script/iframe/style/object/embed/form/on*/javascript:) | `human_review` | 0 |
| Literal Boris-syntax text ([[, {{, <Capitalized) | `transformed` | 3 |

## Tag histogram

| Tag | Count |
| --- | ---: |
| `architecture` | 1 |
| `clips` | 1 |
| `docs` | 1 |
| `review` | 1 |

## Resources by MIME type

| MIME type | Count | Decoded bytes |
| --- | ---: | ---: |
| `application/pdf` | 1 | 26 |
| `image/jpeg` | 1 | 20 |
| `image/png` | 2 | 29 |
| `text/plain` | 1 | 47 |

## Resource hygiene

| Metric | Count |
| --- | ---: |
| Missing `<file-name>` | 1 |
| `<recognition>` OCR blocks | 1 |
| Unreferenced `<resource>` | 1 |
| Dangling `<en-media>` | 1 |

## `<note-attributes>` presence counts

| Field | Notes with field |
| --- | ---: |
| `author` | 3 |
| `source` | 4 |
| `source-url` | 2 |
| `latitude` | 1 |
| `longitude` | 1 |
| `reminder-order` | 1 |
| `content-class` | 2 |

Raw `author`, email, and GPS coordinate values are excluded from inventory output. Machine-readable twin: `inventory.json`.
