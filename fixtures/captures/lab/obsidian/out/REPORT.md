# Obsidian → Boris migration laboratory

Format: `boris-obsidian-migration-lab` · schema 1 · tool 0.1.1

Source vault: `/CAPTURE_ROOT/src/mini-obsidian`

## Summary

| Metric | Count |
|---|---:|
| Pages | 17 |
| Attachments inventoried | 1 |
| Links resolved | 11 |
| Links unresolved | 2 |
| Links ambiguous | 2 |
| Heading/block refs | 2 |

## Pages

- `A/Shared Path/Deep.md` → `A/Shared-Path/Deep` (exact)
- `Ambiguous/Shared.md` → `Ambiguous/Shared` (exact)
- `B/Shared Path/Deep.md` → `B/Shared-Path/Deep` (exact)
- `Clash/Hello World.md` → `Clash/Hello-World` (exact)
- `Clash/Hello-World.md` → `Clash/Hello-World-2` (exact)
- `Dataview Demo.md` → `Dataview-Demo` (unsupported)
- `Embeds.md` → `Embeds` (human_review)
- `Notes/Alpha.md` → `Notes/Alpha` (human_review)
- `Notes/Beta.md` → `Notes/Beta` (exact)
- `Notes/Sub/Gamma.md` → `Notes/Sub/Gamma` (exact)
- `Notes/Suffix Probe.md` → `Notes/Suffix-Probe` (human_review)
- `Other/Shared.md` → `Other/Shared` (exact)
- `Projects/Q1 Plan.md` → `Projects/Q1-Plan` (transformed)
- `Templates/Nav.md` → `Templates/Nav` (human_review)
- `Vault/Concept Board/Concept Board.md` → `Vault/Concept-Board/Concept-Board` (exact)
- `Vault/Concept Board/Concepts/Vertical Hydroponics.md` → `Vault/Concept-Board/Concepts/Vertical-Hydroponics` (transformed)
- `Welcome.md` → `Welcome` (transformed)

## Links

- [resolved] `Embeds.md`: `![[Attachments/diagram.png]]` → `assets/Attachments/diagram.png`
- [unsupported_embed] `Embeds.md`: `![[Attachments/diagram.png|400]]`
- [resolved] `Embeds.md`: `![[Beta]]` → `Notes/Beta`
- [unresolved] `Embeds.md`: `![[no-such.png]]`
- [heading_or_block] `Notes/Alpha.md`: `[[Beta#Details]]`
- [heading_or_block] `Notes/Alpha.md`: `[[Beta#^block1]]`
- [resolved] `Notes/Alpha.md`: `[[Beta]]` → `Notes/Beta`
- [resolved] `Notes/Alpha.md`: `[[Beta|the beta note]]` → `Notes/Beta`
- [unresolved] `Notes/Alpha.md`: `[[Missing Note]]`
- [resolved] `Notes/Alpha.md`: `[[Notes/Sub/Gamma]]` → `Notes/Sub/Gamma`
- [resolved] `Notes/Alpha.md`: `[[Q1 Plan]]` → `Projects/Q1-Plan`
- [ambiguous] `Notes/Alpha.md`: `[[Shared]]`
- [ambiguous] `Notes/Suffix Probe.md`: `[[Shared Path/Deep]]`
- [resolved] `Projects/Q1 Plan.md`: `[[Welcome]]` → `Welcome`
- [plugin_template] `Templates/Nav.md`: `[[${navLink}|${navName}]]`
- [plugin_template] `Templates/Nav.md`: `[[<% tp.file.title %>]]`
- [resolved] `Vault/Concept Board/Concepts/Vertical Hydroponics.md`: `[[Concept Board/Concept Board|Concept Board]]` → `Vault/Concept-Board/Concept-Board`
- [resolved] `Vault/Concept Board/Concepts/Vertical Hydroponics.md`: `[[Concept-Board/Concept-Board]]` → `Vault/Concept-Board/Concept-Board`
- [resolved] `Welcome.md`: `[[Beta]]` → `Notes/Beta`
- [resolved] `Welcome.md`: `[[Notes/Alpha]]` → `Notes/Alpha`

## Hazards (Dataview / Canvas / plugins / review)

- **canvas** in `Canvas Board.canvas`: Obsidian Canvas is not converted in phase-1; inventoried only
- **dataview** in `Dataview Demo.md`: dataview fenced block (not evaluated)
- **dataviewjs** in `Dataview Demo.md`: dataviewjs fenced block (not evaluated)
- **plugin_syntax** in `Dataview Demo.md`: inline $= query syntax retained raw
- **plugin_syntax** in `Dataview Demo.md`: possible inline field (key:: value) retained raw
- **tasks_plugin** in `Dataview Demo.md`: tasks plugin fenced block (not evaluated)
- **note_embed** in `Embeds.md`: ![[Beta]] → [[Notes/Beta]]
- **unresolved_link** in `Embeds.md`: ![[no-such.png]]
- **unsupported_embed** in `Embeds.md`: ![[Attachments/diagram.png|400]]
- **ambiguous_link** in `Notes/Alpha.md`: [[Shared]]
- **heading_or_block_ref** in `Notes/Alpha.md`: [[Beta#Details]]
- **heading_or_block_ref** in `Notes/Alpha.md`: [[Beta#^block1]]
- **unknown_frontmatter_key** in `Notes/Alpha.md`: key `cssclass` not in Boris closed grammar
- **unresolved_link** in `Notes/Alpha.md`: [[Missing Note]]
- **ambiguous_link** in `Notes/Suffix Probe.md`: [[Shared Path/Deep]]
- **plugin_syntax** in `Templates/Nav.md`: [[${navLink}|${navName}]]
- **plugin_syntax** in `Templates/Nav.md`: [[<% tp.file.title %>]]

## Unsupported items

- `Canvas Board.canvas` (canvas): Obsidian Canvas is not converted in phase-1; inventoried only
- `Clash/Hello-World.md` (entity_id_collision): collides with Clash/Hello World.md → Clash/Hello-World; remapped to Clash/Hello-World-2

## Human review queue

- `Canvas Board.canvas` — canvas: Obsidian Canvas is not converted in phase-1; inventoried only
- `Dataview Demo.md` — dataview: dataview fenced block (not evaluated)
- `Dataview Demo.md` — dataviewjs: dataviewjs fenced block (not evaluated)
- `Dataview Demo.md` — plugin_syntax: inline $= query syntax retained raw
- `Dataview Demo.md` — plugin_syntax: possible inline field (key:: value) retained raw
- `Dataview Demo.md` — tasks_plugin: tasks plugin fenced block (not evaluated)
- `Embeds.md` — unresolved: ![[no-such.png]]
- `Embeds.md` — unsupported_embed: ![[Attachments/diagram.png|400]]
- `Notes/Alpha.md` — ambiguous: [[Shared]]
- `Notes/Alpha.md` — heading_or_block: [[Beta#Details]]
- `Notes/Alpha.md` — heading_or_block: [[Beta#^block1]]
- `Notes/Alpha.md` — unknown_frontmatter_key: cssclass
- `Notes/Alpha.md` — unresolved: [[Missing Note]]
- `Notes/Suffix Probe.md` — ambiguous: [[Shared Path/Deep]]
- `Templates/Nav.md` — plugin_template: [[${navLink}|${navName}]]
- `Templates/Nav.md` — plugin_template: [[<% tp.file.title %>]]

## Attachments

- `Attachments/diagram.png` → `assets/Attachments/diagram.png` (copied) (referenced)

---

Phase-1 boundaries: no Dataview/Canvas/plugin evaluation; heading/block refs left raw; ambiguous/unresolved links left raw; vault never modified.
