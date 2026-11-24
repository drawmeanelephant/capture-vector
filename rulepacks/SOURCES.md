# Rule-pack sources

<!-- GENERATED from rule citations by `cargo run -p cv-rules --example gen-sources > rulepacks/SOURCES.md`; do not edit by hand. -->

27 rules in 4 packs cite 21 sources. A rule without a citation does not load.

Tiers: `NormativeSpec` (a published standard) and `EngineDocumented` (the Boris compiler contract or a search engine's own documentation) may block a publish when they fail at `Error` severity; `Heuristic` findings are advisory and can never block.

## Boris 0.9.0 contract: Compiler-owned head metadata (docs/contracts/head-metadata.md)

<https://github.com/drawmeanelephant/boris/blob/v0.9.0/docs/contracts/head-metadata.md>

> Canonical is `normalized_base + "/" + percent_encoded(emitted_output_path)`, using the sitemap URL builder and the same path-byte encoding as RSS.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `TGT-CANONICAL-PARITY` | targets | EngineDocumented | Error | 2026-11-23 |

## Boris 0.9.0 contract: Frontmatter, status (docs/contracts/frontmatter.md)

<https://github.com/drawmeanelephant/boris/blob/v0.9.0/docs/contracts/frontmatter.md>

> Standard.site, Nostr, RSS, sitemap, and search exclude drafts; IR and RAG keep every page.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `TGT-RSS-DRAFT-LEAK` | targets | EngineDocumented | Error | 2026-11-23 |
| `TGT-SITEMAP-DRAFT-LEAK` | targets | EngineDocumented | Error | 2026-11-23 |

## Boris 0.9.0 contract: HTML output, Status gating (docs/contracts/html-output.md)

<https://github.com/drawmeanelephant/boris/blob/v0.9.0/docs/contracts/html-output.md#status-gating-on-the-default-html-target-738>

> `status` is a document fact; each projection owns its eligibility rule. On the default HTML target a `status: draft` page is **emitted but not advertised**:

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `TGT-PAGESET-PARITY` | targets | EngineDocumented | Error | 2026-11-23 |

## Boris 0.9.0 reference: Outputs, llms.txt (content/reference/outputs.md)

<https://github.com/drawmeanelephant/boris/blob/v0.9.0/content/reference/outputs.md#llmstxt>

> Entries follow the validated navigation tree and link to the current static routes

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `TGT-LLMS-DRAFT-LEAK` | targets | EngineDocumented | Error | 2026-11-23 |

## Google Search Central: Block search indexing with noindex

<https://developers.google.com/search/docs/crawling-indexing/block-indexing>

> There are two ways to implement noindex: as a <meta> tag and as an HTTP response header. They have the same effect; choose the method that is more convenient for your site and appropriate for the content type.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `SEO-LIVE-X-ROBOTS-TAG` | seo-core | Heuristic | Warning | 2026-11-23 |

## Google Search Central: Build and submit a sitemap

<https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap>

> Include the URLs in your sitemap that you want to see in Google's search results.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `TGT-SITEMAP-NOINDEX-LEAK` | targets | EngineDocumented | Error | 2026-11-23 |

## Google Search Central: How to specify a canonical with rel="canonical"

<https://developers.google.com/search/docs/crawling-indexing/consolidate-duplicate-urls>

> Do include a rel="canonical" link on the canonical page itself (also known as a self-referential canonical).

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `SEO-CANONICAL-PRESENT` | seo-core | Heuristic | Warning | 2026-11-23 |

## Google Search Central: How to write meta descriptions

<https://developers.google.com/search/docs/appearance/snippet>

> A meta description tag generally informs and interests users with a short, relevant summary of what a particular page is about.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `SEO-META-DESCRIPTION` | seo-core | Heuristic | Info | 2026-11-23 |

## Google Search Central: Introduction to structured data markup

<https://developers.google.com/search/docs/appearance/structured-data/intro-structured-data>

> Google uses structured data that it finds on the web to understand the content of the page, as well as to gather information about the web and the world in general

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `AEO-JSONLD-TYPED` | aeo-llm | Heuristic | Info | 2026-11-23 |

## Nielsen Norman Group: Inverted Pyramid: Writing for Comprehension

<https://www.nngroup.com/articles/inverted-pyramid/>

> In journalism, the inverted pyramid refers to a story structure where the most important information (or what might even be considered the conclusion) is presented first.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `AEO-ANSWER-FIRST` | aeo-llm | Heuristic | Info | 2026-11-23 |

## PROMPT-v3 work order: Audit section (spec/PROMPT-v3.md)

<https://github.com/drawmeanelephant/capture-vector/blob/main/spec/PROMPT-v3.md>

> Draft/noindex leakage into sitemap/rss/llms/RAG is a blocking error.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `TGT-LLMS-NOINDEX-LEAK` | targets | NormativeSpec | Error | 2026-11-24 |
| `TGT-RAG-DRAFT-LEAK` | targets | NormativeSpec | Error | 2026-11-24 |
| `TGT-RAG-NOINDEX-LEAK` | targets | NormativeSpec | Error | 2026-11-24 |
| `TGT-RSS-NOINDEX-LEAK` | targets | NormativeSpec | Error | 2026-11-24 |

## RFC 6596 The Canonical Link Relation, section 5

<https://www.rfc-editor.org/rfc/rfc6596#section-5>

> Specify only one canonical link relation for a resource. (It would be confusing to consider/label/designate more than one IRI as authoritative.)

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `SEO-CANONICAL-SINGLE` | seo-core | NormativeSpec | Error | 2026-11-23 |

## RFC 9309 Robots Exclusion Protocol, 2.3 Access Method

<https://www.rfc-editor.org/rfc/rfc9309#section-2.3>

> The rules MUST be accessible in a file named "/robots.txt" (all lowercase) in the top-level path of the service.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `TGT-LIVE-ROBOTS-SERVED` | targets | NormativeSpec | Warning | 2026-11-23 |
| `TGT-ROBOTS-TXT` | targets | NormativeSpec | Error | 2026-11-23 |

## W3C JSON-LD 1.1, 9. JSON-LD Grammar

<https://www.w3.org/TR/json-ld11/#json-ld-grammar>

> A JSON-LD document MUST be valid JSON text as described in [RFC8259]

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `AEO-JSONLD-VALID` | aeo-llm | NormativeSpec | Error | 2026-11-23 |

## WCAG 2.2, Success Criterion 1.1.1 Non-text Content

<https://www.w3.org/TR/WCAG22/#non-text-content>

> All non-text content that is presented to the user has a text alternative that serves the equivalent purpose, except for the situations listed below.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `A11Y-IMG-ALT` | a11y-content | NormativeSpec | Error | 2026-11-23 |

## WCAG 2.2, Success Criterion 2.4.2 Page Titled

<https://www.w3.org/TR/WCAG22/#page-titled>

> Web pages have titles that describe topic or purpose.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `A11Y-PAGE-TITLE` | a11y-content | NormativeSpec | Error | 2026-11-23 |

## WCAG 2.2, Success Criterion 3.1.1 Language of Page

<https://www.w3.org/TR/WCAG22/#language-of-page>

> The default human language of each web page can be programmatically determined.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `A11Y-HTML-LANG` | a11y-content | NormativeSpec | Error | 2026-11-23 |

## WHATWG HTML Living Standard, 4.2.2 The title element

<https://html.spec.whatwg.org/multipage/semantics.html#the-title-element>

> There must be no more than one title element per document.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `SEO-TITLE-SINGLE` | seo-core | NormativeSpec | Error | 2026-11-23 |

## llms.txt proposal (llmstxt.org), Format

<https://llmstxt.org/>

> A blockquote with a short summary of the project, containing key information necessary for understanding the rest of the file

> An H1 with the name of the project or site. This is the only required section

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `AEO-LLMS-TXT-H1` | aeo-llm | Heuristic | Warning | 2026-11-23 |
| `AEO-LLMS-TXT-SUMMARY` | aeo-llm | Heuristic | Info | 2026-11-23 |

## sitemaps.org Sitemap protocol, Specifying the Sitemap location in your robots.txt file

<https://www.sitemaps.org/protocol.html#submit_robots>

> You can specify the location of the Sitemap using a robots.txt file.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `TGT-ROBOTS-SITEMAP` | targets | Heuristic | Info | 2026-11-23 |

## sitemaps.org Sitemap protocol, XML tag definitions

<https://www.sitemaps.org/protocol.html>

> The date of last modification of the page. This date should be in W3C Datetime format.

| Rule | Pack | Tier | Severity | Captured |
|---|---|---|---|---|
| `TGT-LASTMOD-PARITY` | targets | Heuristic | Warning | 2026-11-23 |
