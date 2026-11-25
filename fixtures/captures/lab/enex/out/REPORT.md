# Evernote ENEX -> Boris conversion report

Candidate Boris Markdown from Evernote `.enex` exports. Source files are never modified; external DTDs are never fetched; `<en-crypt>` blocks are never decrypted.

| Field | Value |
| --- | --- |
| Root | `/CAPTURE_ROOT/src/mini-enex` |
| Notebooks (Trunks) | 2 |
| Notes (Satellites) | 4 |
| Findings | 19 |
| Materialized assets | 4 |

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

## Pages

| Entity ID | Parent | Output | Status | Conversion |
| --- | --- | --- | --- | --- |
| `clips/quick-capture` | `clips` | `content/clips/quick-capture.md` | draft | `unsupported` |
| `clips/web-clip-with-ocr-scan` | `clips` | `content/clips/web-clip-with-ocr-scan.md` | draft | `unsupported` |
| `notebook/architecture-overview` | `notebook` | `content/notebook/architecture-overview.md` | draft | `unsupported` |
| `notebook/encrypted-and-internal-links` | `notebook` | `content/notebook/encrypted-and-internal-links.md` | draft | `human_review` |

## Findings

| Entity ID | Code | Class | Detail |
| --- | --- | --- | --- |
| `clips/quick-capture` | `note_attributes_provenance_only` | `unsupported` | Evernote <note-attributes> kept out of Boris frontmatter and body |
| `clips/quick-capture` | `published_at_needs_summary` | `transformed` | no summary present; Boris requires summary with published_at, <created> kept in provenance |
| `clips/quick-capture` | `updated_provenance_only` | `unsupported` | 20250310T180500Z |
| `clips/web-clip-with-ocr-scan` | `note_attributes_provenance_only` | `unsupported` | Evernote <note-attributes> kept out of Boris frontmatter and body |
| `clips/web-clip-with-ocr-scan` | `published_at_needs_summary` | `transformed` | no summary present; Boris requires summary with published_at, <created> kept in provenance |
| `clips/web-clip-with-ocr-scan` | `recognition_ocr_dropped` | `unsupported` | OCR <recognition> block on 912a61bf3c65883bc23d60d4fade3f21 not injected into body |
| `clips/web-clip-with-ocr-scan` | `resource_filename_fallback` | `transformed` | missing file-name (image/jpeg) -> 6e0acc08d8e5.jpg |
| `clips/web-clip-with-ocr-scan` | `updated_provenance_only` | `unsupported` | 20250305T082000Z |
| `notebook/architecture-overview` | `note_attributes_provenance_only` | `unsupported` | Evernote <note-attributes> kept out of Boris frontmatter and body |
| `notebook/architecture-overview` | `published_at_needs_summary` | `transformed` | no summary present; Boris requires summary with published_at, <created> kept in provenance |
| `notebook/architecture-overview` | `updated_provenance_only` | `unsupported` | 20250201T141500Z |
| `notebook/encrypted-and-internal-links` | `boris_syntax_neutralized` | `transformed` | neutralized literal Boris syntax outside fenced code |
| `notebook/encrypted-and-internal-links` | `dangling_en_media` | `human_review` | deadbeefdeadbeefdeadbeefdeadbeef |
| `notebook/encrypted-and-internal-links` | `encrypted_block` | `human_review` | synthetic-hint |
| `notebook/encrypted-and-internal-links` | `evernote_internal_link` | `human_review` | evernote:///view/10001/s1/00000000-0000-4000-8000-000000000001/00000000-0000-4000-8000-000000000001/ |
| `notebook/encrypted-and-internal-links` | `note_attributes_provenance_only` | `unsupported` | Evernote <note-attributes> kept out of Boris frontmatter and body |
| `notebook/encrypted-and-internal-links` | `published_at_needs_summary` | `transformed` | no summary present; Boris requires summary with published_at, <created> kept in provenance |
| `notebook/encrypted-and-internal-links` | `unreferenced_resource` | `human_review` | orphan-handout.pdf (bf9d6d007add12db4be03a82c7e70f83) |
| `notebook/encrypted-and-internal-links` | `updated_provenance_only` | `unsupported` | 20250211T164500Z |

## Materialized assets

| Entity ID | MD5 | MIME | Output | Bytes |
| --- | --- | --- | --- | ---: |
| `clips/web-clip-with-ocr-scan` | `6e0acc08d8e594291c82676e09b12aa4` | `image/jpeg` | `content/clips/web-clip-with-ocr-scan.assets/6e0acc08d8e5.jpg` | 20 |
| `clips/web-clip-with-ocr-scan` | `912a61bf3c65883bc23d60d4fade3f21` | `image/png` | `content/clips/web-clip-with-ocr-scan.assets/scan.png` | 17 |
| `notebook/architecture-overview` | `78dc75bbdaf576f1f4f214d0f0f1b1ea` | `image/png` | `content/notebook/architecture-overview.assets/diagram.png` | 12 |
| `notebook/architecture-overview` | `d16d8ec1f1e82c2e97702fbf474c903d` | `text/plain` | `content/notebook/architecture-overview.assets/notes.txt` | 47 |

Machine-readable twins: `report.json`, `inventory.json`, `provenance_manifest.json`.
