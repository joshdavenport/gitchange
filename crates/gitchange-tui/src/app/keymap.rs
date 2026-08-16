//! The binding core (ADR 0014): one record per panel-level action —
//! identity, key spelling(s), capability link, help label. Dispatch
//! resolves keys against these records and both display surfaces render
//! spellings from them, so a hint can never name a key dispatch doesn't
//! match. Modal and overlay keys stay with their modals; `ctrl+c` stays
//! a protocol-level quit above the keymap.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Panel;

/// A binding's identity: what dispatch matches on once the key resolves,
/// and what the keybar arms mention editorially.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingId {
    FocusPanel,
    MoveDown,
    MoveUp,
    PageDown,
    PageUp,
    JumpBottom,
    JumpTop,
    DrillIn,
    AssignSelected,
    AssignUnassigned,
    AssignAll,
    StageToggle,
    Commit,
    NewChangelist,
    DeleteChangelist,
    RenameChangelist,
    SwitchActive,
    Back,
    Refresh,
    Help,
    Quit,
}

/// The context capability a binding needs — the one link dispatch and
/// the keybar both consult, through `App::disabled_reason`. Context
/// only: what the selection affords. Content no-ops (hunkless `enter`,
/// an empty assign payload, a file-less `space`) stay press-time checks
/// inside the handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Capability {
    Always,
    /// `d`/`r`: a changelist row is scoped, from a panel that shows it.
    /// Unassigned is built in — there is nothing to rename or delete.
    ChangelistOps,
    /// `s`: a changelist row *or* the unassigned row is scoped, from a
    /// panel that shows it. Wider than [`Capability::ChangelistOps`]
    /// because unassigned is a switchable target (ADR 0015): switching
    /// there is capture-off.
    SwitchActive,
    /// The assign trio: Files or Diff focus, or a blurred hunk selection
    /// surviving cross-panel (issue #45).
    Assign,
    /// `space`: on the Changelists panel, a row that is a staging target
    /// — not the All view. Elsewhere `space` scopes itself by the
    /// panel's own selection and needs no context guard.
    Stage,
    /// `c`: no git operation in progress, and not the All view.
    Commit,
}

/// One key a binding answers to. `Char` never matches with CONTROL held,
/// so a `Ctrl` binding on the same letter can't collide; `Ctrl` matches
/// on CONTROL alone and case-agnostically (ADR 0013 — the control byte a
/// plain terminal sends carries no shift bit, so consulting SHIFT would
/// make `ctrl+shift+<letter>` fire a different action than asked for).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Key {
    Char(char),
    Ctrl(char),
    Enter,
    Esc,
    Down,
    Up,
    PageDown,
    PageUp,
    /// Any digit in the panel numbering. Keys and spellings derive from
    /// [`Panel::ALL`], so the bar's span, the help row and the borders'
    /// `[n]` prefixes cannot disagree.
    PanelDigit,
}

/// Which display surface a spelling is for: the bar renders compactly,
/// the help overlay explains.
#[derive(Clone, Copy)]
enum Register {
    Bar,
    Help,
}

impl Key {
    fn matches(self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self {
            Key::Char(c) => !ctrl && key.code == KeyCode::Char(c),
            Key::Ctrl(c) => {
                ctrl && matches!(key.code, KeyCode::Char(k) if k.eq_ignore_ascii_case(&c))
            }
            Key::Enter => key.code == KeyCode::Enter,
            Key::Esc => key.code == KeyCode::Esc,
            Key::Down => key.code == KeyCode::Down,
            Key::Up => key.code == KeyCode::Up,
            Key::PageDown => key.code == KeyCode::PageDown,
            Key::PageUp => key.code == KeyCode::PageUp,
            Key::PanelDigit => {
                !ctrl && matches!(key.code, KeyCode::Char(c) if Panel::from_number(c).is_some())
            }
        }
    }

    fn spelling(self, register: Register) -> String {
        match self {
            Key::Char(' ') => "space".into(),
            Key::Char(c) => c.to_string(),
            Key::Ctrl(c) => format!("ctrl+{c}"),
            Key::Enter => "enter".into(),
            Key::Esc => "esc".into(),
            Key::Down => "↓".into(),
            Key::Up => "↑".into(),
            Key::PageDown => "PgDn".into(),
            Key::PageUp => "PgUp".into(),
            Key::PanelDigit => match register {
                Register::Bar => panels_bar_spelling(),
                Register::Help => panels_help_spelling(),
            },
        }
    }
}

/// The help register's per-binding label. `Arrowed` clauses join with
/// the theme's drill arrow at render time — what lets the overlay's two
/// arrows theme (ADR 0014's last token-literal carve-out).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HelpLabel {
    Plain(&'static str),
    Arrowed {
        prefix: &'static str,
        parts: &'static [&'static str],
        suffix: &'static str,
    },
    /// `focus panel (0 = diff)`, the digit taken from the numbering.
    PanelFocus,
}

impl HelpLabel {
    fn render(&self, arrow: char) -> String {
        match self {
            HelpLabel::Plain(text) => (*text).into(),
            HelpLabel::Arrowed {
                prefix,
                parts,
                suffix,
            } => format!("{prefix}{}{suffix}", parts.join(&format!(" {arrow} "))),
            HelpLabel::PanelFocus => format!("focus panel ({} = diff)", Panel::Diff.number()),
        }
    }
}

/// One binding record: the single place an action's keys are spelled.
pub(super) struct Binding {
    pub id: BindingId,
    /// Match values, primary spelling first — the bar shows the primary,
    /// the help overlay shows all.
    pub keys: &'static [Key],
    pub capability: Capability,
    pub help: HelpLabel,
}

/// The binding core, one record per panel-level action; record order is
/// the help overlay's order. Consecutive records sharing a help label
/// merge into one help row (`j`/`k` and their arrows).
pub(super) const BINDINGS: &[Binding] = &[
    Binding {
        id: BindingId::FocusPanel,
        keys: &[Key::PanelDigit],
        capability: Capability::Always,
        help: HelpLabel::PanelFocus,
    },
    Binding {
        id: BindingId::MoveDown,
        keys: &[Key::Char('j'), Key::Down],
        capability: Capability::Always,
        help: HelpLabel::Plain("move within panel / hunks / scroll diff"),
    },
    Binding {
        id: BindingId::MoveUp,
        keys: &[Key::Char('k'), Key::Up],
        capability: Capability::Always,
        help: HelpLabel::Plain("move within panel / hunks / scroll diff"),
    },
    // The page keys (issue #84), lazygit's spellings, with the keypad's
    // own page keys as equal second spellings. Their shared help label
    // merges them into one overlay row, as `j`/`k` and the arrows merge.
    Binding {
        id: BindingId::PageDown,
        keys: &[Key::Char('.'), Key::PageDown],
        capability: Capability::Always,
        help: HelpLabel::Plain("page within panel / hunks / scroll diff"),
    },
    Binding {
        id: BindingId::PageUp,
        keys: &[Key::Char(','), Key::PageUp],
        capability: Capability::Always,
        help: HelpLabel::Plain("page within panel / hunks / scroll diff"),
    },
    // The jump keys (issue #88), lazygit's spellings, paired with the
    // page keys above as lazygit pairs them. Each arrives as its own
    // character byte whether or not the terminal reports the shift that
    // typed it, so ADR 0013 permits them and dispatch inspects no
    // modifier. One shared help label, one overlay row.
    Binding {
        id: BindingId::JumpBottom,
        keys: &[Key::Char('>')],
        capability: Capability::Always,
        help: HelpLabel::Plain("jump to either end of panel / hunks / diff"),
    },
    Binding {
        id: BindingId::JumpTop,
        keys: &[Key::Char('<')],
        capability: Capability::Always,
        help: HelpLabel::Plain("jump to either end of panel / hunks / diff"),
    },
    Binding {
        id: BindingId::DrillIn,
        keys: &[Key::Enter],
        capability: Capability::Always,
        help: HelpLabel::Arrowed {
            prefix: "drill in: ",
            parts: &["changelist", "files", "hunk mode"],
            suffix: "",
        },
    },
    Binding {
        id: BindingId::AssignSelected,
        keys: &[Key::Char('a')],
        capability: Capability::Assign,
        help: HelpLabel::Plain("assign hunk (files: the row's group)"),
    },
    Binding {
        id: BindingId::AssignUnassigned,
        keys: &[Key::Char('A')],
        capability: Capability::Assign,
        help: HelpLabel::Plain("assign the file's unassigned hunks"),
    },
    Binding {
        id: BindingId::AssignAll,
        keys: &[Key::Ctrl('a')],
        capability: Capability::Assign,
        help: HelpLabel::Plain("assign every hunk of the file"),
    },
    Binding {
        id: BindingId::StageToggle,
        keys: &[Key::Char(' ')],
        capability: Capability::Stage,
        help: HelpLabel::Plain("toggle stage changelist/file row/hunk"),
    },
    Binding {
        id: BindingId::Commit,
        keys: &[Key::Char('c')],
        capability: Capability::Commit,
        help: HelpLabel::Plain("commit changelist"),
    },
    Binding {
        id: BindingId::NewChangelist,
        keys: &[Key::Char('n')],
        capability: Capability::Always,
        help: HelpLabel::Plain("new changelist (and switch to it)"),
    },
    Binding {
        id: BindingId::DeleteChangelist,
        keys: &[Key::Char('d')],
        capability: Capability::ChangelistOps,
        help: HelpLabel::Plain("delete changelist"),
    },
    Binding {
        id: BindingId::RenameChangelist,
        keys: &[Key::Char('r')],
        capability: Capability::ChangelistOps,
        help: HelpLabel::Plain("rename changelist"),
    },
    Binding {
        id: BindingId::SwitchActive,
        keys: &[Key::Char('s')],
        capability: Capability::SwitchActive,
        help: HelpLabel::Plain("switch active changelist (or unassigned)"),
    },
    Binding {
        id: BindingId::Back,
        keys: &[Key::Esc],
        capability: Capability::Always,
        help: HelpLabel::Arrowed {
            prefix: "back (",
            parts: &["diff", "files", "changelists", "all"],
            suffix: ")",
        },
    },
    Binding {
        id: BindingId::Refresh,
        keys: &[Key::Char('R')],
        capability: Capability::Always,
        help: HelpLabel::Plain("refresh now"),
    },
    Binding {
        id: BindingId::Help,
        keys: &[Key::Char('?')],
        capability: Capability::Always,
        help: HelpLabel::Plain("toggle keybindings"),
    },
    Binding {
        id: BindingId::Quit,
        keys: &[Key::Char('q')],
        capability: Capability::Always,
        help: HelpLabel::Plain("quit"),
    },
];

/// The record a key event resolves to, if any. Unbound keys are nobody's
/// business — dispatch drops them.
pub(super) fn binding_for(key: KeyEvent) -> Option<&'static Binding> {
    BINDINGS
        .iter()
        .find(|binding| binding.keys.iter().any(|k| k.matches(key)))
}

/// The record behind an identity — total, since every [`BindingId`] has
/// exactly one record in [`BINDINGS`].
pub(super) fn binding(id: BindingId) -> &'static Binding {
    BINDINGS
        .iter()
        .find(|binding| binding.id == id)
        .expect("every BindingId has a record in BINDINGS")
}

/// The bar spelling of an editorial mention: each member's primary key,
/// joined — `a/A/ctrl+a` derives here, never hand-concatenated.
pub(super) fn bar_spelling(ids: &[BindingId]) -> String {
    ids.iter()
        .map(|&id| binding(id).keys[0].spelling(Register::Bar))
        .collect::<Vec<_>>()
        .join("/")
}

/// The help overlay's rows in core order: every spelling of every
/// binding, consecutive same-label records merged into one row
/// (`j/k  ↓/↑`). `arrow` is the theme's drill glyph.
///
/// Spellings join with `/` within a position and with blanks between
/// positions: a comma is itself a bound key (issue #84), so punctuating
/// the seam would read as a third spelling.
pub fn help_rows(arrow: char) -> Vec<(String, String)> {
    let mut groups: Vec<Vec<&Binding>> = Vec::new();
    for binding in BINDINGS {
        match groups.last_mut() {
            Some(group) if group[0].help == binding.help => group.push(binding),
            _ => groups.push(vec![binding]),
        }
    }
    groups
        .into_iter()
        .map(|group| {
            let positions = group.iter().map(|b| b.keys.len()).max().unwrap_or(0);
            let spelling = (0..positions)
                .map(|position| {
                    group
                        .iter()
                        .filter_map(|b| b.keys.get(position))
                        .map(|key| key.spelling(Register::Help))
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .collect::<Vec<_>>()
                .join("  ");
            (spelling, group[0].help.render(arrow))
        })
        .collect()
}

fn panel_digits() -> Vec<char> {
    let mut digits: Vec<char> = Panel::ALL.into_iter().map(Panel::number).collect();
    digits.sort_unstable();
    digits
}

/// The bar's compact panel-digit span (`0-5`).
fn panels_bar_spelling() -> String {
    let digits = panel_digits();
    format!(
        "{}-{}",
        digits.first().expect("panels exist"),
        digits.last().expect("panels exist")
    )
}

/// The help register's explanatory span (`1-5, 0`): the drill digits as
/// a run, the Diff panel's off-run `0` named last.
fn panels_help_spelling() -> String {
    let digits = panel_digits();
    match digits.split_first() {
        Some((&'0', rest)) => format!(
            "{}-{}, 0",
            rest.first().expect("more than one panel"),
            rest.last().expect("more than one panel")
        ),
        _ => panels_bar_spelling(),
    }
}
