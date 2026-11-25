# Hugo inventory

Read-only project scan. The Hugo source tree is never modified;
templates and shortcode bodies are scanned as text, never executed.

| Field | Value |
| --- | --- |
| Root | `/CAPTURE_ROOT/src/mini-hugo` |
| Config | `config.toml` (toml) |
| Site title | Mini Hugo Lab Site |
| baseURL | https://example.com/ |
| Default language | en |

## Languages

| Code | Name | Content dir | Weight |
| --- | --- | --- | ---: |
| `en` | English | `content/en` | 1 |
| `fr` | Français | `content/fr` | 2 |

## Counts

| Kind | Count |
| --- | ---: |
| Pages | 12 |
| Sections | 5 |
| Leaf bundles | 2 |
| Branch bundles | 1 |
| Single pages | 4 |
| Page resources | 2 |
| Aliases | 2 |
| Languages | 2 |
| Static assets | 2 |
| Support-dir files | 0 |
| Unclassified | 0 |

## Shortcode usage

| Name | Delimiter | Count |
| --- | --- | ---: |
| `callout` | `{{% …` | 1 |
| `callout` | `{{< …` | 1 |
| `figure` | `{{< …` | 1 |
| `ref` | `{{< …` | 1 |
| `relref` | `{{< …` | 1 |

Shortcodes inside fenced code blocks (not counted): `figure`

## Pages

| Path | Lang | Kind | Depth | Class | FM | FM keys | Aliases | Resources | Shortcodes |
| --- | --- | --- | ---: | --- | --- | --- | --- | --- | --- |
| `content/en/_index.md` | en | section | 0 | transformed | yaml | `description`, `title` |  |  |  |
| `content/en/about.md` | en | single | 0 | transformed | yaml | `categories`, `date`, `tags`, `title` |  |  | `ref`, `relref` |
| `content/en/guides/_index.md` | en | branch_bundle | 1 | unsupported | yaml | `title`, `weight` |  | `cover.png` |  |
| `content/en/guides/inner/_index.md` | en | section | 2 | human_review | yaml | `title` |  |  |  |
| `content/en/guides/inner/deep-page.md` | en | single | 2 | transformed | yaml | `date`, `title` |  |  |  |
| `content/en/posts/_index.md` | en | section | 1 | unsupported | yaml | `title`, `weight` |  |  |  |
| `content/en/posts/draft-note.md` | en | single | 1 | human_review | yaml | `date`, `draft`, `tags`, `title` |  |  | `callout` |
| `content/en/posts/hello-bundle/index.md` | en | leaf_bundle | 2 | unsupported | toml | `aliases`, `categories`, `date`, `draft`, `slug`, `tags`, `title`, `weight` | `/legacy/hb`, `/old/hello-bundle/` | `banner.png` | `figure` |
| `content/en/posts/markup-shortcode.md` | en | single | 1 | human_review | yaml | `date`, `title` |  |  | `callout` |
| `content/fr/_index.md` | fr | section | 0 | transformed | yaml | `title` |  |  |  |
| `content/fr/posts/_index.md` | fr | section | 1 | transformed | yaml | `title` |  |  |  |
| `content/fr/posts/bonjour/index.md` | fr | leaf_bundle | 2 | transformed | toml | `date`, `draft`, `tags`, `title` |  |  |  |

## Static assets

- `static/assets/site-logo.png`
- `static/files/notes.txt`

## Layout shortcodes (layouts/shortcodes/)

- `callout`

Machine-readable twin: `inventory.json`. Disposition contract: `docs/contracts/hugo.md`.
