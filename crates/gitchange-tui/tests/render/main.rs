//! Rendering smoke tests over ratatui's TestBackend: the panel stack
//! draws, the All view groups and tags, drilled views dim — asserted on
//! buffer text, not exact cells (snapshot-style, opportunistic per
//! ADR 0008).
//!
//! One test binary, split by concern so a change to one area reads one
//! file. Start from the module whose concern you are changing:
//!
//! | Module      | Concern                                              |
//! |-------------|------------------------------------------------------|
//! | `helpers`   | fixtures, one frame, cell readers — shared by all     |
//! | `frame`     | panel geometry read off a frame — `focus`, `geometry`  |
//! | `panels`    | the panel stack, All and drilled views, the keybar    |
//! | `overlays`  | help, assign popup, text inputs, confirmations, errors |
//! | `log`       | severity glyphs, core's advisories, pin banners       |
//! | `commit`    | the commit flow's overlays (ticket #33)               |
//! | `selection` | full-width selection tints (issue #45)                |
//! | `colours`   | diff origin colours under decoration (issue #46)      |
//! | `focus`     | focus-conditional tints and persistent cursors (#45)  |
//! | `binary`    | binary whole-file hunks (ADR 0009, issue #43)         |
//! | `mode`      | mode hunks beside content hunks (ADR 0017, #104)      |
//! | `geometry`  | the left column's heights and scroll (issue #87)      |

mod frame;
mod helpers;

mod binary;
mod colours;
mod commit;
mod focus;
mod geometry;
mod log;
mod mode;
mod overlays;
mod panels;
mod selection;
