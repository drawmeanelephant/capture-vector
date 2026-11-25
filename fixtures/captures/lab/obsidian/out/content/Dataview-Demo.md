---
title: "Dataview Demo"
---
<!-- boris-migration-provenance
  format: boris-obsidian-migration-lab
  source_path: Dataview Demo.md
  entity_id: Dataview-Demo
  tool_version: 0.1.1
-->
# Dataview Demo

Inline field:: value

```dataview
TABLE file.name FROM "Notes"
```

```dataviewjs
dv.list(dv.pages().file.name)
```

Also `$= dv.current().file.name` and a tasks query line:

```tasks
not done
```
