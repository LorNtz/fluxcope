# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/LorNtz/fluxcope/releases/tag/v0.1.0) - 2026-09-15

### Added

- prepare Fluxcope for its first release
- support searching in request tree
- suppressed checkbox color in tables when toggles at higher priority are off
- support URL glob matching based request prefilter
- setting popup
- render request body the same way as response body
- press enter on selected request brings focus to the detail panel
- simulated vim-like yank action in edtui editor
- introduced edtui as the read-only editor widget for request/response body view
- added some useful keybinds to expand/collapse nodes in request panel
- added a count badge behind each subtree node in request panel
- changed the display content of header panels to tables
- add support for switching recording on/off and a status bar for indication
- map remote & map local
- focusable panel and independent keybinding for each panel
- add keybinds to delete a request or clear all from the request panel
- render request list in tree view
- support to display compressed response body
- config file and basic setting mechanism
- add a popup to render a QR code allowing user to download the CA by scanning it
- add a check for port availability and quit the app if not available
- log info of listened address
- pretty print response body
- add scrolling support for main display & log panel
- support formatted display for request body with content type set to x-www-form-urlencoded
- content display for response body
- add support to hide log panel
- requests panel selection indicator
- content display for request header tab
- persistent ca
- ui feature

### Changed

- remove right panel wrapper
- make UI cache mutation explicit
- unify header table presentation
- decompose settings UI
- extract settings UI module
- split settings popup state
- split detail body responsibilities
- consolidate detail application state
- extract detail UI modules
- organize unit test suites
- better scroll offset preservation in body panel
- bound streaming capture and render state
- add bounded ordered capture store
- supervise runtime and bound logging
- establish neutral capture boundary
- added a slash suffix to path subtree nodes
- adjusted the display layout of the log panel in the app
- inline the tab area into the upper border of the details panel
- change the behavior of moving focus through mouse event
- split app.rs into multiple file modules
- move widget geometry ownership from App struct to panel structs
- separate the logic out from the main function
- refactor the app state with hierarchical design
- init

### Fixed

- unify terminal cell fitting
- retain rendered settings hit regions
- fixed an issue where mouse scroll goes beyond limit in request list
- avoid body loading flash
- broken buffer when displaying mapped local file containing tabs
- bind the server listener to 0.0.0.0
- mojibake in displayed request body
- displayed response will not reformatted when it's from a mapped local file
- fixed the issue where entries with default values will be reordered to the end of its scope in config file
- fixed the issue where entries with default values in the config file will lost after app launch
- tree nodes in request panel are now always displayed at the top in their respective levels
- fixed the problem of request being mistakenly dropped
