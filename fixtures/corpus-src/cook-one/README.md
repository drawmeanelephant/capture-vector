# Cooklang Stunt (Stunts/cook-one)

This stunt verifies Cooklang recipe handling in Boris (`--cooklang` over a `.cook`-only `content/` tree; the plain build captures the mode-selection `ECOOKLANG` contract).

Boris parses Cooklang ingredients (`@sugar{2%tbsp}`), cookware (`#skillet`), and timers (`~{10%minutes}`) into typed nodes in `graph.json`.
