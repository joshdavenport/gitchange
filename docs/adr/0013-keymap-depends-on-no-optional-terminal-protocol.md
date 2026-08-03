# Keymap depends on no optional terminal protocol

No gitchange binding may depend on a modifier the terminal protocol cannot
be relied upon to report. In practice this bars exactly two shapes:

- **`shift` on a non-printing key** (`shift+enter`, `shift+tab`,
  `shift+↑`) — plain terminals deliver these identical to the unshifted
  key.
- **`ctrl+shift+<letter>`** — `ctrl+<letter>` is encoded as a control byte
  (0x01–0x1A) carrying neither case nor a shift bit, so `ctrl+shift+a` and
  `ctrl+a` are the same input.

Both become distinguishable only under the kitty keyboard protocol
(`DISAMBIGUATE_ESCAPE_CODES`), which most terminals do not implement.

Permitted, because they are protocol-independent: bare keys, `shift+<letter>`
(a distinct character byte), and `ctrl+<letter>` **matched without
inspecting `SHIFT`** — so a user holding shift gets the same action rather
than nothing.

## Why: a bound key that degrades is worse than one that is missing

The rule exists because of a real misfire (ticket 41, from the ticket 32
review). Hunk mode bound `shift+enter` to "assign the selected hunk" and
plain `enter` to "assign every hunk of the file". On a terminal without the
protocol, `shift+enter` arrives as `enter`, so a user asking for one hunk
silently got the whole file — the wrong action, not a no-op, and no surface
said so.

An unreachable binding is a visible failure: nothing happens, the user
tries the other key. A degrading binding is a silent one, which ADR 0007's
presentation rules exist to prevent. Terminal capability is not something a
user thinks about, so it must not be something a keymap depends on.

## Considered options

- **Detect at startup with `supports_keyboard_enhancement()` and vary the
  bindings** — rejected. It was the live alternative and would have worked,
  but it buys one binding at the price of two keymaps that must be kept in
  sync, displayed differently, and tested in both states forever. The
  degrading key is also the *nice* key; paying that structurally to keep it
  inverts the cost.
- **Push the enhancement flags and accept degradation** — the status quo,
  and the defect itself.
- **Keep the flags as a capability floor, restricting bindings anyway** —
  no gain: if no binding needs the protocol, the push is dead weight.

## Consequences

- The assign keymap (ticket 41) is `a` / `A` / `ctrl+a`, all
  protocol-independent, with `ctrl+a` matched shift-agnostically.
- `PushKeyboardEnhancementFlags` / `PopKeyboardEnhancementFlags` are no
  longer needed at TUI startup. Retaining them is harmless but must not be
  read as licence to bind against them.
- Anyone proposing a new binding checks it against the two barred shapes.
  A binding that needs the protocol is a signal the action wants a
  different key, not that the terminal wants upgrading.
- Not a claim about capability detection generally — colour depth, mouse
  reporting, and focus events (`EnableFocusChange`) are unaffected. This
  governs the keymap only.
