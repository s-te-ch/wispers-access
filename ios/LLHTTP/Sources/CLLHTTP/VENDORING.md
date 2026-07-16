# Vendored: llhttp

This directory contains a third-party source, copied verbatim.

- **Project:** [llhttp](https://github.com/nodejs/llhttp) — the HTTP/1.x parser used by
  Node.js and vendored inside SwiftNIO.
- **Version:** v9.4.2 (upstream tag `release/v9.4.2`)
- **License:** MIT — see [`LICENSE-MIT`](LICENSE-MIT).

## Files

Copied unmodified from the release tag:

| here | upstream path |
|---|---|
| `include/llhttp.h` | `include/llhttp.h` |
| `api.c` | `src/api.c` |
| `http.c` | `src/http.c` |
| `llhttp.c` | `src/llhttp.c` |

`include/module.modulemap` is ours (exposes `llhttp.h` as the `CLLHTTP` module to SwiftPM).

## Re-vendoring (to bump the version)

```sh
TAG=release/v9.4.2   # set to the desired release tag
BASE="https://raw.githubusercontent.com/nodejs/llhttp/$TAG"
curl -fsSL "$BASE/include/llhttp.h"  -o include/llhttp.h
curl -fsSL "$BASE/src/api.c"         -o api.c
curl -fsSL "$BASE/src/http.c"        -o http.c
curl -fsSL "$BASE/src/llhttp.c"      -o llhttp.c
curl -fsSL "$BASE/LICENSE-MIT"       -o LICENSE-MIT
```

Then run the package tests (`swift test` in `ios/LLHTTP`) and update the version above.
