# Hugo → Boris conversion report

Source root: `/CAPTURE_ROOT/src/mini-hugo` · config: `config.toml` · format `boris-hugo-migration-lab` v1

| Metric | Count |
|--------|-------|
| Pages emitted | 12 |
| Trunks | 6 |
| Satellites | 6 |
| Synthesized section landings | 0 |
| exact / transformed / unsupported / human_review | 0 / 5 / 3 / 4 |
| Shortcodes transformed / preserved | 3 / 2 |
| Page assets | 2 |
| Static assets | 2 |
| Redirect entries | 3 |

## Pages

| Entity id | Source | Role | Parent | Class |
|-----------|--------|------|--------|-------|
| `en` | content/en/_index.md | trunk | — | transformed |
| `en/about` | content/en/about.md | trunk | — | transformed |
| `en/guides` | content/en/guides/_index.md | trunk | — | unsupported |
| `en/guides/inner` | content/en/guides/inner/_index.md | satellite | en/guides | human_review |
| `en/guides/inner/deep-page` | content/en/guides/inner/deep-page.md | satellite | en/guides | human_review |
| `en/posts` | content/en/posts/_index.md | trunk | — | unsupported |
| `en/posts/draft-note` | content/en/posts/draft-note.md | satellite | en/posts | human_review |
| `en/posts/hello-bundle` | content/en/posts/hello-bundle/index.md | satellite | en/posts | unsupported |
| `en/posts/markup-shortcode` | content/en/posts/markup-shortcode.md | satellite | en/posts | human_review |
| `fr` | content/fr/_index.md | trunk | — | transformed |
| `fr/posts` | content/fr/posts/_index.md | trunk | — | transformed |
| `fr/posts/bonjour` | content/fr/posts/bonjour/index.md | satellite | fr/posts | transformed |

## Review items

- `en/guides/inner`: nested section guides/inner flattened: Satellite of en/guides (Boris graph is two-level)
- `en/guides/inner/deep-page`: page in nested section guides/inner flattened: Satellite of en/guides (Boris graph is two-level)
- `en/posts/draft-note`: custom {{< callout >}} shortcode preserved (no Boris target)
- `en/posts/markup-shortcode`: custom {{% callout %}} shortcode preserved (no Boris target)

## Redirects (aliases, slug, url)

| Kind | From | To entity id |
|------|------|--------------|
| alias | `/legacy/hb` | `en/posts/hello-bundle` |
| alias | `/old/hello-bundle/` | `en/posts/hello-bundle` |
| slug | `hello-custom-slug` | `en/posts/hello-bundle` |

## Not emitted

- `static/` copied to `assets/` (2 files) for the theme `assets/` tree; not page-owned.
- `layouts/shortcodes/`: callout (never executed)

Machine-readable twins: `report.json`, `redirects.json`, `provenance/<entity-id>.json`. Disposition contract: `docs/contracts/hugo.md`.
