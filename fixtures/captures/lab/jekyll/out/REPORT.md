# Jekyll emit report

Boris Markdown under `content/`; per-page provenance under `provenance/`.
No Liquid, Ruby, or `_plugins/*.rb` was executed and no network was used.
Lab provenance (permalink, categories, layout, raw front matter) never
becomes Boris frontmatter. Machine-readable twin: `report.json`.

| Field | Value |
| --- | --- |
| Source root | `/CAPTURE_ROOT/src/mini-jekyll` |
| Pages emitted | 10 |
| Not emitted | 7 |

## Construct dispositions

| Construct | Class | Count | Emit landing |
| --- | --- | ---: | --- |
| `_posts/YYYY-MM-DD-slug.md` | `transformed` | 3 | Satellite of generated `posts` Trunk; published_at from filename |
| `_posts/ file not matching YYYY-MM-DD-slug` | `human_review` | 1 | not emitted; listed for review |
| `_drafts/*.md` | `transformed` | 1 | Satellite of `posts` with status: draft |
| `published: false` | `transformed` | 1 | status: draft; flag in provenance |
| `permalink frontmatter` | `transformed` | 2 | provenance + route report |
| `categories frontmatter` | `transformed` | 2 | provenance + route report |
| `tags frontmatter` | `exact` | 3 | tags: [] |
| `layout frontmatter / _layouts/` | `unsupported` | 4 | provenance; layouts not emitted |
| `_includes/ + {% include %}` | `unsupported` | 2 | call site kept verbatim; include bodies not inlined |
| `_plugins/*.rb` | `unsupported` | 1 | not emitted; never executed |
| `Custom collection with output: true (_docs/ etc.)` | `transformed` | 2 | Satellites of a generated per-collection Trunk |
| `Custom collection with output: false` | `unsupported` | 1 | not emitted |
| `_data/*.yml referenced via {{ site.data.<file>.<key> }}` | `transformed` | 2 | call site kept verbatim; data not inlined |
| `{% post_url YYYY-MM-DD-slug %} (resolves)` | `transformed` | 2 | [[entity-id]] |
| `{% post_url %} (broken target)` | `human_review` | 1 | kept verbatim; human_review |
| `{% link path %} (resolves)` | `transformed` | 2 | [[entity-id]] or page-local asset |
| `{% link %} (broken target)` | `human_review` | 0 | kept verbatim; human_review |
| `{{ site.baseurl }}` | `transformed` | 1 | `{{ site.baseurl }}/asset` → page-local asset; else verbatim |
| `{% raw %}…{% endraw %}` | `exact` | 1 | verbatim body text |
| `Liquid tag inside a fenced code block` | `exact` | 1 | verbatim body text |
| `Other {% … %} / {{ … }} Liquid` | `human_review` | 0 | kept verbatim; human_review |
| `Plain top-level page (*.md outside _posts, _drafts, collections)` | `transformed` | 2 | Trunk page |
| `assets/ and other static files` | `transformed` | 2 | copied to `{stem}.assets/` when referenced; else listed |
| `_config.yml` | `transformed` | 1 | collections + provenance in report.json |

## Pages

| Entity id | Kind | Parent | Status | Class | Review | Source |
| --- | --- | --- | --- | --- | --- | --- |
| `posts` | generated_trunk | — | published | `transformed` |  | generated |
| `docs` | generated_trunk | — | published | `transformed` |  | generated |
| `posts/welcome-post` | post | posts | published | `unsupported` | layout_not_rendered | `_posts/2024-01-02-welcome-post.md` |
| `posts/permalink-post` | post | posts | published | `transformed` |  | `_posts/2024-02-10-permalink-post.md` |
| `posts/unpublished-post` | post | posts | draft | `transformed` |  | `_posts/2024-03-15-unpublished-post.md` |
| `posts/draft-notes` | draft | posts | draft | `transformed` |  | `_drafts/draft-notes.md` |
| `docs/guide-one` | collection | docs | published | `transformed` |  | `_docs/guide-one.md` |
| `docs/guide-two` | collection | docs | published | `transformed` |  | `_docs/guide-two.md` |
| `about` | page | — | published | `transformed` |  | `about.md` |
| `index` | page | — | published | `human_review` | layout_not_rendered, liquid_include, post_url_unresolved | `index.md` |

## Routes

Jekyll URLs are not carried into Boris; each page routes by entity id.

| Source | Boris route | Jekyll permalink | Categories |
| --- | --- | --- | --- |
| `_config.yml` | — | `pretty` (site scheme) | — |
| `_posts/2024-01-02-welcome-post.md` | `posts/welcome-post.html` | — | news, updates |
| `_posts/2024-02-10-permalink-post.md` | `posts/permalink-post.html` | `/custom/permalink-post/` | news |
| `_posts/2024-03-15-unpublished-post.md` | `posts/unpublished-post.html` | — |  |
| `_drafts/draft-notes.md` | `posts/draft-notes.html` | — |  |
| `_docs/guide-one.md` | `docs/guide-one.html` | — |  |
| `_docs/guide-two.md` | `docs/guide-two.html` | — |  |
| `about.md` | `about.html` | `/about/` |  |
| `index.md` | `index.html` | — |  |

## Liquid call sites

| Source | Line | Construct | Class | Text | Landing |
| --- | ---: | --- | --- | --- | --- |
| `_posts/2024-01-02-welcome-post.md` | 14 | `{% post_url YYYY-MM-DD-slug %} (resolves)` | `transformed` | `{% post_url 2024-02-10-permalink-post %}` | [[posts/permalink-post]] |
| `_docs/guide-one.md` | 7 | `{% link path %} (resolves)` | `transformed` | `{% link _docs/guide-two.md %}` | [[docs/guide-two]] |
| `_docs/guide-two.md` | 7 | `_data/*.yml referenced via {{ site.data.<file>.<key> }}` | `transformed` | `{{ site.data.team.maintainer.name }}` | verbatim; data not inlined |
| `about.md` | 7 | `_data/*.yml referenced via {{ site.data.<file>.<key> }}` | `transformed` | `{{ site.data.team.maintainer.name }}` | verbatim; data not inlined |
| `index.md` | 8 | `_includes/ + {% include %}` | `unsupported` | `{% include note.html text="Included from _includes." %}` | verbatim; include body not inlined |
| `index.md` | 10 | `{% post_url YYYY-MM-DD-slug %} (resolves)` | `transformed` | `{% post_url 2024-01-02-welcome-post %}` | [[posts/welcome-post]] |
| `index.md` | 11 | `{% link path %} (resolves)` | `transformed` | `{% link _docs/guide-one.md %}` | [[docs/guide-one]] |
| `index.md` | 13 | `{% post_url %} (broken target)` | `human_review` | `{% post_url 1999-01-01-ghost-post %}` | verbatim |
| `index.md` | 15 | `{{ site.baseurl }}` | `transformed` | `{{ site.baseurl }}/assets/images/hero.png` | index.assets/images/hero.png |
| `index.md` | 17 | `{% raw %}…{% endraw %}` | `exact` | `{% raw %}` | verbatim |
| `index.md` | 22 | `Liquid tag inside a fenced code block` | `exact` | `{% post_url 2024-01-02-welcome-post %}` | verbatim |

## Assets

| Page | Source | Destination |
| --- | --- | --- |
| `docs/guide-two` | `assets/images/hero.png` | `content/docs/guide-two.assets/images/hero.png` |
| `index` | `assets/images/hero.png` | `content/index.assets/images/hero.png` |

Unreferenced static files (not copied):

- `README.md`

## Excluded

Pruned like Jekyll does: dot/`#`/`~` names, the default `exclude` list, and `_config.yml` `exclude:`.

- none

## Not emitted

| Path | Class | Reason |
| --- | --- | --- |
| `_data/team.yml` | `transformed` | _data file; referenced values are not inlined |
| `_includes/note.html` | `unsupported` | include body; never inlined |
| `_internal/checklist.md` | `unsupported` | collection with output: false; Jekyll writes no page |
| `_layouts/default.html` | `unsupported` | layout; theme machinery is never rendered |
| `_layouts/post.html` | `unsupported` | layout; theme machinery is never rendered |
| `_plugins/shout_filter.rb` | `unsupported` | plugin; never executed |
| `_posts/no-date.md` | `human_review` | post filename does not match YYYY-MM-DD-slug; Jekyll ignores it |
