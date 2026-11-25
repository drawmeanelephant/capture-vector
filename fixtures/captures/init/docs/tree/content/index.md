---
title: My Boris Site
tags: [home]
---

# Welcome

This is a fresh [Boris](https://github.com/drawmeanelephant/boris)
documentation site. It already has a page graph: this trunk page,
two guides beneath it, wiki links between them, and one semantic
relation.

Start here:

- [[guides/getting-started]] — add your own pages and watch the graph grow.
- [[guides/publishing]] — turn the site into a verified publication.

## Anatomy of this starter

The tree `boris init` created:

```text
content/
  index.md                    trunk page (this one)
  guides/getting-started.md   satellite, parent: index
  guides/publishing.md        satellite, parent: index
themes/boris/                 the catalog boris theme (graph slots, search UI)
boris.json                    publication profile (GitHub Pages)
standard-site.json            Atmosphere profile (edit the fake DID/URL)
```

The theme renders the graph, not just the prose: tags in the page
header, this page's satellites under "In this section", and the
relation at the foot of [[guides/publishing]] ("Related") and of
[[guides/getting-started]] ("Linked from").

Search is wired up: the compiler publishes
`dist/_boris/search/search-index.json` and this theme ships the
no-dependency browser client that queries it. Press `/` on any page.
