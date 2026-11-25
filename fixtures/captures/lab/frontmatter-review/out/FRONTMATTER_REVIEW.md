# Unsupported frontmatter preservation report

> **Migration-lab only.** Boris core grammar is unchanged.
> Source files are never modified.

**Source root:** `/CAPTURE_ROOT/src/fm-review-mixed/content`  
**Tool version:** 0.1.0  
**Total unsupported key occurrences:** 5  
**Files with unsupported keys:** 2

---

## Field dispositions

| Key | Disposition | Occurrences | Affected files |
|---|---|---:|---:|
| `caseNumber` | `identity_source` | 1 | 1 |
| `description` | `body_candidate` | 1 | 1 |
| `mascotId` | `identity_source` | 1 | 1 |
| `slug` | `identity_source` | 1 | 1 |
| `updatedAt` | `sidecar_only` | 1 | 1 |

---

## Per-file classification

### `mascots/cass.md`

| Line | Key | Raw value |
|-----:|-----|-----------|
| 3 | `mascotId` | `cass-d` |
| 4 | `description` | `The primary mascot for failure states.` |

### `posts/case-alpha.md`

| Line | Key | Raw value |
|-----:|-----|-----------|
| 3 | `caseNumber` | `CASE-ALPHA-001` |
| 4 | `slug` | `posts/case-alpha` |
| 6 | `updatedAt` | `2026-07-01` |

---

_Machine-readable twin: `frontmatter_review.json`._
