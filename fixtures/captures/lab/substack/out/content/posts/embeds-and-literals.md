---
id: posts/embeds-and-literals
title: Embeds and Literal Syntax
parent: posts
status: published
published_at: 2024-05-01T11:00:00Z
summary: Exercises unsafe HTML withholding and Boris syntax escaping.
---

This post tests withholding unsafe HTML and neutralizing literal Boris syntax.

<!-- boris-migration-review: unsafe iframe HTML withheld; payload in report.json -->

Literal tokens such as &#91;[wiki-link]], &#123;{include notes.md}}, and &lt;Aside kind="note"> must render as plain text.
