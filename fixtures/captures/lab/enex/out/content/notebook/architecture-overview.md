---
id: notebook/architecture-overview
title: Architecture Overview
parent: notebook
status: draft
tags: ["architecture", "docs"]
---

# Architecture Overview

This synthetic note covers **bold**, *italic*, **strong**, and *emphasized* inline markup plus `inline_code()`.

A div paragraph linking to [Example Spec](https://example.com/spec).

## Key Components

- Notebook XML container
- ENML content body

1. Parse export metadata
2. Match resources by decoded MD5

### Checklist and Snippets

- [x] Verify MD5 media binding
- [ ] Review encrypted blocks

```
fn main() { return 0; }
```

> Quoted design note for ENEX conversion.

| Construct | Handling |
| --- | --- |
| ENML table | Markdown table |

![](architecture-overview.assets/diagram.png)

[notes.txt](architecture-overview.assets/notes.txt)
