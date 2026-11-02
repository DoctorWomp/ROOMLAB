# Licenses

RoomLab source code in this repository: MIT License.

```
MIT License — Copyright (c) 2026 RoomLab contributors
Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions: The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.
THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED.
```

Third-party components (Rust crates, React, Tauri, SQLite via libsqlite3-sys) keep their own licenses;
`tools/package_portable.ps1` generates `THIRD_PARTY_NOTICES.txt` from `Cargo.lock` and `node_modules`.
No model weights, llama.cpp binaries or WebView2 runtime are redistributed. Users are responsible for
the licenses of models and binaries they connect. The SOMA/RANGE/KIE concepts referenced in the
blueprint are the blueprint author's; no commercial rights are assumed.
