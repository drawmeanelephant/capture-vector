# Asset filename compatibility report

Migration laboratory only. Boris core still requires ASCII content-local
asset path segments (`[A-Za-z0-9._-]+`). This lab rewrites unsafe source
names into that grammar without changing the product contract.

| metric | count |
|---|---:|
| assets | 12 |
| unchanged | 3 |
| rewritten | 6 |
| rejected | 3 |
| markdown rewrites | 7 |

## Assets

- `case-collision.assets/Foo Bar.png` → `case-collision.assets/Foo-Bar.png` (rewritten, spaces) sha256=`68e173b7fce619b2374d8e52bf32c4b40a3ab602d0c59e79422a5fce21db0254`
- `case-collision.assets/foo-bar.png` → `case-collision.assets/foo-bar.png` (rejected, case_collision) sha256=`5962e8b60aa02e6c3c55262473c8922acba89810cab7206abda42d7a03f02589`
- `nested.assets/deep/sub dir/shot.png` → `nested.assets/deep/sub-dir/shot.png` (rewritten, spaces) sha256=`912b3a5655fdc695ccac46b72f2e39430dea6eecc34c5983b45d21b288bb8fb8`
- `percent.assets/diagram%20copy.png` → `percent.assets/diagram-copy.png` (rewritten, percent_encoding) sha256=`bed9a394300042914449a197a7f6f0783a9d6fc142932401cc72d6c6f37bc1be`
- `safe.assets/already-ok.png` → `safe.assets/already-ok.png` (unchanged, already_safe) sha256=`001445fda74cd03b527bb6e3235a07817ab5b09d987f42c60b86182433f37c2b`
- `sanitize-collision.assets/foo bar.png` → `sanitize-collision.assets/foo-bar.png` (rewritten, spaces) sha256=`5e46f9e879d817b51a31ca0b779308c4d701bdac6566c92d2b5a063d92239d4c`
- `sanitize-collision.assets/foo-bar.png` → `sanitize-collision.assets/foo-bar.png` (rejected, destination_collision) sha256=`2d90166f8c23bd7116185bcd367173559ffc0b708f3fe14303b82a5090b8d68a`
- `spaces.assets/hello world.png` → `spaces.assets/hello-world.png` (rewritten, spaces) sha256=`8b1fd450298cfbad296aab184e35a063c035f341d5b19da1b31c6a9acdba39ec`
- `symlink.assets/alias.png` → `(rejected)` (rejected, symlink)
- `symlink.assets/real.png` → `symlink.assets/real.png` (unchanged, already_safe) sha256=`a96efdac0588a6b7c00cf03760e944770431441be72e1875e514fa5423d402a2`
- `traversal.assets/nested/ok.png` → `traversal.assets/nested/ok.png` (unchanged, already_safe) sha256=`0bf9a4877351d0afcd587e70632cd4bc97c2358ed53fb0edcd50aff56ec8068d`
- `unicode.assets/café.png` → `unicode.assets/caf.png` (rewritten, unicode) sha256=`76b60d3a19f7a99462c76db992c75d3043dcbf2f3e0191297d952b6d9f965865`

## Markdown rewrites

- `case-collision.md`: `case-collision.assets/Foo Bar.png` → `case-collision.assets/Foo-Bar.png`
- `nested.md`: `nested.assets/deep/sub dir/shot.png` → `nested.assets/deep/sub-dir/shot.png`
- `percent.md`: `percent.assets/diagram%20copy.png` → `percent.assets/diagram-copy.png`
- `sanitize-collision.md`: `sanitize-collision.assets/foo bar.png` → `sanitize-collision.assets/foo-bar.png`
- `spaces.md`: `spaces.assets/hello world.png` → `spaces.assets/hello-world.png`
- `spaces.md`: `spaces.assets/hello%20world.png` → `spaces.assets/hello-world.png`
- `unicode.md`: `unicode.assets/café.png` → `unicode.assets/caf.png`

Machine-readable twins: `asset_filename_manifest.json`, `rewrite_manifest.json`, `report.json`.
