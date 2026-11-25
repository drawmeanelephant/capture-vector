# Substack → Boris conversion report

Candidate Boris Markdown from an unpacked Substack export. Source `posts.csv`, `posts/*.html`, and media are never modified; `subscribers.csv` and `email_list*.csv` are ignored and never read.

| Field | Value |
| --- | --- |
| Source | `/CAPTURE_ROOT/src/mini-substack/posts.csv` |
| SHA-256 | `27da73dc27b806f2d992111c55963c6966e8b2e17e812545eff5b77a080ac70d` |
| Emitted pages | 8 |
| Missing HTML rows | 1 |
| Orphan HTML files | 1 |
| Findings | 15 |
| Assets copied | 1 |
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

## Pages

| Entity | Post ID | Emitted | Type | Audience | Substack published | Boris status | Conversion |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `posts/draft-outline` | `100003.draft-outline` | yes | newsletter | everyone | no | draft | `transformed` |
| `posts/embeds-and-literals` | `100008.embeds-and-literals` | yes | newsletter | everyone | yes | published | `human_review` |
| `posts/episode-one` | `100006.episode-one` | yes | podcast | everyone | yes | published | `human_review` |
| `posts/field-notes-no-subtitle` | `100002.field-notes-no-subtitle` | yes | newsletter | everyone | yes | published | `human_review` |
| `posts/founding-memo` | `100005.founding-memo` | yes | newsletter | founding | yes | draft | `human_review` |
| `posts/missing-body` | `100009.missing-body` | no | newsletter | everyone | yes | draft | `human_review` |
| `posts/paid-briefing` | `100004.paid-briefing` | yes | newsletter | only_paid | yes | draft | `human_review` |
| `posts/reader-thread` | `100007.reader-thread` | yes | thread | everyone | yes | draft | `human_review` |
| `posts/welcome-to-the-lab` | `100001.welcome-to-the-lab` | yes | newsletter | everyone | yes | published | `human_review` |

## Findings

| Entity | Code | Class | Detail |
| --- | --- | --- | --- |
| `` | `orphan_post_html` | `human_review` | posts/100099.orphan-post.html not listed in posts.csv |
| `posts/embeds-and-literals` | `unsafe_html_withheld` | `human_review` | <iframe src="https://embed.example.com/chart/1" width="600" height="320" title="Synthetic chart"></iframe> |
| `posts/episode-one` | `podcast_episode` | `human_review` | https://api.substack.example.com/feed/podcast/100006/audio.mp3 |
| `posts/field-notes-no-subtitle` | `published_at_needs_summary` | `transformed` | empty subtitle; Boris requires summary whenever published_at is present, date kept in provenance |
| `posts/field-notes-no-subtitle` | `widget_button_stripped` | `unsupported` | button-wrapper stripped |
| `posts/field-notes-no-subtitle` | `widget_digest_embed_withheld` | `human_review` | digest-post-embed withheld: https://lab.example.com/p/welcome-to-the-lab |
| `posts/field-notes-no-subtitle` | `widget_subscription_stripped` | `unsupported` | subscription-widget-wrap stripped |
| `posts/founding-memo` | `restricted_audience` | `human_review` | audience founding: emitted as draft, never published |
| `posts/missing-body` | `missing_post_html` | `human_review` | missing posts/100009.missing-body.html |
| `posts/paid-briefing` | `restricted_audience` | `human_review` | audience only_paid: emitted as draft, never published |
| `posts/paid-briefing` | `widget_paywall_stripped` | `unsupported` | paywall divider stripped |
| `posts/reader-thread` | `thread_replies_absent` | `human_review` | discussion thread replies absent from export; emitted as draft |
| `posts/welcome-to-the-lab` | `footnote_converted` | `transformed` | footnote 1 |
| `posts/welcome-to-the-lab` | `remote_image_not_fetched` | `human_review` | https://substackcdn.example.com/image/fetch/hero-remote.png |
| `posts/welcome-to-the-lab` | `unresolved_cross_post_link` | `human_review` | https://lab.example.com/p/nonexistent-post |

## Assets

| Entity | Source (`posts/`) | Output |
| --- | --- | --- |
| `posts/welcome-to-the-lab` | `images/hero.png` | `content/posts/welcome-to-the-lab.assets/images/hero.png` |

Lab provenance stays in `provenance_manifest.json`, never frontmatter. Machine-readable twin: `report.json`.
