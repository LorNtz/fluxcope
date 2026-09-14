# Project Description

Fluxcope is a terminal UI MITM proxy built with ratatui and hudsucker. The long-term goal is to provide a Charles-like local debugging proxy experience in the terminal: capture HTTP/HTTPS traffic, inspect requests and responses, rewrite requests, map responses to local files, and keep the configuration editable and stable across launches.

The app currently runs a local proxy on the configured port, forwards HTTP and HTTPS traffic, and records captured request/response pairs while recording is enabled. The main UI has a status bar at the top and a normal workspace below it:
1. Left request tree:
    - Displays captured requests in capture order, grouped by origin and URL path.
    - Uses the effective mapped remote URL as the displayed request entry when map-remote rewrites a request.
2. Right detail panel:
    - Displays request headers, request body, response headers, and response body.
    - Uses border-rendered Ratatui tabs for switching the active detail tab.
    - Request/response body tabs can be entered as read-only editor views for search, selection/copy, and quick in-view jumps.
3. Full-workspace log panel:
    - Hidden by default.
    - Toggled over the normal request/detail workspace without covering the top status bar.

Persistent settings live in `~/.fluxcope/config.yml`. The `proxy` YAML section is optional, supports named presets, `map_remote`, and `map_local`, and is intentionally structured for future in-app editing. Serialization should not add proxy boilerplate when no proxy settings are present, and should preserve explicit but semantically default config entries and existing mapping order when possible.

# Current Status

- Proxy startup uses the configured `server.port` and rejects startup when the port is already occupied.
- CA generation/loading uses configurable certificate store settings, and the app can expose the CA PEM through a temporary LAN download server with a QR-code popup opened by `c`.
- Recording can start enabled or disabled from config, can be toggled with `r`, and recording-off traffic still forwards and applies mapping without storing captures.
- Request-response matching is tracked per proxy handler instance, so concurrent in-flight requests are captured independently.
- Captured requests are ordered by request capture sequence rather than response completion order.
- Request and response body capture reconstructs the original forwarded bytes after inspection.
- Display-side content decoding supports gzip, deflate, brotli, and zstd while preserving forwarded bytes.
- Request body display formats `application/x-www-form-urlencoded` bodies and decodes percent-encoded UTF-8 text correctly.
- Response body display pretty-prints JSON for normal responses, but map-local responses display the mapped file text exactly as captured.
- Request tree supports keyboard and mouse navigation, folded-by-default behavior, optional auto expansion through `ui.request_list.auto_expand`, and a scrollbar.
- Request tree supports `d` to delete a selected request leaf/subtree and `D` to clear all captured requests and release request memory.
- Post-delete request tree selection moves to the closest useful visible neighbor, including folded branch edge cases.
- Focus handling supports `Tab`/`Shift+Tab`, directional `Ctrl+h/j/k/l`, mouse focus, panel-specific scroll keys, and mouse wheel scrolling.
- Detail panel tabs are rendered on the detail panel border with Ratatui `Tabs`; mouse clicks on those border tabs switch detail tabs.
- Request/response body tabs support an enterable read-only `edtui` editor view entered with `Enter` and exited with `Esc`.
- The body editor supports Vim-like navigation, `/` search, visual selection and copy, flash-style visible-text jumping with adaptive labels, and a scoped subset of normal-mode `y` copy actions for common motions plus inner-word/delimiter text objects.
- Log panel is hidden by default and toggles as a full-workspace panel below the status bar.
- Persistent settings manager reads and writes YAML at `~/.fluxcope/config.yml`.
- Supported config currently includes `server.port`, `certificate.store_dir`, `certificate.pem_filename`, `recording.start_record_on_launch`, `ui.request_list.auto_expand`, and optional `proxy` mapping settings.
- Optional proxy mapping supports enable flags, active presets, map-remote rules, map-local rules, disabled rules, host-level rules, full-path rules, and map-local after map-remote.
- Config serialization preserves present default-valued entries generically when removing them would be a semantic no-op, and keeps existing YAML mapping order stable where possible.

# Rules to Follow When Editing

- when drafting plans or proposals, grill me if there's anything unclear or you need my decision.
- When making changes, do your best to not touch and improve adjacent but irrelevant code, comments, or formatting. Don't refactor what isn't broken. I don't want my feature commits include irrelevant code changes.
- When fixing issues, unless I ask you to do explicitly, do not add speculative features to handle them.
- This is aim to be a serious production-ready project and do not treat it as a demo when implementing features. Every delivery should be a complete feature and don't implement things halfway.
- For newly added code, do some proper abstractions like traits or generics to provide extensibility, especially at the interface level but avoid over-engineering (like adding a helper function only to take arguments and create a struct with them but without doing anything else). It is encouraged to create helper functions to improve maintainability and readability but avoid splitting a continuous logic into multiple functions, especially those that are unlikely to be reused and require lengthy naming to explain their purpose.
- Avoid making a file or module overly lengthy. If the newly added code causes this, it is encouraged to perform some idiomatic and appropriate module/file splitting (at this point, adjustments and refactoring of the existing code architecture without breaking are allowed). However, it is not encouraged to rigidly split some highly cohesive code.
- Do track @agent_journal.md in git history. When you switch branches just bring it across branches.
- After code implementation (remember that plan or doc changes are not included), use subagents to review the changes. Spawn two subagents with their jobs described below. Notice that the main agent will do all the programmatic checkings (cargo test, cargo fmt, clippy and so on) so subagents should not do these again. Wait for all of them, then make appropriate modifications to the change based on the review opinions.
    - One subagent for design pattern and maintainability review, with (and not limited to) these responsibilities:
        - discovering repeating patterns that can be elegantly abstracted and properly encapsulated, and offer design advices as a senior professional engineer
        - hunting for common anti-patterns in rust
        - evaluate and find code structures that might be hard to read and maintain for unexperienced human developers
        - identify code smells in the context of changed code
        - spotting code that over-encapsulate and over-design
        - putting forward suggestions that can make the data structure architecture & code structure more organized and elegant.
        - should also pay attention to the stale code that was probably forgotten to be deleted during the change of implementation approach by main agent.
    - One for performance checks to identify if there's any room of improvement to time consumption or memory usage. It's work would include and not limited to: hunting for common optimization points in rust like unnecessary memory cloning, looking for room of optimization on data structure & algorithm efficiency, and identify slow operations on hot paths.
- After each code change you make, append a brief record of what you just did to the end of @agent_journal.md to allow me to know your work steps. Every record should mention the task objective and the time the change is made. Do not read it for context lookup, since it may contain info about iteration that has already been discarded. You may follow this template:

```markdown
### Task 1: Some task done before
Date: yyyy/mm/dd hh:mm

- Modified somewhere
- Modified elsewhere
- Design Review
    - some design issues addressed by subagent
- Performance Review
    - some performance issues addressed by subagent
- Changes after adopting the review opinions

**Result**: some feature implemented/ bug fixed, verified with some tests in xxx.rs
```
