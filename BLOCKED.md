# BLOCKED: WO-8

WO-8 could not start because the required clean-tree precondition failed:

```text
$ git log --oneline -1
2410e8b ui: liteship-auditor fixes - real lifecycle events, empty @theme block, honest hysteresis

$ git branch --show-current
main

$ just verify
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo deny check
typos
error: recipe `typos` failed on line 32 with exit code 2
```

The failing files are tracked `ui/dist/_astro/*.js` assets. WO-8 explicitly
says not to touch `ui/`, and changing `typos.toml` would shrink the gate
outside the ordered scope.

Focused failing gate output:

```text
$ typos --format brief ui/dist
ui/dist/_astro/graph.DjaQLNCU.js:1:466: error: `ue` should be `use`, `due`
ui/dist/_astro/graph.DjaQLNCU.js:1:485: error: `Pn` should be `On`
ui/dist/_astro/graph.DjaQLNCU.js:1:814: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/graph.DjaQLNCU.js:1:911: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/graph.DjaQLNCU.js:1:2500: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/graph.DjaQLNCU.js:1:3256: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/graph.DjaQLNCU.js:1:4700: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/graph.DjaQLNCU.js:1:5175: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/graph.DjaQLNCU.js:1:6034: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/graph.DjaQLNCU.js:2:3139: error: `pn` should be `on`
ui/dist/_astro/worker.BkJetcAO.js:356:1690: error: `ue` should be `use`, `due`
ui/dist/_astro/worker.BkJetcAO.js:356:8504: error: `ue` should be `use`, `due`
ui/dist/_astro/graph.DjaQLNCU.js:2:11942: error: `Pn` should be `On`
ui/dist/_astro/graph.DjaQLNCU.js:2:12470: error: `Pn` should be `On`
ui/dist/_astro/worker.BkJetcAO.js:356:14355: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/graph.DjaQLNCU.js:2:18175: error: `pn` should be `on`
ui/dist/_astro/graph.DjaQLNCU.js:2:20742: error: `ue` should be `use`, `due`
ui/dist/_astro/worker.BkJetcAO.js:649:1274: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/page.DJh9l2y9.js:2:1640: error: `ue` should be `use`, `due`
ui/dist/_astro/page.DJh9l2y9.js:2:1781: error: `ue` should be `use`, `due`
ui/dist/_astro/graph.DjaQLNCU.js:2:21332: error: `pn` should be `on`
ui/dist/_astro/page.DJh9l2y9.js:2:5407: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/page.DJh9l2y9.js:2:5484: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/page.DJh9l2y9.js:2:6772: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/page.DJh9l2y9.js:2:7079: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/stream.DY_oUjcB.js:1:489: error: `ue` should be `use`, `due`
ui/dist/_astro/stream.DY_oUjcB.js:1:500: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/stream.DY_oUjcB.js:1:831: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/stream.DY_oUjcB.js:1:876: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/page.DJh9l2y9.js:4:573: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/stream.DY_oUjcB.js:1:984: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/page.DJh9l2y9.js:4:687: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/stream.DY_oUjcB.js:1:1413: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/dag.DdtR_zDy.js:1:2499: error: `ue` should be `use`, `due`
ui/dist/_astro/stream.DY_oUjcB.js:1:2610: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/stream.DY_oUjcB.js:1:2705: error: `ue` should be `use`, `due`
ui/dist/_astro/stream.DY_oUjcB.js:1:3772: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/page.DJh9l2y9.js:4:5067: error: `pn` should be `on`
ui/dist/_astro/stream.DY_oUjcB.js:1:3812: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/page.DJh9l2y9.js:4:5184: error: `pn` should be `on`
ui/dist/_astro/page.DJh9l2y9.js:4:5722: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/page.DJh9l2y9.js:4:6290: error: `Pn` should be `On`
ui/dist/_astro/page.DJh9l2y9.js:4:6555: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/page.DJh9l2y9.js:4:6878: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/stream.DY_oUjcB.js:1:6083: error: `ue` should be `use`, `due`
ui/dist/_astro/page.DJh9l2y9.js:4:7078: error: `Pn` should be `On`
ui/dist/_astro/stream.DY_oUjcB.js:1:6347: error: `ue` should be `use`, `due`
ui/dist/_astro/stream.DY_oUjcB.js:1:6788: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/stream.DY_oUjcB.js:1:7230: error: `pn` should be `on`
ui/dist/_astro/stream.DY_oUjcB.js:1:7253: error: `pn` should be `on`
ui/dist/_astro/stream.DY_oUjcB.js:1:10145: error: `Pn` should be `On`
ui/dist/_astro/stream.DY_oUjcB.js:1:10689: error: `Pn` should be `On`
ui/dist/_astro/stream.DY_oUjcB.js:1:10847: error: `Pn` should be `On`
ui/dist/_astro/page.DJh9l2y9.js:14:7448: error: `ba` should be `by`, `be`
ui/dist/_astro/page.DJh9l2y9.js:14:7483: error: `ba` should be `by`, `be`
ui/dist/_astro/page.DJh9l2y9.js:14:7517: error: `ba` should be `by`, `be`
ui/dist/_astro/page.DJh9l2y9.js:14:9163: error: `Ba` should be `By`, `Be`
ui/dist/_astro/dag.DdtR_zDy.js:1:8262: error: `ue` should be `use`, `due`
ui/dist/_astro/dag.DdtR_zDy.js:1:9209: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/page.DJh9l2y9.js:14:11590: error: `fo` should be `of`, `for`, `do`, `go`, `to`
ui/dist/_astro/dag.DdtR_zDy.js:1:10831: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/page.DJh9l2y9.js:14:14177: error: `pn` should be `on`
ui/dist/_astro/page.DJh9l2y9.js:14:14200: error: `Fo` should be `Of`, `For`, `Do`, `Go`, `To`
ui/dist/_astro/page.DJh9l2y9.js:14:14257: error: `pn` should be `on`
ui/dist/_astro/dag.DdtR_zDy.js:1:13933: error: `Ue` should be `Use`, `Due`
ui/dist/_astro/page.DJh9l2y9.js:14:14285: error: `Fo` should be `Of`, `For`, `Do`, `Go`, `To`
ui/dist/_astro/page.DJh9l2y9.js:14:14289: error: `Fo` should be `Of`, `For`, `Do`, `Go`, `To`
ui/dist/_astro/dag.DdtR_zDy.js:1:14404: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/page.DJh9l2y9.js:14:14328: error: `pn` should be `on`
ui/dist/_astro/page.DJh9l2y9.js:14:14409: error: `pn` should be `on`
ui/dist/_astro/dag.DdtR_zDy.js:1:17972: error: `ue` should be `use`, `due`
ui/dist/_astro/dag.DdtR_zDy.js:1:18276: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/dag.DdtR_zDy.js:1:18953: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/page.DJh9l2y9.js:14:30953: error: `pn` should be `on`
ui/dist/_astro/page.DJh9l2y9.js:14:31271: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/dag.DdtR_zDy.js:1:28916: error: `pn` should be `on`
ui/dist/_astro/page.DJh9l2y9.js:14:31288: error: `Pn` should be `On`
ui/dist/_astro/dag.DdtR_zDy.js:1:29593: error: `pn` should be `on`
ui/dist/_astro/page.DJh9l2y9.js:14:31291: error: `Ba` should be `By`, `Be`
ui/dist/_astro/page.DJh9l2y9.js:14:31859: error: `Ot` should be `To`, `Of`, `Or`, `Not`, `It`
ui/dist/_astro/page.DJh9l2y9.js:14:32027: error: `ot` should be `to`, `of`, `or`, `not`, `it`
ui/dist/_astro/page.DJh9l2y9.js:14:32044: error: `pn` should be `on`
ui/dist/_astro/page.DJh9l2y9.js:14:32287: error: `fo` should be `of`, `for`, `do`, `go`, `to`
ui/dist/_astro/llm.BLWwFNMP.js:1:14624: error: `ue` should be `use`, `due`
ui/dist/_astro/llm.BLWwFNMP.js:1:17520: error: `ue` should be `use`, `due`
```
