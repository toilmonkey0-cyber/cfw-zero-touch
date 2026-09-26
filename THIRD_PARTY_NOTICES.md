# Third-Party Notices

## Needle (cactus-compute)

The optional local-AI tier uses artifacts of **Needle 3** by Cactus Compute,
Inc. — https://github.com/cactus-compute/needle

- **Engine code and C header** (`needle.exe`, `libneedle.a`, `needle.h`):
  licensed under the Apache License, Version 2.0 —
  https://www.apache.org/licenses/LICENSE-2.0.
  Copyright © 2026 Cactus Compute, Inc. and Needle contributors
  (Ndubuaku, Henry; Mosoyan, Karen; Mroz, Jakub; Cylich, Noah; Kumar,
  Satyajit; Sandhu, Parkirat; Shemet, Roman; Lee, Justin H.).
- **Model weights** (`needle3.cact`, served from
  https://huggingface.co/Cactus-Compute/needle3): the code repository's
  Apache-2.0 LICENSE covers the repository; the specific license tag for the
  weights file is pending confirmation (design Open Question 9) and this
  notice will state it accurately once confirmed.

Card Studio acquires these artifacts only with explicit user consent and
verifies every download against SHA-256 pins compiled into the app
(`src-tauri/src/needle/manifest.rs`, pins taken from Hugging Face revision
`b274efcb211a9eef48c9a88da4b43bd569696a39`). The engine archive and header
are build-time inputs for the `cfw-embed` helper; the weights and the
serve engine are runtime downloads under `%LOCALAPPDATA%\cfw-card-studio\needle`.
Telemetry is disabled on everything the app spawns
(`NEEDLE_TELEMETRY=0`, `DO_NOT_TRACK=1`), and inference itself never
touches the network.
