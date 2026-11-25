# Jekyll inventory

Read-only source archaeology. The Jekyll tree is never modified; no
Liquid, Ruby, or `_plugins/*.rb` is executed, and no network is used.

| Field | Value |
| --- | --- |
| Source root | `/CAPTURE_ROOT/src/mini-jekyll` |
| baseurl | `` |
| permalink | `pretty` |

## Construct dispositions

| Construct | Class | Count |
| --- | --- | ---: |
| `_posts/YYYY-MM-DD-slug.md` | `transformed` | 3 |
| `_posts/ file not matching YYYY-MM-DD-slug` | `human_review` | 1 |
| `_drafts/*.md` | `transformed` | 1 |
| `published: false` | `transformed` | 1 |
| `permalink frontmatter` | `transformed` | 2 |
| `categories frontmatter` | `transformed` | 2 |
| `tags frontmatter` | `exact` | 3 |
| `layout frontmatter / _layouts/` | `unsupported` | 4 |
| `_includes/ + {% include %}` | `unsupported` | 2 |
| `_plugins/*.rb` | `unsupported` | 1 |
| `Custom collection with output: true (_docs/ etc.)` | `transformed` | 2 |
| `Custom collection with output: false` | `unsupported` | 1 |
| `_data/*.yml referenced via {{ site.data.<file>.<key> }}` | `transformed` | 2 |
| `{% post_url YYYY-MM-DD-slug %} (resolves)` | `transformed` | 2 |
| `{% post_url %} (broken target)` | `human_review` | 1 |
| `{% link path %} (resolves)` | `transformed` | 2 |
| `{% link %} (broken target)` | `human_review` | 0 |
| `{{ site.baseurl }}` | `transformed` | 1 |
| `{% raw %}…{% endraw %}` | `exact` | 1 |
| `Liquid tag inside a fenced code block` | `exact` | 1 |
| `Other {% … %} / {{ … }} Liquid` | `human_review` | 0 |
| `Plain top-level page (*.md outside _posts, _drafts, collections)` | `transformed` | 2 |
| `assets/ and other static files` | `transformed` | 2 |
| `_config.yml` | `transformed` | 1 |

## Posts

| Path | Date | Slug | Status | Title |
| --- | --- | --- | --- | --- |
| `_posts/2024-01-02-welcome-post.md` | 2024-01-02 | `welcome-post` | published | Welcome Post |
| `_posts/2024-02-10-permalink-post.md` | 2024-02-10 | `permalink-post` | published | Permalink Post |
| `_posts/2024-03-15-unpublished-post.md` | 2024-03-15 | `unpublished-post` | draft | Unpublished Post |
| `_posts/no-date.md` | — | — | human_review | Not A Post |

## Counts

| Kind | Count |
| --- | ---: |
| Drafts | 1 |
| Pages | 2 |
| Layouts | 2 |
| Includes | 1 |
| Plugins | 1 |
| Data files | 1 |
| Static files | 2 |

## Collections

| Name | output | Members |
| --- | --- | --- |
| `docs` | true | `_docs/guide-one.md`, `_docs/guide-two.md` |
| `internal` | false | `_internal/checklist.md` |

## Liquid tags

| Name | Count |
| --- | ---: |
| `include` | 1 |
| `link` | 2 |
| `post_url` | 3 |
| `site.baseurl` | 1 |
| `site.data.team` | 2 |

Tags inside `{% raw %}…{% endraw %}` and fenced code blocks are not live.

## `_data` references

| Reference | File | Exists | Count |
| --- | --- | --- | ---: |
| `site.data.team` | `_data/team.yml` | yes | 2 |

## Front-matter keys

- `categories`
- `layout`
- `permalink`
- `published`
- `tags`
- `title`

Machine-readable twin: `inventory.json`.
