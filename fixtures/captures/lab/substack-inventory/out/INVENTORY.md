# Substack inventory

Read-only Substack export archaeology. Source `posts.csv`, `posts/*.html`, and media are never modified; `subscribers.csv` and `email_list*.csv` are ignored and never read.

| Field | Value |
| --- | --- |
| Source | `/CAPTURE_ROOT/src/mini-substack/posts.csv` |
| SHA-256 | `27da73dc27b806f2d992111c55963c6966e8b2e17e812545eff5b77a080ac70d` |
| CSV rows | 9 |
| Matched HTML bodies | 8 |
| Missing HTML bodies | 1 |
| Orphan HTML files | 1 |
| Published without subtitle | 1 |
| Ignored PII files | 0 |

## Construct dispositions

| Construct | Class | Count |
| --- | --- | ---: |
| posts.csv newsletter row (published, everyone, post_date + subtitle) | `transformed` | 3 |
| published row with post_date and empty subtitle (published_at_needs_summary) | `transformed` | 1 |
| draft row (is_published=false) | `transformed` | 1 |
| restricted audience (audience != everyone) | `human_review` | 2 |
| podcast row (type=podcast) | `human_review` | 1 |
| thread row (type=thread) | `human_review` | 1 |
| missing_post_html (CSV row missing posts/<post_id>.html) | `human_review` | 1 |
| orphan_post_html (posts/*.html absent from posts.csv) | `human_review` | 1 |
| CSV quoted fields | `exact` | 2 |
| post_id (<numeric-id>.<slug>) | `unsupported` | 9 |
| email_sent_at / inbox_sent_at | `unsupported` | 16 |
| subscribers.csv / email_list.*.csv (ignored PII files) | `unsupported` | 0 |
| standard HTML body | `transformed` | 8 |
| footnotes (anchors + definitions) | `transformed` | 2 |
| captioned image (local existing) | `transformed` | 1 |
| captioned image (local missing, unsafe, or symlink) | `human_review` | 0 |
| captioned image (remote URL) | `human_review` | 1 |
| internal cross-post link (resolved) | `transformed` | 2 |
| internal cross-post link (unresolved) | `human_review` | 1 |
| subscription-widget-wrap | `unsupported` | 1 |
| button-wrapper | `unsupported` | 1 |
| paywall | `unsupported` | 1 |
| digest-post-embed | `human_review` | 1 |
| unsafe HTML | `human_review` | 1 |
| literal Boris-syntax sequences | `transformed` | 3 |

## Posts

| Post ID | Type | Published | Audience | Date | Subtitle | HTML |
| --- | --- | --- | --- | --- | --- | --- |
| `100001.welcome-to-the-lab` | newsletter | yes | everyone | 2024-01-15T14:30:00.000Z | yes | yes |
| `100002.field-notes-no-subtitle` | newsletter | yes | everyone | 2024-02-02T10:00:00.000Z | no | yes |
| `100003.draft-outline` | newsletter | no | everyone | (none) | yes | yes |
| `100004.paid-briefing` | newsletter | yes | only_paid | 2024-03-10T16:00:00.000Z | yes | yes |
| `100005.founding-memo` | newsletter | yes | founding | 2024-03-20T12:00:00.000Z | yes | yes |
| `100006.episode-one` | podcast | yes | everyone | 2024-04-05T09:00:00.000Z | yes | yes |
| `100007.reader-thread` | thread | yes | everyone | 2024-04-18T18:00:00.000Z | yes | yes |
| `100008.embeds-and-literals` | newsletter | yes | everyone | 2024-05-01T11:00:00.000Z | yes | yes |
| `100009.missing-body` | newsletter | yes | everyone | 2024-05-12T15:00:00.000Z | yes | **missing** |

## Reconciliation

- Missing HTML (`missing_post_html`): `100009.missing-body`
- Orphan HTML (`orphan_post_html`): `100099.orphan-post.html`

## Image references

| Post ID | Source | Kind | Captioned |
| --- | --- | --- | --- |
| `100001.welcome-to-the-lab` | `https://substackcdn.example.com/image/fetch/hero-remote.png` | `remote` | yes |
| `100001.welcome-to-the-lab` | `images/hero.png` | `local_existing` | yes |

Subscriber PII files (`subscribers.csv`, `email_list*.csv`) are never opened or read. Machine-readable twin: `inventory.json`.
