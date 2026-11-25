# WordPress → Boris migration laboratory

Format `boris-wordpress-migration-lab` schema `3` tool `0.4.1`.

Source export: `/CAPTURE_ROOT/src/mini-wxr/export.xml`

Media directory: `/CAPTURE_ROOT/src/mini-wxr/media`

Original WXR/export and media inputs were **not modified**. Verified local media (when `--media` is set) is copied into page-local `{stem}.assets/` trees; see `media_manifest.json` for the audit. No network access. Content is never silently discarded: unsupported items are preserved under `content/_preserved/`.

## Summary

| Metric | Count |
|---|---:|
| Generated pages (posts/pages) | 9 |
| Parent relationships | 2 |
| Links | 3 |
| Media references | 4 |
| Media copied (materialized) | 1 |
| Media missing | 2 |
| Media ambiguous | 0 |
| Media rejected | 1 |
| Missing media (legacy list) | 2 |
| Feature findings | 10 |
| Duplicate slugs | 1 |
| Unsupported items (preserved) | 3 |
| Comments / trackbacks / pingbacks | 0 |
| Taxonomies | 4 |
| High-cardinality taxonomy | no |
| Human review | 4 |
| Provenance records | 12 |

## Authors

| login | display_name | email |
|---|---|---|
| `admin` | Site Admin | `admin@example.com` |
| `writer` | Staff Writer | `writer@example.com` |

## Taxonomies

Stats: total=4 categories=2 tags=2 other=0 high_cardinality=false (threshold 50).

| domain | nicename | name | parent |
|---|---|---|---|
| category | `news` | News | `` |
| category | `releases` | Releases | `news` |
| tag | `boris` | Boris | `` |
| tag | `migration` | Migration | `` |

## Posts and pages

| output | type | title | slug | date | sticky | excerpt | wp status | conversion |
|---|---|---|---|---|---|---|---|---|
| `content/pages.md` | trunk | Pages | `pages` | `` | no | — | publish | **exact** |
| `content/pages/about.md` | page | About | `about` | `2024-01-01 08:00:00` | no | — | publish | **transformed** |
| `content/pages/engineering.md` | page | Engineering | `engineering` | `2024-01-01 10:00:00` | no | — | publish | **human_review** |
| `content/pages/hello-world.md` | page | Hello World (Page) | `hello-world` | `2024-01-18 12:00:00` | no | — | publish | **human_review** |
| `content/pages/team.md` | page | Team | `team` | `2024-01-01 09:00:00` | no | — | publish | **transformed** |
| `content/posts.md` | trunk | Posts | `posts` | `` | no | — | publish | **exact** |
| `content/posts/draft-ideas.md` | post | Draft Ideas | `draft-ideas` | `2024-01-17 09:00:00` | no | — | draft | **human_review** |
| `content/posts/hello-world.md` | post | Hello World | `hello-world` | `2024-01-15 10:00:00` | no | yes | publish | **human_review** |
| `content/posts/release-notes.md` | post | Release Notes | `release-notes` | `2024-01-16 11:00:00` | no | — | publish | **human_review** |

## Proposed Boris frontmatter

### `synthetic:pages` → `pages`

```yaml
---
title: Pages
status: published
tags: [pages]
---
```

- Categories: 
- Tags: `pages`

### `4` → `pages/about`

```yaml
---
title: About
status: published
---
```

### `6` → `pages/engineering`

```yaml
---
title: Engineering
parent: pages
status: published
---
```

### `7` → `pages/hello-world`

```yaml
---
title: "Hello World (Page)"
status: published
---
```

### `5` → `pages/team`

```yaml
---
title: Team
parent: pages/about
status: published
---
```

### `synthetic:posts` → `posts`

```yaml
---
title: Posts
status: published
tags: [posts]
---
```

- Categories: 
- Tags: `posts`

### `3` → `posts/draft-ideas`

```yaml
---
title: Draft Ideas
parent: posts
status: draft
tags: [news]
---
```

- Categories: `news`
- Tags: 

### `1` → `posts/hello-world`

```yaml
---
title: Hello World
parent: posts
status: published
tags: [migration, boris, news]
---
```

- **Excerpt** (preserved in body, not frontmatter):

  > Welcome post

- Categories: `news`
- Tags: `migration`, `boris`

### `2` → `posts/release-notes`

```yaml
---
title: Release Notes
parent: posts
status: published
tags: [boris, releases]
---
```

- Categories: `releases`
- Tags: `boris`

## Parent / page relationships

| child | parent_post_id | parent_entity | reason | confidence | note |
|---|---|---|---|---|---|
| `pages/engineering` | `5` | `pages/team` | wp_post_parent | low | Parent is not a trunk; Boris allows one hop only — proposed parent pages trunk |
| `pages/team` | `4` | `pages/about` | wp_post_parent | medium | Page hierarchy mapped to parent entity id; ensure parent has no parent (trunk) |

## Internal links

| source | kind | target | status | resolved |
|---|---|---|---|---|
| `content/posts/hello-world.md` | internal_href | `https://example.com/about/` | ok | `4` |
| `content/posts/release-notes.md` | internal_href | `https://example.com/gone/` | unresolved | `—` |
| `content/posts/release-notes.md` | internal_href | `https://example.com/hello-world/` | ok | `1` |

## Media references

| source | referenced | local | status |
|---|---|---|---|
| `content/_preserved/attachment-10.md` | `https://example.com/wp-content/uploads/2024/01/hero.png` | `2024/01/hero.png` | present |
| `content/_preserved/attachment-11.md` | `https://example.com/wp-content/uploads/2024/01/missing-asset.png` | `—` | missing |
| `content/posts/hello-world.md` | `https://example.com/wp-content/uploads/2024/01/hero.png` | `2024/01/hero.png` | present |
| `content/posts/release-notes.md` | `https://example.com/wp-content/uploads/2024/01/missing-asset.png` | `—` | missing |

## Media materialization

Machine-readable audit: `media_manifest.json` (byte-stable, sorted).

| status | count |
|---|---:|
| copied | 1 |
| missing | 2 |
| ambiguous | 0 |
| rejected | 1 |

Unresolved / notable entries (missing, ambiguous, rejected, and non-empty reasons):

| source | original | status | reason |
|---|---|---|---|
| `content/_preserved/attachment-10.md` | `https://example.com/wp-content/uploads/2024/01/hero.png` | **rejected** | attachment_inventory_only_not_materialized |
| `content/_preserved/attachment-11.md` | `https://example.com/wp-content/uploads/2024/01/missing-asset.png` | **missing** | local_media_not_found |
| `content/posts/release-notes.md` | `https://example.com/wp-content/uploads/2024/01/missing-asset.png` | **missing** | local_media_not_found |

## Missing media

- `https://example.com/wp-content/uploads/2024/01/missing-asset.png` from `content/posts/release-notes.md` (missing)
- `https://example.com/wp-content/uploads/2024/01/missing-asset.png` from `content/_preserved/attachment-11.md` (missing)

## Raw HTML, shortcodes, embeds, galleries, custom blocks

| source | code | classification | message |
|---|---|---|---|
| `content/posts/draft-ideas.md` | `status_draft` | **human_review** | WordPress status 'draft' mapped to Boris status 'draft'; author review required before publish |
| `content/posts/hello-world.md` | `excerpt_preserved` | **transformed** | WordPress excerpt preserved in Markdown body (not a closed Boris frontmatter field) |
| `content/posts/hello-world.md` | `gallery_or_image_block` | **human_review** | Gallery/image blocks need media path verification |
| `content/posts/hello-world.md` | `gutenberg_block` | **transformed** | Gutenberg block comments detected; block chrome stripped, inner HTML converted |
| `content/posts/release-notes.md` | `embed_or_custom_html_block` | **unsupported** | Embed/custom HTML blocks preserved as raw HTML |
| `content/posts/release-notes.md` | `gutenberg_block` | **transformed** | Gutenberg block comments detected; block chrome stripped, inner HTML converted |
| `content/posts/release-notes.md` | `shortcode_caption` | **unsupported** | Caption shortcode preserved raw (not silently converted to Markdown) |
| `content/posts/release-notes.md` | `shortcode_embed_media` | **unsupported** | Media/embed shortcodes preserved raw (no network expansion) |
| `content/posts/release-notes.md` | `shortcode_gallery` | **unsupported** | Gallery shortcode cannot be expanded offline; preserved raw as unsupported artifact |
| `content/posts/release-notes.md` | `shortcode_generic` | **unsupported** | One or more shortcodes remain as raw unsupported artifacts in body |

## Duplicate slugs

- **`hello-world`** (duplicate_post_name): post_id=1, post_id=7

## Unsupported content (preserved)

- post_id=`10` type=`attachment` title=hero → `content/_preserved/attachment-10.md` — attachment item; not a Boris page — preserved for media inventory
- post_id=`11` type=`attachment` title=missing-asset → `content/_preserved/attachment-11.md` — attachment item; not a Boris page — preserved for media inventory
- post_id=`8` type=`product` title=Product Widget → `content/_preserved/product-8.md` — non post/page post_type; preserved raw for review

## Comments, trackbacks, and pingbacks

These are **not** converted into page Markdown. Full text is under `content/_preserved/comments-*.md`.

_None._

## Human review queue

### `content/pages/engineering.md` (post_id=6)

- Reason: Flagged for human review (status, media, hierarchy, or links)
- Codes: `deep_page_hierarchy`

### `content/posts/draft-ideas.md` (post_id=3)

- Reason: Flagged for human review (status, media, hierarchy, or links)
- Codes: `status_draft`

### `content/posts/hello-world.md` (post_id=1)

- Reason: Flagged for human review (status, media, hierarchy, or links)
- Codes: `gutenberg_block`, `gallery_or_image_block`, `excerpt_preserved`, `media_materialized`

### `content/posts/release-notes.md` (post_id=2)

- Reason: Flagged for human review (status, media, hierarchy, or links)
- Codes: `gutenberg_block`, `embed_or_custom_html_block`, `shortcode_gallery`, `shortcode_caption`, `shortcode_embed_media`, `shortcode_generic`, `shortcode_remaining`, `unresolved_internal_link`, `missing_media`

## Provenance index

Every generated Markdown file carries a provenance HTML comment and appears here.

| output_path | post_id | type | conversion |
|---|---|---|---|
| `content/_preserved/attachment-10.md` | `10` | attachment | unsupported |
| `content/_preserved/attachment-11.md` | `11` | attachment | unsupported |
| `content/_preserved/product-8.md` | `8` | product | unsupported |
| `content/pages.md` | `synthetic:pages` | trunk | exact |
| `content/pages/about.md` | `4` | page | transformed |
| `content/pages/engineering.md` | `6` | page | human_review |
| `content/pages/hello-world.md` | `7` | page | transformed |
| `content/pages/team.md` | `5` | page | transformed |
| `content/posts.md` | `synthetic:posts` | trunk | exact |
| `content/posts/draft-ideas.md` | `3` | post | human_review |
| `content/posts/hello-world.md` | `1` | post | human_review |
| `content/posts/release-notes.md` | `2` | post | human_review |

---

Machine-readable twin: `report.json`.
Generated Markdown under `content/`.
