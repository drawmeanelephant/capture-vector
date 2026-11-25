---
id: index
title: Home
status: published
---

Welcome to the synthetic mini-Jekyll site.

{% include note.html text="Included from _includes." %}

Read the [[posts/welcome-post|welcome post]] and the
[[docs/guide-one|first guide]].

This `post_url` is intentionally broken: {% post_url 1999-01-01-ghost-post %}.

Hero image: index.assets/images/hero.png

{% raw %}{{ not liquid }}{% endraw %}

A Liquid tag inside a fenced code block is not a live tag:

```liquid
{% post_url 2024-01-02-welcome-post %}
```
